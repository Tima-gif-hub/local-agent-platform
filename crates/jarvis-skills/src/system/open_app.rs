use std::{
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use jarvis_types::{Skill, SkillContext, SkillError, SkillManifest, SkillOutput};
use serde_json::{json, Value};
use strsim::jaro_winkler;
use walkdir::WalkDir;

use crate::manifest;

const FUZZY_THRESHOLD: f64 = 0.84;

/// Opens an installed application.
pub struct OpenAppSkill {
    manifest: SkillManifest,
    resolver: Arc<dyn AppResolver>,
}

impl OpenAppSkill {
    /// Creates the skill.
    pub fn new() -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/system/open_app.json")),
            resolver: Arc::new(SystemAppResolver),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_resolver(resolver: Arc<dyn AppResolver>) -> Self {
        Self {
            manifest: manifest(include_str!("../../../../skills/system/open_app.json")),
            resolver,
        }
    }
}

impl Default for OpenAppSkill {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Skill for OpenAppSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn execute(&self, params: Value, ctx: &SkillContext) -> Result<SkillOutput, SkillError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| SkillError::InvalidParams("name is required".to_string()))?;
        let target = resolve_app(name, self.resolver.as_ref())?;
        ctx.spawn_program(&target.program, &target.args)?;
        Ok(SkillOutput {
            summary: format!("Opened {}.", target.display_name),
            data: json!({
                "program": target.program,
                "args": target.args,
                "display_name": target.display_name
            }),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppTarget {
    display_name: String,
    program: String,
    args: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StartMenuEntry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
}

pub(crate) trait AppResolver: Send + Sync {
    fn start_menu_entries(&self) -> Vec<StartMenuEntry>;
    fn app_path(&self, executable_name: &str) -> Option<PathBuf>;
    fn path_lookup(&self, executable_name: &str) -> Option<PathBuf>;
}

struct SystemAppResolver;

impl AppResolver for SystemAppResolver {
    fn start_menu_entries(&self) -> Vec<StartMenuEntry> {
        start_menu_dirs()
            .into_iter()
            .flat_map(|dir| {
                WalkDir::new(dir)
                    .max_depth(8)
                    .into_iter()
                    .filter_map(Result::ok)
                    .filter(|entry| {
                        entry.path().extension().and_then(|ext| ext.to_str()) == Some("lnk")
                    })
                    .filter_map(|entry| {
                        let path = entry.into_path();
                        let name = path.file_stem()?.to_string_lossy().to_string();
                        Some(StartMenuEntry { name, path })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn app_path(&self, executable_name: &str) -> Option<PathBuf> {
        app_path_registry_lookup(executable_name)
    }

    fn path_lookup(&self, executable_name: &str) -> Option<PathBuf> {
        path_lookup(executable_name)
    }
}

fn resolve_app(name: &str, resolver: &dyn AppResolver) -> Result<AppTarget, SkillError> {
    let query = normalize_alias(name);
    if query.is_empty() {
        return Err(SkillError::Execution(
            "Could not resolve an empty application name.".to_string(),
        ));
    }

    let entries = resolver.start_menu_entries();
    if let Some(entry) = best_start_menu_match(&query, &entries) {
        return Ok(AppTarget {
            display_name: entry.name.clone(),
            program: "explorer.exe".to_string(),
            args: vec![entry.path.display().to_string()],
        });
    }

    for executable in executable_candidates(&query) {
        if let Some(path) = resolver.app_path(&executable) {
            return Ok(AppTarget {
                display_name: executable.clone(),
                program: path.display().to_string(),
                args: Vec::new(),
            });
        }
    }

    for executable in executable_candidates(&query) {
        if let Some(path) = resolver.path_lookup(&executable) {
            return Ok(AppTarget {
                display_name: executable.clone(),
                program: path.display().to_string(),
                args: Vec::new(),
            });
        }
    }

    let suggestions = suggestions(&query, &entries);
    Err(SkillError::Execution(format!(
        "Could not resolve application '{name}'. suggestions={}",
        serde_json::to_string(&suggestions).unwrap_or_else(|_| "[]".to_string())
    )))
}

fn best_start_menu_match<'a>(
    query: &str,
    entries: &'a [StartMenuEntry],
) -> Option<&'a StartMenuEntry> {
    let normalized_query = normalize(query);
    entries
        .iter()
        .filter_map(|entry| {
            let normalized_name = normalize(&entry.name);
            let score = if normalized_name == normalized_query {
                1.0
            } else {
                jaro_winkler(&normalized_query, &normalized_name)
            };
            (score >= FUZZY_THRESHOLD).then_some((entry, score))
        })
        .max_by(|(_, left), (_, right)| left.total_cmp(right))
        .map(|(entry, _)| entry)
}

fn suggestions(query: &str, entries: &[StartMenuEntry]) -> Vec<String> {
    let normalized_query = normalize(query);
    let mut scored = entries
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                jaro_winkler(&normalized_query, &normalize(&entry.name)),
            )
        })
        .collect::<Vec<_>>();
    scored.sort_by(|(_, left), (_, right)| right.total_cmp(left));
    scored.into_iter().take(5).map(|(name, _)| name).collect()
}

fn normalize_alias(name: &str) -> String {
    match normalize(name).as_str() {
        "вс код" | "vs code" | "vscode" => "visual studio code".to_string(),
        "хром" | "chrome" => "google chrome".to_string(),
        "блокнот" => "notepad".to_string(),
        other => other.to_string(),
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

fn executable_candidates(query: &str) -> Vec<String> {
    let base = query
        .trim()
        .trim_end_matches(".exe")
        .replace(' ', "")
        .to_lowercase();
    let mut candidates = vec![format!("{base}.exe")];
    if base == "googlechrome" {
        candidates.push("chrome.exe".to_string());
    }
    if base == "visualstudiocode" {
        candidates.push("code.exe".to_string());
    }
    candidates
}

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(appdata) = env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs"));
    }
    if let Some(program_data) = env::var_os("ProgramData") {
        dirs.push(PathBuf::from(program_data).join("Microsoft/Windows/Start Menu/Programs"));
    }
    dirs
}

#[cfg(windows)]
fn app_path_registry_lookup(executable_name: &str) -> Option<PathBuf> {
    use winreg::{enums::*, RegKey};

    let subkey = format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{}",
        executable_name
    );
    for hive in [
        RegKey::predef(HKEY_CURRENT_USER),
        RegKey::predef(HKEY_LOCAL_MACHINE),
    ] {
        if let Ok(key) = hive.open_subkey(&subkey) {
            if let Ok(value) = key.get_value::<String, _>("") {
                return Some(PathBuf::from(value));
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn app_path_registry_lookup(_executable_name: &str) -> Option<PathBuf> {
    None
}

fn path_lookup(executable_name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.BAT;.CMD".to_string());
    let mut names = vec![executable_name.to_string()];
    if Path::new(executable_name).extension().is_none() {
        names.extend(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(|ext| format!("{executable_name}{ext}")),
        );
    }

    env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeResolver {
    pub(crate) start_menu: Vec<StartMenuEntry>,
    pub(crate) app_paths: Vec<(String, PathBuf)>,
    pub(crate) path_entries: Vec<(String, PathBuf)>,
}

#[cfg(test)]
impl AppResolver for FakeResolver {
    fn start_menu_entries(&self) -> Vec<StartMenuEntry> {
        self.start_menu.clone()
    }

    fn app_path(&self, executable_name: &str) -> Option<PathBuf> {
        self.app_paths
            .iter()
            .find(|(name, _)| name == executable_name)
            .map(|(_, path)| path.clone())
    }

    fn path_lookup(&self, executable_name: &str) -> Option<PathBuf> {
        self.path_entries
            .iter()
            .find(|(name, _)| name == executable_name)
            .map(|(_, path)| path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_fuzzy_start_menu_link() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let lnk = tempdir.path().join("Visual Studio Code.lnk");
        std::fs::write(&lnk, b"fake").expect("lnk");
        let resolver = FakeResolver {
            start_menu: vec![StartMenuEntry {
                name: "Visual Studio Code".to_string(),
                path: lnk.clone(),
            }],
            ..Default::default()
        };

        let target = resolve_app("вс код", &resolver).expect("target");

        assert_eq!(target.program, "explorer.exe");
        assert_eq!(target.args, vec![lnk.display().to_string()]);
        assert_eq!(target.display_name, "Visual Studio Code");
    }

    #[test]
    fn resolves_registry_app_path_before_path_lookup() {
        let resolver = FakeResolver {
            app_paths: vec![(
                "chrome.exe".to_string(),
                PathBuf::from("C:/Chrome/Application/chrome.exe"),
            )],
            path_entries: vec![("chrome.exe".to_string(), PathBuf::from("C:/bin/chrome.exe"))],
            ..Default::default()
        };

        let target = resolve_app("chrome", &resolver).expect("target");

        assert_eq!(target.program, "C:/Chrome/Application/chrome.exe");
    }

    #[test]
    fn resolves_path_lookup() {
        let resolver = FakeResolver {
            path_entries: vec![(
                "notepad.exe".to_string(),
                PathBuf::from("C:/Windows/notepad.exe"),
            )],
            ..Default::default()
        };

        let target = resolve_app("notepad", &resolver).expect("target");

        assert_eq!(target.program, "C:/Windows/notepad.exe");
    }

    #[test]
    fn unresolved_app_returns_suggestions() {
        let resolver = FakeResolver {
            start_menu: vec![StartMenuEntry {
                name: "Visual Studio Code".to_string(),
                path: PathBuf::from("Code.lnk"),
            }],
            ..Default::default()
        };

        let error = resolve_app("totally unrelated app", &resolver).expect_err("missing");
        let SkillError::Execution(message) = error else {
            panic!("expected execution error");
        };

        assert!(message.contains("Visual Studio Code"));
    }

    #[test]
    fn skill_can_use_injected_resolver() {
        let resolver = Arc::new(FakeResolver {
            path_entries: vec![(
                "notepad.exe".to_string(),
                PathBuf::from("C:/Windows/notepad.exe"),
            )],
            ..Default::default()
        });

        let skill = OpenAppSkill::with_resolver(resolver);

        assert_eq!(skill.manifest().id, "system.open_app");
    }
}
