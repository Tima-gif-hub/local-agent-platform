use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};
use sysinfo::{Disks, System};

use crate::manifest;

/// Reports system information.
pub struct SystemInfoSkill {
    manifest: SkillManifest,
}

impl SystemInfoSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/system/info.json")),
        }
    }
}

impl Default for SystemInfoSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for SystemInfoSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(
        &self,
        _params: Value,
        _ctx: &SkillContext,
    ) -> Result<SkillOutput, SkillError> {
        let mut system = System::new_all();
        system.refresh_all();
        let disks = Disks::new_with_refreshed_list()
            .iter()
            .map(|disk| {
                json!({
                    "name": disk.name().to_string_lossy(),
                    "total": disk.total_space(),
                    "available": disk.available_space()
                })
            })
            .collect::<Vec<_>>();
        Ok(SkillOutput {
            summary: "Collected system information.".to_string(),
            data: json!({
                "cpus": system.cpus().len(),
                "memory_total": system.total_memory(),
                "memory_used": system.used_memory(),
                "disks": disks
            }),
        })
    }
}
