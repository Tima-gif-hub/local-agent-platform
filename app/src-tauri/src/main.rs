#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    collections::HashMap,
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use jarvis_llm::OllamaClient;
use jarvis_router::{RouteResult, Router};
use jarvis_skills::{
    builtin_skills,
    executor::{ExecutionReport, Executor, ExecutorConfig},
};
use jarvis_store::{AuditFilter, Store};
use jarvis_types::{Confirmer, InvocationPlan, Skill, SkillError, Spawner};
use serde::{Deserialize, Serialize};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tokio::sync::oneshot;

#[derive(Clone)]
struct PendingPlan {
    plan: InvocationPlan,
    conversation_id: i64,
}

struct AppState {
    store: Store,
    llm: OllamaClient,
    skills: Vec<Arc<dyn Skill>>,
    plans: Mutex<HashMap<String, PendingPlan>>,
    plan_counter: AtomicU64,
    confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    confirmation_counter: AtomicU64,
    app_handle: tauri::AppHandle,
}

#[derive(Clone)]
struct TauriConfirmer {
    app_handle: tauri::AppHandle,
    confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    counter: Arc<AtomicU64>,
}

#[async_trait]
impl Confirmer for TauriConfirmer {
    async fn confirm(&self, prompt: String) -> bool {
        let id = self.counter.fetch_add(1, Ordering::Relaxed).to_string();
        let (sender, receiver) = oneshot::channel();
        self.confirmations
            .lock()
            .expect("confirmations")
            .insert(id.clone(), sender);
        let _ = self.app_handle.emit(
            "confirmation-requested",
            ConfirmationRequest {
                id: id.clone(),
                prompt,
            },
        );
        let accepted = tokio::time::timeout(Duration::from_secs(60), receiver).await;
        self.confirmations
            .lock()
            .expect("confirmations")
            .remove(&id);
        matches!(accepted, Ok(Ok(true)))
    }
}

#[derive(Clone)]
struct CommandSpawner;

impl Spawner for CommandSpawner {
    fn spawn(&self, program: &str, args: &[String]) -> Result<(), SkillError> {
        Command::new(program)
            .args(args)
            .spawn()
            .map(|_| ())
            .map_err(|error| SkillError::Execution(error.to_string()))
    }
}

#[derive(Clone, Serialize)]
struct ConfirmationRequest {
    id: String,
    prompt: String,
}

#[derive(Serialize)]
struct PreviewDto {
    plan_id: Option<String>,
    plan: Option<InvocationPlan>,
    clarify: Option<String>,
}

#[derive(Serialize)]
struct ReportDto {
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
struct HistoryDto {
    rows: Vec<HistoryRowDto>,
}

#[derive(Serialize)]
struct HistoryRowDto {
    id: i64,
    ts: String,
    skill_id: String,
    outcome: String,
    route_source: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct ConfirmationResponse {
    id: String,
    accepted: bool,
}

#[tauri::command]
async fn route_and_preview(text: String, state: State<'_, AppState>) -> Result<PreviewDto, String> {
    let catalog = state
        .skills
        .iter()
        .map(|skill| skill.manifest().clone())
        .collect::<Vec<_>>();
    let result = Router::route(&text, &catalog, &state.llm)
        .await
        .map_err(|error| error.to_string())?;

    match result {
        RouteResult::Clarify(question) => Ok(PreviewDto {
            plan_id: None,
            plan: None,
            clarify: Some(question),
        }),
        RouteResult::Plan(plan) => {
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
            })
        }
    }
}

#[tauri::command]
async fn execute(plan_id: String, state: State<'_, AppState>) -> Result<ReportDto, String> {
    let pending = state
        .plans
        .lock()
        .expect("plans")
        .remove(&plan_id)
        .ok_or_else(|| "unknown plan_id".to_string())?;
    let confirmer = Arc::new(TauriConfirmer {
        app_handle: state.app_handle.clone(),
        confirmations: state.confirmations.clone(),
        counter: Arc::new(AtomicU64::new(
            state.confirmation_counter.fetch_add(1, Ordering::Relaxed),
        )),
    });
    let executor = Executor::new(
        state.skills.clone(),
        state.store.clone(),
        confirmer,
        Arc::new(CommandSpawner),
        Some(Arc::new(state.store.clone())),
        ExecutorConfig::default(),
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
async fn history(page: u32, state: State<'_, AppState>) -> Result<HistoryDto, String> {
    let limit = 50;
    let offset = i64::from(page) * limit;
    let rows = state
        .store
        .audit()
        .list(AuditFilter::default(), limit, offset)
        .await
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| HistoryRowDto {
            id: row.id,
            ts: row.ts,
            skill_id: row.skill_id,
            outcome: row.outcome.to_string(),
            route_source: format!("{:?}", row.route_source),
            params: row.params,
        })
        .collect();
    Ok(HistoryDto { rows })
}

#[tauri::command]
async fn respond_confirmation(
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

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let visible = window.is_visible().unwrap_or(false);

        if visible {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let store = tauri::async_runtime::block_on(Store::open_default())?;
            app.manage(AppState {
                store,
                llm: OllamaClient::new("http://localhost:11434", "qwen2.5:3b"),
                skills: builtin_skills(),
                plans: Mutex::new(HashMap::new()),
                plan_counter: AtomicU64::new(1),
                confirmations: Arc::new(Mutex::new(HashMap::new())),
                confirmation_counter: AtomicU64::new(1),
                app_handle: app.handle().clone(),
            });

            let alt_space = Shortcut::new(Some(Modifiers::ALT), Code::Space);

            app.handle().plugin(
                tauri_plugin_global_shortcut::Builder::new()
                    .with_handler(move |app, shortcut, event| {
                        if event.state() == ShortcutState::Pressed && shortcut == &alt_space {
                            toggle_main_window(app);
                        }
                    })
                    .build(),
            )?;

            app.global_shortcut()
                .register(Shortcut::new(Some(Modifiers::ALT), Code::Space))?;

            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;

            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        app.exit(0);
                    }
                })
                .build(app)?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            route_and_preview,
            execute,
            history,
            respond_confirmation
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Jarvis");
}
