use std::{
    collections::HashMap,
    str::FromStr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use jarvis_llm::{pull_model, LlmClient, LlmHealth, ModelPreset, OllamaClient, MODEL_PRESETS};
use jarvis_router::{RouteResult, Router};
use jarvis_skills::{
    executor::{ExecutionReport, Executor, ExecutorConfig},
    builtin_skills,
};
use jarvis_store::{AuditFilter, AuditOutcome, Store};
use jarvis_types::{InvocationPlan, RiskLevel, Skill};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, State};
use tokio::sync::oneshot;

use crate::confirm::{CommandSpawner, ConfirmationResponse, TauriConfirmer};

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const SETTING_LANGUAGE: &str = "language";
const SETTING_CONFIRM_THRESHOLD: &str = "confirm_threshold";
const SETTING_AUTO_RUN_SAFE: &str = "auto_run_safe";
const SETTING_MODEL_PRESET: &str = "model_preset";
const SETTING_ONBOARDING_DONE: &str = "onboarding_done";

#[derive(Clone)]
struct PendingPlan {
    plan: InvocationPlan,
    conversation_id: i64,
}

pub struct AppState {
    pub store: Store,
    pub skills: Vec<Arc<dyn Skill>>,
    plans: Mutex<HashMap<String, PendingPlan>>,
    plan_counter: AtomicU64,
    pub confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    confirmation_counter: Arc<AtomicU64>,
    pub app_handle: tauri::AppHandle,
}

impl AppState {
    pub fn new(store: Store, app_handle: tauri::AppHandle) -> Self {
        Self {
            store,
            skills: builtin_skills(),
            plans: Mutex::new(HashMap::new()),
            plan_counter: AtomicU64::new(1),
            confirmations: Arc::new(Mutex::new(HashMap::new())),
            confirmation_counter: Arc::new(AtomicU64::new(1)),
            app_handle,
        }
    }
}

#[derive(Serialize)]
pub struct PreviewDto {
    plan_id: Option<String>,
    plan: Option<InvocationPlan>,
    clarify: Option<String>,
    risk: Option<String>,
}

#[derive(Serialize)]
pub struct ReportDto {
    success: bool,
    summary: String,
    report: ExecutionReportDto,
}

#[derive(Serialize)]
struct ExecutionReportDto {
    steps: Vec<StepReportDto>,
}

#[derive(Serialize)]
struct StepReportDto {
    skill_id: String,
    risk: String,
    outcome: String,
    error: Option<String>,
    output: Option<serde_json::Value>,
}

impl From<ExecutionReport> for ReportDto {
    fn from(report: ExecutionReport) -> Self {
        Self {
            success: report.success,
            summary: report.summary.clone(),
            report: ExecutionReportDto {
                steps: report
                    .steps
                    .into_iter()
                    .map(|step| StepReportDto {
                        skill_id: step.skill_id,
                        risk: risk_to_text(step.risk).to_string(),
                        outcome: step.outcome.to_string(),
                        error: step.error,
                        output: step.output.map(|output| output.data),
                    })
                    .collect(),
            },
        }
    }
}

#[derive(Serialize)]
pub struct HistoryDto {
    rows: Vec<HistoryRowDto>,
}

#[derive(Serialize)]
struct HistoryRowDto {
    id: i64,
    ts: String,
    skill_id: String,
    risk: String,
    outcome: String,
    route_source: String,
    params: serde_json::Value,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct SettingsDto {
    language: String,
    confirm_threshold: String,
    auto_run_safe: bool,
    model_preset: String,
    onboarding_done: bool,
}

#[derive(Serialize)]
pub struct OllamaStatusDto {
    status: String,
    model: String,
}

#[derive(Clone, Serialize)]
struct PullProgressDto {
    status: String,
    digest: Option<String>,
    completed: Option<u64>,
    total: Option<u64>,
}

#[tauri::command]
pub async fn route_and_preview(
    text: String,
    state: State<'_, AppState>,
) -> Result<PreviewDto, String> {
    let catalog = state
        .skills
        .iter()
        .map(|skill| skill.manifest().clone())
        .collect::<Vec<_>>();
    let settings = load_settings(&state.store).await?;
    let llm = OllamaClient::new(OLLAMA_BASE_URL, model_tag(&settings.model_preset));
    let result = Router::route(&text, &catalog, &llm)
        .await
        .map_err(|error| error.to_string())?;

    match result {
        RouteResult::Clarify(question) => Ok(PreviewDto {
            plan_id: None,
            plan: None,
            clarify: Some(question),
            risk: None,
        }),
        RouteResult::Plan(plan) => {
            let risk = plan
                .steps
                .first()
                .and_then(|step| catalog.iter().find(|manifest| manifest.id == step.skill_id))
                .map(|manifest| risk_to_text(manifest.risk).to_string());
            let conversation_id = state
                .store
                .conversations()
                .create()
                .await
                .map_err(|error| error.to_string())?;
            state
                .store
                .conversations()
                .add_message(conversation_id, "user", &text)
                .await
                .map_err(|error| error.to_string())?;
            let plan_id = state
                .plan_counter
                .fetch_add(1, Ordering::Relaxed)
                .to_string();
            state.plans.lock().expect("plans").insert(
                plan_id.clone(),
                PendingPlan {
                    plan: plan.clone(),
                    conversation_id,
                },
            );
            Ok(PreviewDto {
                plan_id: Some(plan_id),
                plan: Some(plan),
                clarify: None,
                risk,
            })
        }
    }
}

#[tauri::command]
pub async fn execute(plan_id: String, state: State<'_, AppState>) -> Result<ReportDto, String> {
    let pending = state
        .plans
        .lock()
        .expect("plans")
        .remove(&plan_id)
        .ok_or_else(|| "unknown plan_id".to_string())?;
    let settings = load_settings(&state.store).await?;
    let confirmer = Arc::new(TauriConfirmer {
        app_handle: state.app_handle.clone(),
        confirmations: state.confirmations.clone(),
        counter: state.confirmation_counter.clone(),
    });
    let executor = Executor::new(
        state.skills.clone(),
        state.store.clone(),
        confirmer,
        Arc::new(CommandSpawner),
        Some(Arc::new(state.store.clone())),
        ExecutorConfig {
            confirm_threshold: parse_risk(&settings.confirm_threshold),
            grants: Vec::new(),
        },
    );
    let report = executor.run(pending.plan).await;
    let summary = if report.summary.is_empty() {
        "No action completed.".to_string()
    } else {
        report.summary.clone()
    };
    state
        .store
        .conversations()
        .add_message(pending.conversation_id, "assistant", &summary)
        .await
        .map_err(|error| error.to_string())?;
    Ok(report.into())
}

#[tauri::command]
pub async fn history(
    page: u32,
    outcome: Option<String>,
    state: State<'_, AppState>,
) -> Result<HistoryDto, String> {
    let limit = 50;
    let offset = i64::from(page) * limit;
    let filter = AuditFilter {
        skill_id: None,
        outcome: outcome
            .as_deref()
            .filter(|value| !value.is_empty())
            .map(AuditOutcome::from_str)
            .transpose()
            .map_err(|error| error.to_string())?,
    };
    let rows = state
        .store
        .audit()
        .list(filter, limit, offset)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| HistoryRowDto {
            id: row.id,
            ts: row.ts,
            skill_id: row.skill_id,
            risk: risk_to_text(row.risk).to_string(),
            outcome: row.outcome.to_string(),
            route_source: format!("{:?}", row.route_source),
            params: row.params,
        })
        .collect();
    Ok(HistoryDto { rows })
}

#[tauri::command]
pub async fn respond_confirmation(
    response: ConfirmationResponse,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if let Some(sender) = state
        .confirmations
        .lock()
        .expect("confirmations")
        .remove(&response.id)
    {
        let _ = sender.send(response.accepted);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<SettingsDto, String> {
    load_settings(&state.store).await
}

#[tauri::command]
pub async fn save_settings(
    settings: SettingsDto,
    state: State<'_, AppState>,
) -> Result<SettingsDto, String> {
    let normalized = normalize_settings(settings);
    let repo = state.store.settings();
    repo.set(SETTING_LANGUAGE, &normalized.language)
        .await
        .map_err(|error| error.to_string())?;
    repo.set(SETTING_CONFIRM_THRESHOLD, &normalized.confirm_threshold)
        .await
        .map_err(|error| error.to_string())?;
    repo.set(
        SETTING_AUTO_RUN_SAFE,
        if normalized.auto_run_safe { "true" } else { "false" },
    )
    .await
    .map_err(|error| error.to_string())?;
    repo.set(SETTING_MODEL_PRESET, &normalized.model_preset)
        .await
        .map_err(|error| error.to_string())?;
    repo.set(
        SETTING_ONBOARDING_DONE,
        if normalized.onboarding_done { "true" } else { "false" },
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(normalized)
}

#[tauri::command]
pub async fn ollama_status(state: State<'_, AppState>) -> Result<OllamaStatusDto, String> {
    let settings = load_settings(&state.store).await?;
    let tag = model_tag(&settings.model_preset);
    let health = OllamaClient::new(OLLAMA_BASE_URL, tag).health().await;
    let status = match health {
        LlmHealth::Available { .. } => "available",
        LlmHealth::ModelMissing => "model_missing",
        LlmHealth::Down => "down",
    };
    Ok(OllamaStatusDto {
        status: status.to_string(),
        model: tag.to_string(),
    })
}

#[tauri::command]
pub async fn pull_selected_model(state: State<'_, AppState>) -> Result<(), String> {
    let settings = load_settings(&state.store).await?;
    let tag = model_tag(&settings.model_preset).to_string();
    let app = state.app_handle.clone();
    pull_model(OLLAMA_BASE_URL, &tag, move |progress| {
        let _ = app.emit(
            "pull-progress",
            PullProgressDto {
                status: progress.status,
                digest: progress.digest,
                completed: progress.completed,
                total: progress.total,
            },
        );
    })
    .await
    .map_err(|error| error.to_string())
}

async fn load_settings(store: &Store) -> Result<SettingsDto, String> {
    let repo = store.settings();
    let settings = SettingsDto {
        language: repo
            .get(SETTING_LANGUAGE)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| "en".to_string()),
        confirm_threshold: repo
            .get(SETTING_CONFIRM_THRESHOLD)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| "moderate".to_string()),
        auto_run_safe: repo
            .get(SETTING_AUTO_RUN_SAFE)
            .await
            .map_err(|error| error.to_string())?
            .as_deref()
            == Some("true"),
        model_preset: repo
            .get(SETTING_MODEL_PRESET)
            .await
            .map_err(|error| error.to_string())?
            .unwrap_or_else(|| "fast".to_string()),
        onboarding_done: repo
            .get(SETTING_ONBOARDING_DONE)
            .await
            .map_err(|error| error.to_string())?
            .as_deref()
            == Some("true"),
    };
    Ok(normalize_settings(settings))
}

fn normalize_settings(settings: SettingsDto) -> SettingsDto {
    SettingsDto {
        language: match settings.language.as_str() {
            "ru" => "ru".to_string(),
            _ => "en".to_string(),
        },
        confirm_threshold: match settings.confirm_threshold.as_str() {
            "safe" => "safe".to_string(),
            "destructive" => "destructive".to_string(),
            _ => "moderate".to_string(),
        },
        auto_run_safe: settings.auto_run_safe,
        model_preset: match settings.model_preset.as_str() {
            "balanced" => "balanced".to_string(),
            "capable" => "capable".to_string(),
            _ => "fast".to_string(),
        },
        onboarding_done: settings.onboarding_done,
    }
}

fn parse_risk(value: &str) -> RiskLevel {
    match value {
        "safe" => RiskLevel::Safe,
        "destructive" => RiskLevel::Destructive,
        _ => RiskLevel::Moderate,
    }
}

fn risk_to_text(risk: RiskLevel) -> &'static str {
    match risk {
        RiskLevel::Safe => "safe",
        RiskLevel::Moderate => "moderate",
        RiskLevel::Destructive => "destructive",
    }
}

fn model_tag(preset: &str) -> &'static str {
    let preset = match preset {
        "balanced" => ModelPreset::Balanced,
        "capable" => ModelPreset::Capable,
        _ => ModelPreset::Fast,
    };
    MODEL_PRESETS
        .iter()
        .find(|entry| entry.preset == preset)
        .map(|entry| entry.tag)
        .unwrap_or("qwen2.5:3b")
}
