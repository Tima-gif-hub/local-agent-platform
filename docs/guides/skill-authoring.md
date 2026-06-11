# Skill Authoring Guide (MVP — built-in skills)

This guide covers how to add a built-in skill to Jarvis in v0.1/v0.2. Built-in skills
are in-process Rust that implement the `Skill` trait in `crates/jarvis-skills`.

Out-of-process skills (Python sidecars) arrive in v0.3 using the same manifest format;
see the note at the bottom.

---

## 1. Skill manifest

Every skill is described by a JSON manifest. For built-in skills the canonical manifest
lives in `skills/<category>/<name>.json` and is embedded into the binary at build time.
The manifest is also constructed in Rust (as `SkillManifest`) and returned from
`manifest()`. The JSON file is the human-readable source of truth; the Rust struct is
derived from it.

### Full annotated example — `files.compress_folder`

```json
{
  "id": "files.compress_folder",
  "version": "0.1.0",
  "description": "Compress a folder into a zip archive. The archive is placed next to the source folder unless an output path is given.",
  "params_schema": {
    "type": "object",
    "properties": {
      "folder_path": {
        "type": "string",
        "description": "Absolute path to the folder to compress."
      },
      "output_path": {
        "type": "string",
        "description": "Absolute path for the output .zip file. Optional — defaults to <folder_name>.zip beside the source."
      },
      "overwrite": {
        "type": "boolean",
        "description": "If true, overwrite an existing archive. Defaults to false.",
        "default": false
      }
    },
    "required": ["folder_path"],
    "additionalProperties": false
  },
  "permissions": [{ "FsRead": "Anywhere" }, { "FsWrite": "Anywhere" }],
  "risk": "Moderate",
  "examples": [
    "compress the reports folder",
    "zip D:\\projects\\myapp into an archive",
    "сожми папку загрузки в архив",
    "make a zip of C:\\backup"
  ]
}
```

Field notes:

| Field | Rules |
|---|---|
| `id` | `<category>.<name>`, lowercase snake_case. Category groups related skills (`files`, `system`, `web`, `memory`). |
| `version` | Semver string. Bump when params_schema changes in a breaking way. |
| `description` | English. This string is included verbatim in the LLM routing prompt — write it for the model, not for humans. |
| `params_schema` | Standard JSON Schema (draft-07). Always `"type": "object"`, always `"additionalProperties": false`. Required fields listed in `"required"`. |
| `permissions` | List of `Permission` values as JSON. Unit variants are strings: `"ProcessSpawn"`, `"ProcessInspect"`, `"Network"`, `"Memory"`. Filesystem variants carry a scope: `{ "FsRead": "Anywhere" }` or `{ "FsWrite": { "Within": "C:/some/dir" } }`. Declare the minimum needed — the executor may narrow `Anywhere` to the invocation's target folder at runtime. |
| `risk` | `"Safe"` (no confirmation), `"Moderate"` (confirmation by default), `"Destructive"` (always confirmation). |
| `examples` | Natural-language phrases a user might type to invoke this skill. Include at least one non-English example where the skill is plausibly multilingual. These feed both fuzzy matching and the LLM few-shot prompt. |

---

## 2. Rust implementation

### 2.1 The `Skill` trait (frozen — `jarvis-types`)

```rust
#[async_trait]
pub trait Skill: Send + Sync {
    fn manifest(&self) -> &SkillManifest;

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SkillContext,
    ) -> Result<SkillOutput, SkillError>;
}
```

This signature is frozen (🔒). Do not propose changes without an ADR.

### 2.2 Implementation skeleton

Create `crates/jarvis-skills/src/files/compress_folder.rs`:

```rust
use async_trait::async_trait;
use jarvis_types::{
    PathScope, Permission, RiskLevel, SkillContext, SkillError, SkillManifest, SkillOutput,
};
use semver::Version;
use serde::Deserialize;

pub struct CompressFolderSkill {
    manifest: SkillManifest,
}

impl CompressFolderSkill {
    pub fn new() -> Self {
        Self {
            manifest: SkillManifest {
                id: "files.compress_folder".to_string(),
                version: Version::new(0, 1, 0),
                description: "Compress a folder into a zip archive. \
                    The archive is placed next to the source folder \
                    unless an output path is given."
                    .to_string(),
                params_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "folder_path": { "type": "string" },
                        "output_path": { "type": "string" },
                        "overwrite":   { "type": "boolean", "default": false }
                    },
                    "required": ["folder_path"],
                    "additionalProperties": false
                }),
                permissions: vec![
                    Permission::FsRead(PathScope::Anywhere),
                    Permission::FsWrite(PathScope::Anywhere),
                ],
                risk: RiskLevel::Moderate,
                examples: vec![
                    "compress the reports folder".to_string(),
                    "zip D:\\projects\\myapp into an archive".to_string(),
                    "сожми папку загрузки в архив".to_string(),
                    "make a zip of C:\\backup".to_string(),
                ],
            },
        }
    }
}

// Typed params — always deserialize before touching the OS.
#[derive(Deserialize)]
struct Params {
    folder_path: String,
    output_path: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

#[async_trait]
impl jarvis_types::Skill for CompressFolderSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SkillContext,
    ) -> Result<SkillOutput, SkillError> {
        // 1. Deserialize — schema was pre-validated by the executor, but
        //    typed deserialization catches anything JSON Schema missed.
        let p: Params = serde_json::from_value(params)
            .map_err(|e| SkillError::InvalidParams(e.to_string()))?;

        // 2. Resolve paths using SkillContext helpers.
        //    NEVER call std::fs or std::path::Path directly for I/O.
        let src = ctx.resolve_path(&p.folder_path)?;
        let dest = match p.output_path {
            Some(ref raw) => ctx.resolve_path(raw)?,
            None => src.with_extension("zip"),
        };

        // 3. Check for overwrite safety before doing any work.
        if dest.exists() && !p.overwrite {
            return Err(SkillError::Execution(format!(
                "Archive already exists at {}. Pass overwrite=true to replace it.",
                dest.display()
            )));
        }

        // 4. Do the work using ctx helpers (fs_read_dir, fs_create_file, …).
        ctx.report_progress("Compressing…").await;
        // … actual zip logic using ctx.fs_* helpers …

        Ok(SkillOutput {
            summary: format!(
                "Compressed {} → {}",
                src.display(),
                dest.display()
            ),
            data: serde_json::json!({
                "archive_path": dest.to_string_lossy()
            }),
        })
    }
}
```

### 2.3 The `SkillContext` rule

**Use `SkillContext` helpers. Never call `std::fs`, `std::process`, or any OS API
directly from a skill.**

`SkillContext` provides:

- `resolve_path(&str) -> Result<PathBuf>` — validates the path is within allowed scope
- `fs_read_dir`, `fs_read_file`, `fs_write_file`, `fs_create_file`, … — permission-checked wrappers
- `spawn_process(cmd, args) -> Result<Output>` — only available when `Permission::Process` is declared
- `memory_get(key)` / `memory_set(key, value)` — structured memory access
- `report_progress(msg)` — sends a progress update to the UI
- `request_confirmation(preview)` — shows the confirmation dialog; always called by the executor for `Moderate`/`Destructive` skills, but skills can call it manually for sub-actions

This constraint is enforced by code review. Process isolation (which makes it
structurally impossible to bypass) arrives in v0.3 with out-of-process skill workers.

---

## 3. Registration

Register the new skill in `crates/jarvis-skills/src/lib.rs` in the `builtin_skills()`
function, which returns `Vec<Arc<dyn Skill>>`:

```rust
pub fn builtin_skills() -> Vec<Arc<dyn jarvis_types::Skill>> {
    vec![
        Arc::new(system::open_app::OpenAppSkill::new()),
        Arc::new(files::open_folder::OpenFolderSkill::new()),
        Arc::new(files::search::SearchSkill::new()),
        Arc::new(files::convert_images::ConvertImagesSkill::new()),
        // … other existing skills …

        // Add your new skill here:
        Arc::new(files::compress_folder::CompressFolderSkill::new()),
    ]
}
```

Also add `pub mod compress_folder;` inside `crates/jarvis-skills/src/files/mod.rs`.

---

## 4. Testing requirements

Every skill must have at least two test categories.

### 4.1 Parameter validation tests

These must run without touching the filesystem or spawning processes.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_types::Skill;

    fn skill() -> CompressFolderSkill {
        CompressFolderSkill::new()
    }

    #[test]
    fn rejects_missing_required_param() {
        // params_schema requires "folder_path"
        let bad = serde_json::json!({ "overwrite": true });
        // The executor validates against the schema before calling execute(),
        // but the skill's own deserialization must also reject it.
        let result = serde_json::from_value::<Params>(bad);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_extra_fields() {
        // additionalProperties: false — extra fields must be rejected by the
        // executor's schema validation. Verify the schema declares it.
        let schema = skill().manifest().params_schema.clone();
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn accepts_valid_minimal_params() {
        let good = serde_json::json!({ "folder_path": "C:\\some\\folder" });
        let p: Params = serde_json::from_value(good).expect("should parse");
        assert_eq!(p.folder_path, "C:\\some\\folder");
        assert!(!p.overwrite);       // default
        assert!(p.output_path.is_none());
    }
}
```

### 4.2 Behavior tests with a temporary directory

Use `tempfile::TempDir` so tests are self-contained and leave no state on disk.

```rust
    #[tokio::test]
    async fn creates_archive_next_to_source() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("my_folder");
        std::fs::create_dir(&src).unwrap();
        std::fs::write(src.join("file.txt"), b"hello").unwrap();

        let ctx = SkillContext::test_context(dir.path());
        let params = serde_json::json!({ "folder_path": src.to_str().unwrap() });

        let output = skill().execute(params, &ctx).await.unwrap();

        let expected_archive = src.with_extension("zip");
        assert!(expected_archive.exists(), "archive should be created");
        assert!(output.summary.contains("my_folder.zip"));
    }

    #[tokio::test]
    async fn refuses_overwrite_without_flag() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("my_folder");
        std::fs::create_dir(&src).unwrap();
        let archive = src.with_extension("zip");
        std::fs::write(&archive, b"old").unwrap(); // pre-existing archive

        let ctx = SkillContext::test_context(dir.path());
        let params = serde_json::json!({ "folder_path": src.to_str().unwrap() });

        let err = skill().execute(params, &ctx).await.unwrap_err();
        assert!(matches!(err, SkillError::Execution(_)));
    }
```

Key rules:
- No test may require Ollama, a running GUI, or network access.
- Tests must pass in `cargo test` with no external setup.
- Always use `SkillContext::test_context(root)` (a test double that enforces the same
  path scoping as production but writes to the temp dir).

---

## 5. Out-of-process skills (v0.3+)

In v0.3, Jarvis adds a `runner` field to the manifest format:

```json
{
  "id": "files.compress_folder",
  "runner": "sidecar",
  "sidecar": { "command": "python", "entry": "skills/compress_folder/main.py" },
  ...
}
```

The manifest contract (id, params_schema, permissions, risk, examples) is identical.
The executor sees no difference: it still validates params against the schema and checks
permissions before invoking the skill. Only the dispatch layer changes.

Built-in Rust skills written today require no changes when out-of-process skills land —
they continue to implement the `Skill` trait and run in-process.

Third-party skills must wait for v0.3 isolation; there is deliberately no plugin
installation mechanism before then (see ADR-003).
