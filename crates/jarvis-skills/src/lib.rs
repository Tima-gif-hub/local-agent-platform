//! Built-in skills, one module per category.

use std::sync::Arc;

use jarvis_types::Skill;

pub mod files;
pub mod memory;
pub mod system;
pub mod web;

/// Returns all built-in MVP skills.
pub fn builtin_skills() -> Vec<Arc<dyn Skill>> {
    vec![
        Arc::new(system::open_app::OpenAppSkill::new()),
        Arc::new(files::open_folder::OpenFolderSkill::new()),
        Arc::new(files::search::SearchFilesSkill::new()),
        Arc::new(files::convert_images::ConvertImagesSkill::new()),
        Arc::new(system::info::SystemInfoSkill::new()),
        Arc::new(system::processes::SystemProcessesSkill::new()),
        Arc::new(web::open_url::OpenUrlSkill::new()),
        Arc::new(memory::remember::RememberSkill::new()),
        Arc::new(memory::recall::RecallSkill::new()),
    ]
}

fn manifest(input: &str) -> jarvis_types::SkillManifest {
    jarvis_types::SkillManifest::from_json_str(input).expect("valid built-in manifest")
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use jarvis_types::{
        validate_params, Confirmer, MemoryPort, PathScope, Permission, Skill, SkillContext,
        SkillError, Spawner,
    };
    use serde_json::json;

    use super::*;

    struct Yes;

    #[async_trait]
    impl Confirmer for Yes {
        async fn confirm(&self, _prompt: String) -> bool {
            true
        }
    }

    #[derive(Default)]
    struct RecordingSpawner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
    }

    impl Spawner for RecordingSpawner {
        fn spawn(&self, program: &str, args: &[String]) -> Result<(), SkillError> {
            self.calls
                .lock()
                .expect("calls")
                .push((program.to_string(), args.to_vec()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct Memory {
        facts: Mutex<std::collections::BTreeMap<String, String>>,
    }

    #[async_trait]
    impl MemoryPort for Memory {
        async fn remember(&self, key: String, value: String) -> Result<(), SkillError> {
            self.facts.lock().expect("facts").insert(key, value);
            Ok(())
        }

        async fn recall(&self, key: String) -> Result<Option<String>, SkillError> {
            Ok(self.facts.lock().expect("facts").get(&key).cloned())
        }
    }

    fn ctx(perms: Vec<Permission>) -> SkillContext {
        SkillContext::new(perms, Arc::new(Yes))
    }

    #[test]
    fn all_builtin_manifests_load_from_skills_directory() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
        let mut count = 0;
        for entry in walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        {
            let json = std::fs::read_to_string(entry.path()).expect("manifest json");
            jarvis_types::SkillManifest::from_json_str(&json).expect("manifest loads");
            count += 1;
        }
        assert_eq!(count, 9);
    }

    #[test]
    fn validates_sample_params_for_each_skill() {
        let samples = [
            ("system.open_app", json!({ "name": "notepad" })),
            ("files.open_folder", json!({ "path": "C:/dev" })),
            (
                "files.search",
                json!({ "root": "C:/dev", "pattern": "*.rs" }),
            ),
            (
                "files.convert_images",
                json!({ "folder": "C:/pics", "from": "png", "to": "jpg" }),
            ),
            ("system.info", json!({})),
            ("system.processes", json!({})),
            ("web.open_url", json!({ "url": "https://example.com" })),
            (
                "memory.remember",
                json!({ "key": "project.root", "value": "C:/dev" }),
            ),
            ("memory.recall", json!({ "key": "project.root" })),
        ];
        let skills = builtin_skills();
        for (id, params) in samples {
            let skill = skills
                .iter()
                .find(|skill| skill.manifest().id == id)
                .expect("skill");
            validate_params(skill.manifest(), &params).expect("params validate");
        }
    }

    #[tokio::test]
    async fn open_app_uses_injected_spawner() {
        let spawner = Arc::new(RecordingSpawner::default());
        let resolver = Arc::new(system::open_app::FakeResolver {
            path_entries: vec![(
                "notepad.exe".to_string(),
                std::path::PathBuf::from("C:/Windows/notepad.exe"),
            )],
            ..Default::default()
        });
        let ctx = SkillContext::with_spawner(
            vec![Permission::ProcessSpawn],
            Arc::new(Yes),
            spawner.clone(),
        );
        system::open_app::OpenAppSkill::with_resolver(resolver)
            .execute(json!({ "name": "notepad" }), &ctx)
            .await
            .expect("open app");
        assert_eq!(
            spawner.calls.lock().expect("calls")[0].0,
            "C:/Windows/notepad.exe"
        );
    }

    #[tokio::test]
    async fn open_folder_rejects_missing_folder_and_opens_existing() {
        let spawner = Arc::new(RecordingSpawner::default());
        let tempdir = tempfile::tempdir().expect("tempdir");
        let ctx = SkillContext::with_spawner(
            vec![Permission::ProcessSpawn],
            Arc::new(Yes),
            spawner.clone(),
        );
        files::open_folder::OpenFolderSkill::new()
            .execute(
                json!({ "path": tempdir.path().display().to_string() }),
                &ctx,
            )
            .await
            .expect("open folder");
        assert_eq!(spawner.calls.lock().expect("calls").len(), 1);

        let error = files::open_folder::OpenFolderSkill::new()
            .execute(json!({ "path": tempdir.path().join("missing") }), &ctx)
            .await
            .expect_err("missing");
        assert!(matches!(error, SkillError::Execution(_)));
    }

    #[tokio::test]
    async fn search_finds_files_in_tempdir() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        std::fs::write(tempdir.path().join("main.rs"), "fn main() {}").expect("write");
        let ctx = ctx(vec![Permission::FsRead(PathScope::Within(
            tempdir.path().to_path_buf(),
        ))]);
        let output = files::search::SearchFilesSkill::new()
            .execute(
                json!({ "root": tempdir.path().display().to_string(), "pattern": "*.rs" }),
                &ctx,
            )
            .await
            .expect("search");
        assert_eq!(output.data["results"].as_array().expect("results").len(), 1);
    }

    #[tokio::test]
    async fn convert_images_converts_temp_png() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let input = tempdir.path().join("image.png");
        image::RgbImage::new(1, 1).save(&input).expect("png");
        std::fs::write(tempdir.path().join("bad.png"), b"not an image").expect("bad image");
        let ctx = ctx(vec![
            Permission::FsRead(PathScope::Within(tempdir.path().to_path_buf())),
            Permission::FsWrite(PathScope::Within(tempdir.path().to_path_buf())),
        ]);
        let output = files::convert_images::ConvertImagesSkill::new()
            .execute(
                json!({ "folder": tempdir.path().display().to_string(), "from": "png", "to": "jpg" }),
                &ctx,
            )
            .await
            .expect("convert");
        assert_eq!(output.data["converted"], 1);
        assert_eq!(output.data["failed"], 1);
        assert!(tempdir.path().join("image.jpg").exists());
    }

    #[tokio::test]
    async fn system_skills_return_data() {
        let ctx = ctx(vec![Permission::ProcessInspect]);
        assert!(system::info::SystemInfoSkill::new()
            .execute(json!({}), &ctx)
            .await
            .expect("info")
            .data
            .get("memory_total")
            .is_some());
        assert!(system::processes::SystemProcessesSkill::new()
            .execute(json!({}), &ctx)
            .await
            .expect("processes")
            .data
            .get("top_cpu")
            .is_some());
    }

    #[tokio::test]
    async fn open_url_validates_scheme_and_uses_spawner() {
        let spawner = Arc::new(RecordingSpawner::default());
        let ctx = SkillContext::with_spawner(
            vec![Permission::ProcessSpawn],
            Arc::new(Yes),
            spawner.clone(),
        );
        web::open_url::OpenUrlSkill::new()
            .execute(json!({ "url": "https://example.com" }), &ctx)
            .await
            .expect("open url");
        assert_eq!(spawner.calls.lock().expect("calls").len(), 1);
        assert!(web::open_url::OpenUrlSkill::new()
            .execute(json!({ "url": "file:///tmp/a" }), &ctx)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn memory_skills_use_memory_port() {
        let memory = Arc::new(Memory::default());
        let ctx = ctx(vec![Permission::Memory]).with_memory(memory);
        memory::remember::RememberSkill::new()
            .execute(json!({ "key": "project.root", "value": "C:/dev" }), &ctx)
            .await
            .expect("remember");
        let output = memory::recall::RecallSkill::new()
            .execute(json!({ "key": "project.root" }), &ctx)
            .await
            .expect("recall");
        assert_eq!(output.data["value"], "C:/dev");
    }
}
