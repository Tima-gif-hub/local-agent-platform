#![warn(missing_docs)]

//! Shared types, Skill trait, and manifests.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::Arc,
};

use async_trait::async_trait;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

/// What the router produces from user input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InvocationPlan {
    /// Ordered skill invocations. MVP plans contain exactly one step.
    pub steps: Vec<SkillInvocation>,
    /// Which routing stage produced the plan.
    pub source: RouteSource,
    /// Router confidence from 0.0 to 1.0.
    pub confidence: f32,
}

/// A single skill invocation and its JSON parameters.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillInvocation {
    /// Skill identifier, for example `system.open_app`.
    pub skill_id: String,
    /// Parameters validated against the skill manifest's JSON Schema.
    pub params: Value,
}

/// Routing stage that produced an invocation plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RouteSource {
    /// Exact or regex rule match.
    Rule,
    /// Fuzzy match against examples and aliases.
    Fuzzy,
    /// LLM-produced structured route.
    Llm,
}

/// Risk level declared by a skill.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum RiskLevel {
    /// Runs without user confirmation.
    Safe,
    /// Requires confirmation by default.
    Moderate,
    /// High-risk action that must never be guessed.
    Destructive,
}

/// Filesystem scope for a permission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PathScope {
    /// Any path on the system.
    Anywhere,
    /// Only paths inside this directory tree.
    Within(PathBuf),
}

/// Permission declared by a skill or granted to a skill context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Permission {
    /// Read files within the given scope.
    FsRead(PathScope),
    /// Write files within the given scope.
    FsWrite(PathScope),
    /// Spawn a process.
    ProcessSpawn,
    /// Inspect running processes.
    ProcessInspect,
    /// Access the network.
    Network,
    /// Access structured memory.
    Memory,
}

/// Public manifest describing a skill.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillManifest {
    /// Skill id in `category.name` format.
    pub id: String,
    /// Skill implementation version.
    pub version: Version,
    /// English description used in routing prompts.
    pub description: String,
    /// JSON Schema for invocation parameters.
    pub params_schema: Value,
    /// Permissions required by the skill.
    pub permissions: Vec<Permission>,
    /// Declared risk level.
    pub risk: RiskLevel,
    /// Few-shot examples used by the router.
    pub examples: Vec<String>,
    /// Optional regex triggers with named capture groups for routing.
    #[serde(default)]
    pub triggers: Vec<Trigger>,
}

/// Regex trigger used by the router.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    /// Regex pattern with optional named capture groups.
    pub pattern: String,
}

impl SkillManifest {
    /// Parses and validates a skill manifest from JSON.
    pub fn from_json_str(input: &str) -> Result<Self, SkillError> {
        let manifest: Self = serde_json::from_str(input)
            .map_err(|error| SkillError::InvalidParams(error.to_string()))?;

        if !is_valid_skill_id(&manifest.id) {
            return Err(SkillError::InvalidParams(format!(
                "invalid skill id: {}",
                manifest.id
            )));
        }

        jsonschema::validator_for(&manifest.params_schema)
            .map_err(|error| SkillError::InvalidParams(error.to_string()))?;

        Ok(manifest)
    }
}

/// Result returned by a skill after execution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOutput {
    /// English canonical summary; UI may translate it at the edge.
    pub summary: String,
    /// Structured output data.
    pub data: Value,
}

/// Errors produced by parameter validation, permission checks, confirmation, or execution.
#[derive(Debug, Error)]
pub enum SkillError {
    /// Parameters or manifest content failed validation.
    #[error("invalid params: {0}")]
    InvalidParams(String),
    /// A requested operation is outside granted permissions.
    #[error("permission denied")]
    PermissionDenied,
    /// The user denied a required confirmation.
    #[error("confirmation denied")]
    ConfirmationDenied,
    /// Skill execution failed.
    #[error("execution failed: {0}")]
    Execution(String),
}

/// User confirmation channel used by skill contexts.
#[async_trait]
pub trait Confirmer: Send + Sync {
    /// Returns true if the user confirmed the prompt.
    async fn confirm(&self, prompt: String) -> bool;
}

/// Process spawning port used by skills.
pub trait Spawner: Send + Sync {
    /// Spawns a program with arguments.
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), SkillError>;
}

/// Memory access port used by memory skills.
#[async_trait]
pub trait MemoryPort: Send + Sync {
    /// Stores a key-value memory fact.
    async fn remember(&self, key: String, value: String) -> Result<(), SkillError>;

    /// Recalls a memory fact by key.
    async fn recall(&self, key: String) -> Result<Option<String>, SkillError>;
}

/// Runtime context passed to skills.
#[derive(Clone)]
pub struct SkillContext {
    /// Permissions granted to the running skill.
    pub granted: Vec<Permission>,
    /// Confirmation channel for moderate or destructive operations.
    pub confirmer: Arc<dyn Confirmer>,
    /// Process spawning port.
    pub spawner: Arc<dyn Spawner>,
    /// Optional memory access port.
    pub memory: Option<Arc<dyn MemoryPort>>,
}

impl SkillContext {
    /// Creates a new skill context.
    pub fn new(granted: Vec<Permission>, confirmer: Arc<dyn Confirmer>) -> Self {
        Self {
            granted,
            confirmer,
            spawner: Arc::new(SystemSpawner),
            memory: None,
        }
    }

    /// Creates a new skill context with an injected spawner.
    pub fn with_spawner(
        granted: Vec<Permission>,
        confirmer: Arc<dyn Confirmer>,
        spawner: Arc<dyn Spawner>,
    ) -> Self {
        Self {
            granted,
            confirmer,
            spawner,
            memory: None,
        }
    }

    /// Adds a memory port to the context.
    pub fn with_memory(mut self, memory: Arc<dyn MemoryPort>) -> Self {
        self.memory = Some(memory);
        self
    }

    /// Reads a file after checking `Permission::FsRead`.
    pub fn fs_read(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, SkillError> {
        let path = path.as_ref();
        self.check_fs_read(path)?;
        fs::read(path).map_err(|error| SkillError::Execution(error.to_string()))
    }

    /// Checks whether a path is allowed by `Permission::FsRead`.
    pub fn check_fs_read(&self, path: impl AsRef<Path>) -> Result<(), SkillError> {
        self.ensure_path_permission(path.as_ref(), PermissionKind::Read)
    }

    /// Writes a file after checking `Permission::FsWrite`.
    pub fn fs_write(
        &self,
        path: impl AsRef<Path>,
        contents: impl AsRef<[u8]>,
    ) -> Result<(), SkillError> {
        let path = path.as_ref();
        self.check_fs_write(path)?;
        fs::write(path, contents).map_err(|error| SkillError::Execution(error.to_string()))
    }

    /// Checks whether a path is allowed by `Permission::FsWrite`.
    pub fn check_fs_write(&self, path: impl AsRef<Path>) -> Result<(), SkillError> {
        self.ensure_path_permission(path.as_ref(), PermissionKind::Write)
    }

    /// Spawns a process after checking `Permission::ProcessSpawn`.
    pub fn spawn(&self, command: &mut Command) -> Result<Child, SkillError> {
        if !self
            .granted
            .iter()
            .any(|permission| matches!(permission, Permission::ProcessSpawn))
        {
            return Err(SkillError::PermissionDenied);
        }

        command
            .spawn()
            .map_err(|error| SkillError::Execution(error.to_string()))
    }

    /// Spawns a process through the configured spawner after checking permission.
    pub fn spawn_program(&self, program: &str, args: &[String]) -> Result<(), SkillError> {
        if !self
            .granted
            .iter()
            .any(|permission| matches!(permission, Permission::ProcessSpawn))
        {
            return Err(SkillError::PermissionDenied);
        }

        self.spawner.spawn(program, args)
    }

    /// Requests user confirmation through the configured confirmer.
    pub async fn request_confirmation(&self, prompt: String) -> Result<(), SkillError> {
        if self.confirmer.confirm(prompt).await {
            Ok(())
        } else {
            Err(SkillError::ConfirmationDenied)
        }
    }

    fn ensure_path_permission(&self, path: &Path, kind: PermissionKind) -> Result<(), SkillError> {
        let allowed = self
            .granted
            .iter()
            .any(|permission| match (kind, permission) {
                (PermissionKind::Read, Permission::FsRead(scope))
                | (PermissionKind::Write, Permission::FsWrite(scope)) => scope_allows(scope, path),
                _ => false,
            });

        if allowed {
            Ok(())
        } else {
            Err(SkillError::PermissionDenied)
        }
    }
}

struct SystemSpawner;

impl Spawner for SystemSpawner {
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), SkillError> {
        Command::new(program)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|error| SkillError::Execution(error.to_string()))
    }
}

/// Skill extension point.
#[async_trait]
pub trait Skill: Send + Sync {
    /// Returns the skill manifest.
    fn manifest(&self) -> &SkillManifest;

    /// Executes the skill with validated JSON parameters and a scoped context.
    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError>;
}

/// Validates invocation parameters against a skill manifest's JSON Schema.
pub fn validate_params(manifest: &SkillManifest, params: &Value) -> Result<(), SkillError> {
    let validator = jsonschema::validator_for(&manifest.params_schema)
        .map_err(|error| SkillError::InvalidParams(error.to_string()))?;

    if validator.is_valid(params) {
        Ok(())
    } else {
        let message = validator
            .iter_errors(params)
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        Err(SkillError::InvalidParams(message))
    }
}

#[derive(Clone, Copy)]
enum PermissionKind {
    Read,
    Write,
}

fn is_valid_skill_id(id: &str) -> bool {
    let mut parts = id.split('.');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(category), Some(name), None) => is_id_part(category) && is_id_part(name),
        _ => false,
    }
}

fn is_id_part(part: &str) -> bool {
    !part.is_empty() && part.chars().all(|ch| ch.is_ascii_lowercase() || ch == '_')
}

fn scope_allows(scope: &PathScope, path: &Path) -> bool {
    match scope {
        PathScope::Anywhere => true,
        PathScope::Within(root) => {
            let Ok(root) = comparable_path(root) else {
                return false;
            };
            let Ok(path) = comparable_path(path) else {
                return false;
            };
            path.starts_with(root)
        }
    }
}

fn comparable_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.exists() {
        return path.canonicalize();
    }

    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    let parent = absolute.parent().unwrap_or_else(|| Path::new("."));
    let file_name = absolute.file_name();
    let mut comparable = parent.canonicalize()?;

    if let Some(file_name) = file_name {
        comparable.push(file_name);
    }

    Ok(comparable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug)]
    struct AlwaysConfirm(bool);

    #[async_trait]
    impl Confirmer for AlwaysConfirm {
        async fn confirm(&self, _prompt: String) -> bool {
            self.0
        }
    }

    fn manifest_json() -> String {
        json!({
            "id": "system.open_app",
            "version": "0.1.0",
            "description": "Open an installed application.",
            "params_schema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"],
                "additionalProperties": false
            },
            "permissions": ["ProcessSpawn"],
            "risk": "Moderate",
            "examples": ["open chrome"]
        })
        .to_string()
    }

    #[test]
    fn parses_valid_manifest() {
        let manifest = SkillManifest::from_json_str(&manifest_json()).expect("valid manifest");

        assert_eq!(manifest.id, "system.open_app");
        assert_eq!(manifest.version, Version::new(0, 1, 0));
        assert_eq!(manifest.risk, RiskLevel::Moderate);
    }

    #[test]
    fn rejects_invalid_manifest_id() {
        let mut value: Value = serde_json::from_str(&manifest_json()).expect("json");
        value["id"] = json!("system-open-app");

        let error = SkillManifest::from_json_str(&value.to_string()).expect_err("invalid id");
        assert!(matches!(error, SkillError::InvalidParams(_)));
    }

    #[test]
    fn rejects_invalid_manifest_schema() {
        let mut value: Value = serde_json::from_str(&manifest_json()).expect("json");
        value["params_schema"] = json!({ "type": 10 });

        let error = SkillManifest::from_json_str(&value.to_string()).expect_err("invalid schema");
        assert!(matches!(error, SkillError::InvalidParams(_)));
    }

    #[test]
    fn validates_params_against_manifest_schema() {
        let manifest = SkillManifest::from_json_str(&manifest_json()).expect("valid manifest");

        validate_params(&manifest, &json!({ "name": "chrome" })).expect("valid params");
        let error = validate_params(&manifest, &json!({ "name": 42 })).expect_err("invalid params");
        assert!(matches!(error, SkillError::InvalidParams(_)));
    }

    #[test]
    fn rejects_write_outside_path_scope() {
        let allowed = tempfile::tempdir().expect("allowed dir");
        let denied = tempfile::tempdir().expect("denied dir");
        let denied_file = denied.path().join("outside.txt");
        let ctx = SkillContext::new(
            vec![Permission::FsWrite(PathScope::Within(
                allowed.path().to_path_buf(),
            ))],
            Arc::new(AlwaysConfirm(true)),
        );

        let error = ctx
            .fs_write(&denied_file, b"nope")
            .expect_err("outside scope");

        assert!(matches!(error, SkillError::PermissionDenied));
        assert!(!denied_file.exists());
    }

    #[test]
    fn rejects_read_outside_path_scope() {
        let allowed = tempfile::tempdir().expect("allowed dir");
        let denied = tempfile::tempdir().expect("denied dir");
        let denied_file = denied.path().join("outside.txt");
        fs::write(&denied_file, "nope").expect("write denied file");
        let ctx = SkillContext::new(
            vec![Permission::FsRead(PathScope::Within(
                allowed.path().to_path_buf(),
            ))],
            Arc::new(AlwaysConfirm(true)),
        );

        let error = ctx.fs_read(&denied_file).expect_err("outside scope");

        assert!(matches!(error, SkillError::PermissionDenied));
    }

    #[test]
    fn allows_write_inside_path_scope() {
        let allowed = tempfile::tempdir().expect("allowed dir");
        let file = allowed.path().join("inside.txt");
        let ctx = SkillContext::new(
            vec![Permission::FsWrite(PathScope::Within(
                allowed.path().to_path_buf(),
            ))],
            Arc::new(AlwaysConfirm(true)),
        );

        ctx.fs_write(&file, b"ok").expect("inside scope");

        assert_eq!(fs::read_to_string(file).expect("written"), "ok");
    }

    #[test]
    fn serde_round_trips_public_types() {
        let plan = InvocationPlan {
            steps: vec![SkillInvocation {
                skill_id: "system.open_app".to_string(),
                params: json!({ "name": "chrome" }),
            }],
            source: RouteSource::Rule,
            confidence: 1.0,
        };
        let plan_json = serde_json::to_string(&plan).expect("serialize plan");
        assert_eq!(
            serde_json::from_str::<InvocationPlan>(&plan_json).expect("deserialize plan"),
            plan
        );

        let permission = Permission::FsRead(PathScope::Within(PathBuf::from("C:/Users")));
        let permission_json = serde_json::to_string(&permission).expect("serialize permission");
        assert_eq!(
            serde_json::from_str::<Permission>(&permission_json).expect("deserialize permission"),
            permission
        );

        let manifest = SkillManifest::from_json_str(&manifest_json()).expect("valid manifest");
        let manifest_json = serde_json::to_string(&manifest).expect("serialize manifest");
        assert_eq!(
            serde_json::from_str::<SkillManifest>(&manifest_json).expect("deserialize manifest"),
            manifest
        );

        let output = SkillOutput {
            summary: "Opened Chrome.".to_string(),
            data: json!({ "ok": true }),
        };
        let output_json = serde_json::to_string(&output).expect("serialize output");
        assert_eq!(
            serde_json::from_str::<SkillOutput>(&output_json).expect("deserialize output"),
            output
        );
    }
}
