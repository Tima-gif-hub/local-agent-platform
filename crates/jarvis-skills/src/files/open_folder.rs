use std::path::PathBuf;

use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};

use crate::manifest;

/// Opens a folder in the platform file manager.
pub struct OpenFolderSkill {
    manifest: SkillManifest,
}

impl OpenFolderSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/files/open_folder.json")),
        }
    }
}

impl Default for OpenFolderSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for OpenFolderSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("path is required".to_string()))?;
        let path = expand_path(path);
        if !path.is_dir() {
            return Err(SkillError::Execution(format!(
                "Folder does not exist: {}",
                path.display()
            )));
        }
        let path_arg = path.display().to_string();
        #[cfg(windows)]
        ctx.spawn_program("explorer", std::slice::from_ref(&path_arg))?;
        #[cfg(not(windows))]
        ctx.spawn_program("open", std::slice::from_ref(&path_arg))?;
        Ok(SkillOutput {
            summary: format!("Opened folder {}.", path.display()),
            data: json!({ "path": path }),
        })
    }
}

fn expand_path(path: &str) -> PathBuf {
    let mut value = path.to_string();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let home = home.to_string_lossy();
        if value == "~" {
            value = home.to_string();
        } else if let Some(rest) = value.strip_prefix("~/") {
            value = format!("{home}/{rest}");
        }
    }
    for (key, env_value) in std::env::vars() {
        value = value.replace(&format!("%{key}%"), &env_value);
        value = value.replace(&format!("${key}"), &env_value);
    }
    PathBuf::from(value)
}
