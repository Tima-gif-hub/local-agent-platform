use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};

use crate::manifest;

/// Recalls a memory fact.
pub struct RecallSkill {
    manifest: SkillManifest,
}

impl RecallSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/memory/recall.json")),
        }
    }
}

impl Default for RecallSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for RecallSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let key = params
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("key is required".to_string()))?;
        let memory = ctx
            .memory
            .as_ref()
            .ok_or_else(|| SkillError::Execution("memory port is unavailable".to_string()))?;
        let value = memory.recall(key.to_string()).await?;
        Ok(SkillOutput {
            summary: format!("Recalled {key}."),
            data: json!({ "key": key, "value": value }),
        })
    }
}
