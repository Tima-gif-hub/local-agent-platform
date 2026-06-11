use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};
use url::Url;

use crate::manifest;

/// Opens an HTTP(S) URL in the default browser.
pub struct OpenUrlSkill {
    manifest: SkillManifest,
}

impl OpenUrlSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/web/open_url.json")),
        }
    }
}

impl Default for OpenUrlSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for OpenUrlSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let url = params
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("url is required".to_string()))?;
        let parsed =
            Url::parse(url).map_err(|error| SkillError::InvalidParams(error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(SkillError::InvalidParams(
                "only http and https URLs are allowed".to_string(),
            ));
        }
        #[cfg(windows)]
        ctx.spawn_program(
            "cmd",
            &[
                "/C".to_string(),
                "start".to_string(),
                "".to_string(),
                url.to_string(),
            ],
        )?;
        #[cfg(not(windows))]
        ctx.spawn_program("open", &[url.to_string()])?;
        Ok(SkillOutput {
            summary: format!("Opened {url}."),
            data: json!({ "url": url }),
        })
    }
}
