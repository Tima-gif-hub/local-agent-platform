use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};

use crate::manifest;

/// Converts images in a folder without deleting originals.
pub struct ConvertImagesSkill {
    manifest: SkillManifest,
}

impl ConvertImagesSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/files/convert_images.json")),
        }
    }
}

impl Default for ConvertImagesSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for ConvertImagesSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let folder = required(&params, "folder")?;
        let from = normalize_ext(required(&params, "from")?);
        let to = normalize_ext(required(&params, "to")?);
        let folder = std::path::PathBuf::from(folder);
        if !folder.is_dir() {
            return Err(SkillError::Execution(format!(
                "Folder does not exist: {}",
                folder.display()
            )));
        }
        ctx.check_fs_read(&folder)?;
        ctx.check_fs_write(&folder)?;

        let mut converted = 0usize;
        let mut skipped = 0usize;
        let mut failed = 0usize;
        for entry in
            std::fs::read_dir(&folder).map_err(|error| SkillError::Execution(error.to_string()))?
        {
            let entry = entry.map_err(|error| SkillError::Execution(error.to_string()))?;
            let path = entry.path();
            if !path.is_file()
                || path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(normalize_ext)
                    != Some(from.clone())
            {
                skipped += 1;
                continue;
            }
            let Ok(image) = image::open(&path) else {
                failed += 1;
                continue;
            };
            let output = path.with_extension(&to);
            if image.save(&output).is_err() {
                failed += 1;
                continue;
            }
            converted += 1;
        }
        Ok(SkillOutput {
            summary: format!(
                "Converted {converted} images, skipped {skipped}, and failed {failed}."
            ),
            data: json!({ "converted": converted, "skipped": skipped, "failed": failed }),
        })
    }
}

fn required<'a>(params: &'a Value, key: &str) -> Result<&'a str, SkillError> {
    params
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| SkillError::InvalidParams(format!("{key} is required")))
}

fn normalize_ext(value: &str) -> String {
    match value.trim().trim_start_matches('.').to_lowercase().as_str() {
        "jpeg" => "jpg".to_string(),
        other => other.to_string(),
    }
}
