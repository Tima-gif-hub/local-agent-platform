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
use jarvis_types::{Confirmer, SkillError, Spawner};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::Emitter;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct TauriConfirmer {
    pub app_handle: tauri::AppHandle,
    pub confirmations: Arc<Mutex<HashMap<String, oneshot::Sender<bool>>>>,
    pub counter: Arc<AtomicU64>,
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
        let details = parse_confirmation_details(&prompt);
        let _ = self.app_handle.emit(
            "confirmation-requested",
            ConfirmationRequest {
                id: id.clone(),
                prompt,
                skill_id: details.skill_id,
                params: details.params,
                risk: details.risk,
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
pub struct CommandSpawner;

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
pub struct ConfirmationRequest {
    pub id: String,
    pub prompt: String,
    pub skill_id: Option<String>,
    pub params: Value,
    pub risk: Option<String>,
}

#[derive(Deserialize)]
pub struct ConfirmationResponse {
    pub id: String,
    pub accepted: bool,
}

#[derive(Deserialize)]
struct ConfirmationDetails {
    skill_id: Option<String>,
    #[serde(default)]
    params: Value,
    risk: Option<String>,
}

fn parse_confirmation_details(prompt: &str) -> ConfirmationDetails {
    serde_json::from_str(prompt).unwrap_or_else(|_| ConfirmationDetails {
        skill_id: None,
        params: Value::Object(Default::default()),
        risk: None,
    })
}
