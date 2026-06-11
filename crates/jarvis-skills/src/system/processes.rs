use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};
use sysinfo::System;

use crate::manifest;

/// Lists top processes by CPU and RAM.
pub struct SystemProcessesSkill {
    manifest: SkillManifest,
}

impl SystemProcessesSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/system/processes.json")),
        }
    }
}

impl Default for SystemProcessesSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for SystemProcessesSkill {
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
        let mut processes = system
            .processes()
            .values()
            .map(|process| {
                json!({
                    "name": process.name().to_string_lossy(),
                    "pid": process.pid().as_u32(),
                    "cpu": process.cpu_usage(),
                    "memory": process.memory()
                })
            })
            .collect::<Vec<_>>();
        processes.sort_by(|a, b| {
            b["cpu"]
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&a["cpu"].as_f64().unwrap_or_default())
        });
        let top_cpu = processes.iter().take(10).cloned().collect::<Vec<_>>();
        processes
            .sort_by_key(|value| std::cmp::Reverse(value["memory"].as_u64().unwrap_or_default()));
        let top_memory = processes.iter().take(10).cloned().collect::<Vec<_>>();
        Ok(SkillOutput {
            summary: "Collected top processes by CPU and memory.".to_string(),
            data: json!({ "top_cpu": top_cpu, "top_memory": top_memory }),
        })
    }
}
