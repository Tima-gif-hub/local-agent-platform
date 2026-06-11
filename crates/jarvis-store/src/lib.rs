//! SQLite storage for memory, audit, settings, and migrations.

use std::{fmt, path::PathBuf, str::FromStr};

use async_trait::async_trait;
use directories::BaseDirs;
use jarvis_types::{MemoryPort, RiskLevel, RouteSource, SkillError};
use serde_json::Value;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
    Row, SqlitePool,
};
use thiserror::Error;

/// Result type used by the storage layer.
pub type StoreResult<T> = Result<T, StoreError>;

/// Storage errors.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The operating system did not expose a data directory.
    #[error("data directory is unavailable")]
    DataDirUnavailable,
    /// A database operation failed.
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    /// A migration failed.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// A filesystem operation failed.
    #[error("filesystem error: {0}")]
    Fs(#[from] std::io::Error),
    /// Stored JSON could not be parsed.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    /// Stored enum text was not recognized.
    #[error("invalid stored value: {0}")]
    InvalidStoredValue(String),
}

/// SQLite store handle.
#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    /// Opens the default database path and runs migrations.
    pub async fn open_default() -> StoreResult<Self> {
        let path = default_db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Self::open_path(path).await
    }

    /// Opens a database at a filesystem path and runs migrations.
    pub async fn open_path(path: impl Into<PathBuf>) -> StoreResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Opens a database URL, such as `sqlite::memory:`, and runs migrations.
    pub async fn open_url(url: &str) -> StoreResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(url)
            .await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    /// Runs database migrations. Running this more than once is a no-op.
    pub async fn migrate(&self) -> StoreResult<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }

    /// Returns the settings repository.
    pub fn settings(&self) -> Settings {
        Settings::new(self.pool.clone())
    }

    /// Returns the memory facts repository.
    pub fn memory_facts(&self) -> MemoryFacts {
        MemoryFacts::new(self.pool.clone())
    }

    /// Returns the audit repository.
    pub fn audit(&self) -> Audit {
        Audit::new(self.pool.clone())
    }

    /// Returns the conversations repository.
    pub fn conversations(&self) -> Conversations {
        Conversations::new(self.pool.clone())
    }
}

#[async_trait]
impl MemoryPort for Store {
    async fn remember(&self, key: String, value: String) -> Result<(), SkillError> {
        self.memory_facts()
            .upsert(&key, &value, "memory.remember")
            .await
            .map_err(|error| SkillError::Execution(error.to_string()))
    }

    async fn recall(&self, key: String) -> Result<Option<String>, SkillError> {
        self.memory_facts()
            .get(&key)
            .await
            .map(|fact| fact.map(|fact| fact.value))
            .map_err(|error| SkillError::Execution(error.to_string()))
    }
}

/// Returns `%APPDATA%/jarvis/jarvis.db` on Windows, and the platform data dir elsewhere.
pub fn default_db_path() -> StoreResult<PathBuf> {
    let base_dirs = BaseDirs::new().ok_or(StoreError::DataDirUnavailable)?;
    Ok(base_dirs.data_dir().join("jarvis").join("jarvis.db"))
}

/// Settings repository.
#[derive(Clone)]
pub struct Settings {
    pool: SqlitePool,
}

impl Settings {
    fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Sets a key to a value.
    pub async fn set(&self, key: &str, value: &str) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Gets a setting by key.
    pub async fn get(&self, key: &str) -> StoreResult<Option<String>> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?1")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(|row| row.get("value")))
    }
}

/// Stored memory fact.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryFact {
    /// Fact key.
    pub key: String,
    /// Fact value.
    pub value: String,
    /// Source that wrote the fact.
    pub source: String,
    /// Last update timestamp.
    pub updated_at: String,
}

/// Memory facts repository.
#[derive(Clone)]
pub struct MemoryFacts {
    pool: SqlitePool,
}

impl MemoryFacts {
    fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts or updates a memory fact.
    pub async fn upsert(&self, key: &str, value: &str, source: &str) -> StoreResult<()> {
        sqlx::query(
            "INSERT INTO memory_facts (key, value, source, updated_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                source = excluded.source,
                updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(source)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Gets a memory fact by key.
    pub async fn get(&self, key: &str) -> StoreResult<Option<MemoryFact>> {
        let row = sqlx::query(
            "SELECT key, value, source, updated_at
             FROM memory_facts
             WHERE key = ?1",
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        row.map(memory_fact_from_row).transpose()
    }

    /// Searches memory facts by key prefix.
    pub async fn search_by_prefix(&self, prefix: &str) -> StoreResult<Vec<MemoryFact>> {
        let pattern = format!("{prefix}%");
        let rows = sqlx::query(
            "SELECT key, value, source, updated_at
             FROM memory_facts
             WHERE key LIKE ?1
             ORDER BY key ASC",
        )
        .bind(pattern)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(memory_fact_from_row).collect()
    }

    /// Deletes a memory fact by key.
    pub async fn delete(&self, key: &str) -> StoreResult<bool> {
        let result = sqlx::query("DELETE FROM memory_facts WHERE key = ?1")
            .bind(key)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Audit outcome stored as text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuditOutcome {
    /// Skill completed successfully.
    Success,
    /// Skill failed during execution.
    Failed,
    /// Skill was denied by permission checks.
    DeniedPermission,
    /// Skill was denied by user confirmation.
    DeniedConfirmation,
    /// Skill parameters were invalid.
    InvalidParams,
}

impl fmt::Display for AuditOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Success => "success",
            Self::Failed => "failed",
            Self::DeniedPermission => "denied_permission",
            Self::DeniedConfirmation => "denied_confirmation",
            Self::InvalidParams => "invalid_params",
        })
    }
}

impl FromStr for AuditOutcome {
    type Err = StoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "success" => Ok(Self::Success),
            "failed" => Ok(Self::Failed),
            "denied_permission" => Ok(Self::DeniedPermission),
            "denied_confirmation" => Ok(Self::DeniedConfirmation),
            "invalid_params" => Ok(Self::InvalidParams),
            other => Err(StoreError::InvalidStoredValue(other.to_string())),
        }
    }
}

/// New audit entry.
#[derive(Clone, Debug)]
pub struct NewAuditRecord {
    /// Skill id.
    pub skill_id: String,
    /// Invocation params.
    pub params: Value,
    /// Declared skill risk.
    pub risk: RiskLevel,
    /// Execution outcome.
    pub outcome: AuditOutcome,
    /// Route source.
    pub route_source: RouteSource,
}

/// Stored audit record.
#[derive(Clone, Debug, PartialEq)]
pub struct AuditRecord {
    /// Record id.
    pub id: i64,
    /// Timestamp.
    pub ts: String,
    /// Skill id.
    pub skill_id: String,
    /// Invocation params.
    pub params: Value,
    /// Declared skill risk.
    pub risk: RiskLevel,
    /// Execution outcome.
    pub outcome: AuditOutcome,
    /// Route source.
    pub route_source: RouteSource,
}

/// Audit list filter.
#[derive(Clone, Debug, Default)]
pub struct AuditFilter {
    /// Optional skill id filter.
    pub skill_id: Option<String>,
    /// Optional outcome filter.
    pub outcome: Option<AuditOutcome>,
}

/// Audit repository.
#[derive(Clone)]
pub struct Audit {
    pool: SqlitePool,
}

impl Audit {
    fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Appends a new audit record.
    pub async fn append(&self, record: NewAuditRecord) -> StoreResult<i64> {
        let params_json = serde_json::to_string(&record.params)?;
        let result = sqlx::query(
            "INSERT INTO audit_log (skill_id, params_json, risk, outcome, route_source)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(record.skill_id)
        .bind(params_json)
        .bind(risk_to_text(record.risk))
        .bind(record.outcome.to_string())
        .bind(route_source_to_text(record.route_source))
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Lists audit records with optional filters and pagination.
    pub async fn list(
        &self,
        filter: AuditFilter,
        limit: i64,
        offset: i64,
    ) -> StoreResult<Vec<AuditRecord>> {
        let rows = match (filter.skill_id, filter.outcome) {
            (Some(skill_id), Some(outcome)) => {
                sqlx::query(
                    "SELECT id, ts, skill_id, params_json, risk, outcome, route_source
                     FROM audit_log
                     WHERE skill_id = ?1 AND outcome = ?2
                     ORDER BY id DESC
                     LIMIT ?3 OFFSET ?4",
                )
                .bind(skill_id)
                .bind(outcome.to_string())
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?
            }
            (Some(skill_id), None) => {
                sqlx::query(
                    "SELECT id, ts, skill_id, params_json, risk, outcome, route_source
                     FROM audit_log
                     WHERE skill_id = ?1
                     ORDER BY id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .bind(skill_id)
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?
            }
            (None, Some(outcome)) => {
                sqlx::query(
                    "SELECT id, ts, skill_id, params_json, risk, outcome, route_source
                     FROM audit_log
                     WHERE outcome = ?1
                     ORDER BY id DESC
                     LIMIT ?2 OFFSET ?3",
                )
                .bind(outcome.to_string())
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?
            }
            (None, None) => {
                sqlx::query(
                    "SELECT id, ts, skill_id, params_json, risk, outcome, route_source
                     FROM audit_log
                     ORDER BY id DESC
                     LIMIT ?1 OFFSET ?2",
                )
                .bind(limit)
                .bind(offset)
                .fetch_all(&self.pool)
                .await?
            }
        };

        rows.into_iter().map(audit_record_from_row).collect()
    }
}

/// Conversation row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Conversation {
    /// Conversation id.
    pub id: i64,
    /// Start timestamp.
    pub started_at: String,
}

/// Message row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    /// Message id.
    pub id: i64,
    /// Parent conversation id.
    pub conversation_id: i64,
    /// Message role.
    pub role: String,
    /// Message content.
    pub content: String,
    /// Message timestamp.
    pub ts: String,
}

/// Conversations repository.
#[derive(Clone)]
pub struct Conversations {
    pool: SqlitePool,
}

impl Conversations {
    fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Creates a conversation and returns its id.
    pub async fn create(&self) -> StoreResult<i64> {
        let result = sqlx::query("INSERT INTO conversations DEFAULT VALUES")
            .execute(&self.pool)
            .await?;
        Ok(result.last_insert_rowid())
    }

    /// Lists conversations newest first.
    pub async fn list(&self, limit: i64, offset: i64) -> StoreResult<Vec<Conversation>> {
        let rows = sqlx::query(
            "SELECT id, started_at
             FROM conversations
             ORDER BY id DESC
             LIMIT ?1 OFFSET ?2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Conversation {
                id: row.get("id"),
                started_at: row.get("started_at"),
            })
            .collect())
    }

    /// Adds a message to a conversation and returns its id.
    pub async fn add_message(
        &self,
        conversation_id: i64,
        role: &str,
        content: &str,
    ) -> StoreResult<i64> {
        let result = sqlx::query(
            "INSERT INTO messages (conversation_id, role, content)
             VALUES (?1, ?2, ?3)",
        )
        .bind(conversation_id)
        .bind(role)
        .bind(content)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    /// Lists messages in a conversation oldest first.
    pub async fn messages(&self, conversation_id: i64) -> StoreResult<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT id, conversation_id, role, content, ts
             FROM messages
             WHERE conversation_id = ?1
             ORDER BY id ASC",
        )
        .bind(conversation_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Message {
                id: row.get("id"),
                conversation_id: row.get("conversation_id"),
                role: row.get("role"),
                content: row.get("content"),
                ts: row.get("ts"),
            })
            .collect())
    }
}

fn memory_fact_from_row(row: sqlx::sqlite::SqliteRow) -> StoreResult<MemoryFact> {
    Ok(MemoryFact {
        key: row.get("key"),
        value: row.get("value"),
        source: row.get("source"),
        updated_at: row.get("updated_at"),
    })
}

fn audit_record_from_row(row: sqlx::sqlite::SqliteRow) -> StoreResult<AuditRecord> {
    let params_json: String = row.get("params_json");
    let risk: String = row.get("risk");
    let outcome: String = row.get("outcome");
    let route_source: String = row.get("route_source");

    Ok(AuditRecord {
        id: row.get("id"),
        ts: row.get("ts"),
        skill_id: row.get("skill_id"),
        params: serde_json::from_str(&params_json)?,
        risk: risk_from_text(&risk)?,
        outcome: AuditOutcome::from_str(&outcome)?,
        route_source: route_source_from_text(&route_source)?,
    })
}

fn risk_to_text(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Safe => "safe",
        RiskLevel::Moderate => "moderate",
        RiskLevel::Destructive => "destructive",
    }
}

fn risk_from_text(value: &str) -> StoreResult<RiskLevel> {
    match value {
        "safe" => Ok(RiskLevel::Safe),
        "moderate" => Ok(RiskLevel::Moderate),
        "destructive" => Ok(RiskLevel::Destructive),
        other => Err(StoreError::InvalidStoredValue(other.to_string())),
    }
}

fn route_source_to_text(source: RouteSource) -> &'static str {
    match source {
        RouteSource::Rule => "rule",
        RouteSource::Fuzzy => "fuzzy",
        RouteSource::Llm => "llm",
    }
}

fn route_source_from_text(value: &str) -> StoreResult<RouteSource> {
    match value {
        "rule" => Ok(RouteSource::Rule),
        "fuzzy" => Ok(RouteSource::Fuzzy),
        "llm" => Ok(RouteSource::Llm),
        other => Err(StoreError::InvalidStoredValue(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn memory_store() -> Store {
        Store::open_url("sqlite::memory:")
            .await
            .expect("in-memory store")
    }

    #[tokio::test]
    async fn fresh_db_migrates_and_migrations_are_idempotent() {
        let store = memory_store().await;

        store.migrate().await.expect("second migration");
        store
            .settings()
            .set("theme", "dark")
            .await
            .expect("settings table exists");
    }

    #[tokio::test]
    async fn open_path_creates_missing_database_file() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let db_path = tempdir.path().join("nested").join("jarvis.db");

        assert!(!db_path.exists());

        let store = Store::open_path(&db_path).await.expect("open path");
        store
            .settings()
            .set("first_launch", "ok")
            .await
            .expect("write setting");

        assert!(db_path.exists());
        assert_eq!(
            store
                .settings()
                .get("first_launch")
                .await
                .expect("read setting")
                .as_deref(),
            Some("ok")
        );
    }

    #[tokio::test]
    async fn settings_set_and_get_values() {
        let settings = memory_store().await.settings();

        settings.set("model", "qwen").await.expect("set");
        settings.set("model", "llama").await.expect("update");

        assert_eq!(
            settings.get("model").await.expect("get").as_deref(),
            Some("llama")
        );
        assert_eq!(settings.get("missing").await.expect("missing"), None);
    }

    #[tokio::test]
    async fn memory_facts_support_upsert_get_search_and_delete() {
        let memory = memory_store().await.memory_facts();

        memory
            .upsert("project.root", "C:/dev/jarvis", "test")
            .await
            .expect("insert");
        memory
            .upsert("project.name", "Jarvis", "test")
            .await
            .expect("insert");
        memory
            .upsert("project.root", "D:/dev/jarvis", "user")
            .await
            .expect("update");

        let root = memory
            .get("project.root")
            .await
            .expect("get")
            .expect("fact");
        assert_eq!(root.value, "D:/dev/jarvis");
        assert_eq!(root.source, "user");

        let results = memory.search_by_prefix("project.").await.expect("search");
        assert_eq!(results.len(), 2);

        assert!(memory.delete("project.name").await.expect("delete"));
        assert_eq!(memory.get("project.name").await.expect("deleted"), None);
    }

    #[tokio::test]
    async fn audit_appends_lists_and_filters_records() {
        let audit = memory_store().await.audit();

        let first = audit
            .append(NewAuditRecord {
                skill_id: "system.open_app".to_string(),
                params: json!({ "name": "chrome" }),
                risk: RiskLevel::Moderate,
                outcome: AuditOutcome::Success,
                route_source: RouteSource::Rule,
            })
            .await
            .expect("append first");
        let second = audit
            .append(NewAuditRecord {
                skill_id: "files.search".to_string(),
                params: json!({ "pattern": "*.rs" }),
                risk: RiskLevel::Safe,
                outcome: AuditOutcome::DeniedPermission,
                route_source: RouteSource::Fuzzy,
            })
            .await
            .expect("append second");

        assert!(second > first);

        let all = audit
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("list all");
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].skill_id, "files.search");

        let filtered = audit
            .list(
                AuditFilter {
                    skill_id: Some("system.open_app".to_string()),
                    outcome: Some(AuditOutcome::Success),
                },
                10,
                0,
            )
            .await
            .expect("filtered");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].params, json!({ "name": "chrome" }));
    }

    #[tokio::test]
    async fn conversations_create_and_store_messages() {
        let conversations = memory_store().await.conversations();

        let conversation_id = conversations.create().await.expect("create");
        conversations
            .add_message(conversation_id, "user", "hello")
            .await
            .expect("user message");
        conversations
            .add_message(conversation_id, "assistant", "hi")
            .await
            .expect("assistant message");

        let listed = conversations.list(10, 0).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, conversation_id);

        let messages = conversations
            .messages(conversation_id)
            .await
            .expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[1].content, "hi");
    }
}
