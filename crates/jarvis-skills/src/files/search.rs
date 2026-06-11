use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};
use walkdir::WalkDir;

use crate::manifest;

/// Searches files by simple glob-like pattern.
pub struct SearchFilesSkill {
    manifest: SkillManifest,
}

impl SearchFilesSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/files/search.json")),
        }
    }
}

impl Default for SearchFilesSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for SearchFilesSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let root = params
            .get("root")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("root is required".to_string()))?;
        let pattern = params
            .get("pattern")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("pattern is required".to_string()))?;
        let root = std::path::PathBuf::from(root);
        if !root.is_dir() {
            return Err(SkillError::Execution(format!(
                "Search root does not exist: {}",
                root.display()
            )));
        }
        ctx.check_fs_read(&root)?;
        let needle = pattern.trim_matches('*').to_lowercase();
        let extension = pattern.strip_prefix("*.").map(str::to_lowercase);
        let mut results = Vec::new();
        for entry in WalkDir::new(&root)
            .max_depth(8)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0 || !is_hidden(entry.file_name().to_string_lossy().as_ref())
            })
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            let extension_matches = extension.as_ref().is_some_and(|extension| {
                entry.path().extension().and_then(|value| value.to_str()) == Some(extension)
            });
            if extension_matches || name.contains(&needle) {
                results.push(entry.path().display().to_string());
            }
            if results.len() >= 200 {
                break;
            }
        }
        Ok(SkillOutput {
            summary: format!("Found {} matching files.", results.len()),
            data: json!({ "results": results }),
        })
    }
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules" | "$RECYCLE.BIN")
}
