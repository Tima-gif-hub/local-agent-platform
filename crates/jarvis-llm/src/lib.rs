//! LlmClient trait and Ollama implementation, with cloud implementations later.

use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use thiserror::Error;

/// Result type used by LLM clients.
pub type LlmResult<T> = Result<T, LlmError>;

/// LLM access abstraction.
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Completes a request and returns JSON that validates against `schema`.
    async fn complete_json(&self, system: &str, user: &str, schema: &Value) -> LlmResult<Value>;

    /// Completes a request and returns plain text.
    async fn complete_text(&self, system: &str, user: &str) -> LlmResult<String>;

    /// Checks whether the configured model is available.
    async fn health(&self) -> LlmHealth;
}

/// LLM health state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlmHealth {
    /// The model is available.
    Available {
        /// Available model tag.
        model: String,
    },
    /// The service is up, but the configured model is missing.
    ModelMissing,
    /// The service is unreachable or unhealthy.
    Down,
}

/// Errors returned by LLM clients.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum LlmError {
    /// HTTP request failed.
    #[error("http error: {0}")]
    Http(String),
    /// The service returned an unexpected response shape.
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    /// The model output was not valid JSON or did not match the schema.
    #[error("invalid output: {0}")]
    InvalidOutput(String),
}

impl From<reqwest::Error> for LlmError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.to_string())
    }
}

/// Ollama-backed LLM client.
#[derive(Clone)]
pub struct OllamaClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaClient {
    /// Creates an Ollama client for a base URL and model tag.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("failed to build reqwest client"),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
        }
    }

    async fn chat(&self, system: &str, user: &str, json_format: bool) -> LlmResult<String> {
        let mut body = json!({
            "model": self.model,
            "stream": false,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ]
        });

        if json_format {
            body["format"] = json!("json");
        }

        let response: ChatResponse = self
            .client
            .post(format!("{}/api/chat", self.base_url))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(response.message.content)
    }

    async fn complete_json_once(
        &self,
        system: &str,
        user: &str,
        schema: &Value,
    ) -> LlmResult<Value> {
        let content = self.chat(system, user, true).await?;
        let value = serde_json::from_str::<Value>(&content)
            .map_err(|error| LlmError::InvalidOutput(error.to_string()))?;
        validate_output(schema, &value)?;
        Ok(value)
    }
}

#[async_trait]
impl LlmClient for OllamaClient {
    async fn complete_json(&self, system: &str, user: &str, schema: &Value) -> LlmResult<Value> {
        match self.complete_json_once(system, user, schema).await {
            Ok(value) => Ok(value),
            Err(first_error @ LlmError::InvalidOutput(_)) => {
                let retry_user = format!(
                    "{user}\n\nThe previous response was invalid: {first_error}. Return only JSON that matches the schema."
                );
                self.complete_json_once(system, &retry_user, schema)
                    .await
                    .map_err(|error| match error {
                        LlmError::InvalidOutput(message) => LlmError::InvalidOutput(message),
                        other => other,
                    })
            }
            Err(error) => Err(error),
        }
    }

    async fn complete_text(&self, system: &str, user: &str) -> LlmResult<String> {
        self.chat(system, user, false).await
    }

    async fn health(&self) -> LlmHealth {
        let response = self
            .client
            .get(format!("{}/api/tags", self.base_url))
            .timeout(Duration::from_secs(2))
            .send()
            .await;

        let Ok(response) = response else {
            return LlmHealth::Down;
        };

        if !response.status().is_success() {
            return LlmHealth::Down;
        }

        let Ok(tags) = response.json::<TagsResponse>().await else {
            return LlmHealth::Down;
        };

        if tags.models.iter().any(|model| model.name == self.model) {
            LlmHealth::Available {
                model: self.model.clone(),
            }
        } else {
            LlmHealth::ModelMissing
        }
    }
}

/// Model size preset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPreset {
    /// Fastest small routing model.
    Fast,
    /// Balanced default routing model.
    Balanced,
    /// Larger local model for harder routing.
    Capable,
}

/// Model preset entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPresetEntry {
    /// Preset label.
    pub preset: ModelPreset,
    /// Ollama model tag.
    pub tag: &'static str,
}

/// Current Ollama instruct-model choices: 3B-class for fast routing, 8B-class for
/// the default balance, and 14B-class for more capable local routing.
pub const MODEL_PRESETS: &[ModelPresetEntry] = &[
    ModelPresetEntry {
        preset: ModelPreset::Fast,
        tag: "qwen2.5:3b",
    },
    ModelPresetEntry {
        preset: ModelPreset::Balanced,
        tag: "llama3.1:8b",
    },
    ModelPresetEntry {
        preset: ModelPreset::Capable,
        tag: "qwen2.5:14b",
    },
];

/// Progress update emitted while pulling a model.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub struct PullProgress {
    /// Pull status text.
    pub status: String,
    /// Optional digest from Ollama.
    pub digest: Option<String>,
    /// Completed bytes, if reported.
    pub completed: Option<u64>,
    /// Total bytes, if reported.
    pub total: Option<u64>,
}

/// Pulls an Ollama model and reports streaming progress.
pub async fn pull_model<F>(base_url: &str, tag: &str, mut progress_callback: F) -> LlmResult<()>
where
    F: FnMut(PullProgress) + Send,
{
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/api/pull", base_url.trim_end_matches('/')))
        .json(&json!({
            "name": tag,
            "stream": true
        }))
        .send()
        .await?
        .error_for_status()?;

    let mut stream = response.bytes_stream();
    let mut buffer = Vec::new();

    while let Some(chunk) = stream.next().await {
        buffer.extend_from_slice(&chunk?);

        while let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let line = buffer.drain(..=newline).collect::<Vec<_>>();
            emit_progress_line(&line, &mut progress_callback)?;
        }
    }

    if !buffer.is_empty() {
        emit_progress_line(&buffer, &mut progress_callback)?;
    }

    Ok(())
}

/// Mock LLM client for deterministic tests in downstream crates.
#[cfg(feature = "mock")]
#[derive(Clone, Debug)]
pub struct MockLlm {
    responses: Vec<(String, MockResponse)>,
    health: LlmHealth,
}

/// Mock response returned by `MockLlm`.
#[cfg(feature = "mock")]
#[derive(Clone, Debug)]
pub enum MockResponse {
    /// JSON response.
    Json(Value),
    /// Text response.
    Text(String),
    /// Error response.
    Error(LlmError),
}

#[cfg(feature = "mock")]
impl MockLlm {
    /// Creates a mock LLM with canned matcher/response pairs.
    pub fn new(responses: Vec<(String, MockResponse)>) -> Self {
        Self {
            responses,
            health: LlmHealth::Available {
                model: "mock".to_string(),
            },
        }
    }

    /// Sets the health state returned by the mock.
    pub fn with_health(mut self, health: LlmHealth) -> Self {
        self.health = health;
        self
    }

    fn find_response(&self, system: &str, user: &str) -> LlmResult<&MockResponse> {
        self.responses
            .iter()
            .find(|(matcher, _)| system.contains(matcher) || user.contains(matcher))
            .map(|(_, response)| response)
            .ok_or_else(|| LlmError::InvalidResponse("no mock response matched".to_string()))
    }
}

#[cfg(feature = "mock")]
#[async_trait]
impl LlmClient for MockLlm {
    async fn complete_json(&self, system: &str, user: &str, schema: &Value) -> LlmResult<Value> {
        match self.find_response(system, user)? {
            MockResponse::Json(value) => {
                validate_output(schema, value)?;
                Ok(value.clone())
            }
            MockResponse::Text(text) => serde_json::from_str(text)
                .map_err(|error| LlmError::InvalidOutput(error.to_string())),
            MockResponse::Error(error) => Err(error.clone()),
        }
    }

    async fn complete_text(&self, system: &str, user: &str) -> LlmResult<String> {
        match self.find_response(system, user)? {
            MockResponse::Json(value) => Ok(value.to_string()),
            MockResponse::Text(text) => Ok(text.clone()),
            MockResponse::Error(error) => Err(error.clone()),
        }
    }

    async fn health(&self) -> LlmHealth {
        self.health.clone()
    }
}

fn validate_output(schema: &Value, value: &Value) -> LlmResult<()> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| LlmError::InvalidOutput(error.to_string()))?;

    if validator.is_valid(value) {
        Ok(())
    } else {
        let message = validator
            .iter_errors(value)
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        Err(LlmError::InvalidOutput(message))
    }
}

fn emit_progress_line<F>(line: &[u8], progress_callback: &mut F) -> LlmResult<()>
where
    F: FnMut(PullProgress),
{
    let trimmed = String::from_utf8_lossy(line).trim().to_string();
    if trimmed.is_empty() {
        return Ok(());
    }
    let progress = serde_json::from_str::<PullProgress>(&trimmed)
        .map_err(|error| LlmError::InvalidResponse(error.to_string()))?;
    progress_callback(progress);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<TagModel>,
}

#[derive(Debug, Deserialize)]
struct TagModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{body_json, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill_id": { "type": "string" }
            },
            "required": ["skill_id"],
            "additionalProperties": false
        })
    }

    #[tokio::test]
    async fn complete_json_returns_validated_json() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {
                    "content": "{\"skill_id\":\"system.open_app\"}"
                }
            })))
            .mount(&server)
            .await;
        let client = OllamaClient::new(server.uri(), "qwen2.5:3b");

        let value = client
            .complete_json("route", "open chrome", &schema())
            .await
            .expect("valid json");

        assert_eq!(value, json!({ "skill_id": "system.open_app" }));
    }

    #[tokio::test]
    async fn invalid_json_retries_once_then_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {
                    "content": "not json"
                }
            })))
            .expect(2)
            .mount(&server)
            .await;
        let client = OllamaClient::new(server.uri(), "qwen2.5:3b");

        let error = client
            .complete_json("route", "open chrome", &schema())
            .await
            .expect_err("invalid output");

        assert!(matches!(error, LlmError::InvalidOutput(_)));
    }

    #[tokio::test]
    async fn health_reports_available_missing_and_down() {
        let available = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "qwen2.5:3b" }
                ]
            })))
            .mount(&available)
            .await;
        assert_eq!(
            OllamaClient::new(available.uri(), "qwen2.5:3b")
                .health()
                .await,
            LlmHealth::Available {
                model: "qwen2.5:3b".to_string()
            }
        );

        let missing = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "models": [
                    { "name": "llama3.1:8b" }
                ]
            })))
            .mount(&missing)
            .await;
        assert_eq!(
            OllamaClient::new(missing.uri(), "qwen2.5:3b")
                .health()
                .await,
            LlmHealth::ModelMissing
        );

        let down = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/tags"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&down)
            .await;
        assert_eq!(
            OllamaClient::new(down.uri(), "qwen2.5:3b").health().await,
            LlmHealth::Down
        );
    }

    #[tokio::test]
    async fn complete_text_returns_message_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "message": {
                    "content": "hello"
                }
            })))
            .mount(&server)
            .await;
        let client = OllamaClient::new(server.uri(), "qwen2.5:3b");

        assert_eq!(
            client.complete_text("system", "user").await.expect("text"),
            "hello"
        );
    }

    #[tokio::test]
    async fn pull_model_reports_progress() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/pull"))
            .and(body_json(json!({
                "name": "qwen2.5:3b",
                "stream": true
            })))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                "{\"status\":\"pulling\",\"digest\":\"abc\",\"completed\":1,\"total\":2}\n\
                 {\"status\":\"success\"}\n",
            ))
            .mount(&server)
            .await;

        let mut progress = Vec::new();
        pull_model(&server.uri(), "qwen2.5:3b", |update| progress.push(update))
            .await
            .expect("pull");

        assert_eq!(progress.len(), 2);
        assert_eq!(progress[0].status, "pulling");
        assert_eq!(progress[1].status, "success");
    }
}
