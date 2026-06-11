use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};

use crate::manifest;

/// Stores a memory fact.
pub struct RememberSkill {
    manifest: SkillManifest,
}

impl RememberSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/memory/remember.json")),
        }
    }
}

impl Default for RememberSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for RememberSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let key = required(&params, "key")?.to_string();
        let value = required(&params, "value")?.to_string();
        let memory = ctx
            .memory
            .as_ref()
            .ok_or_else(|| SkillError::Execution("memory port is unavailable".to_string()))?;
        memory.remember(key.clone(), value.clone()).await?;
        Ok(SkillOutput {
            summary: format!("Remembered {key}."),
            data: json!({ "key": key, "value": value }),
        })
    }
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, SkillError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| SkillError::InvalidParams(format!("{key} is required")))
}
