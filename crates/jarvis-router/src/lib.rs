//! Rules, fuzzy matching, and LLM routing.

use jarvis_llm::LlmClient;
use jarvis_types::{
    validate_params, InvocationPlan, RiskLevel, RouteSource, SkillInvocation, SkillManifest,
};
use regex::Regex;
use serde_json::{json, Map, Value};
use strsim::jaro_winkler;
use thiserror::Error;

const FUZZY_THRESHOLD: f64 = 0.87;

/// Router result.
#[derive(Clone, Debug, PartialEq)]
pub enum RouteResult {
    /// A concrete invocation plan.
    Plan(InvocationPlan),
    /// A clarification question for the user.
    Clarify(String),
}

/// Router errors.
#[derive(Debug, Error)]
pub enum RouterError {
    /// LLM request failed.
    #[error("llm error: {0}")]
    Llm(String),
}

/// Intent router.
#[derive(Clone, Debug, Default)]
pub struct Router;

impl Router {
    /// Routes user text through rules, fuzzy matching, then LLM fallback.
    ///
    /// Stages 1-2 are intentionally linear and allocation-light; benchmark target:
    /// < 5 ms for a 50-skill catalog on a typical developer laptop.
    pub async fn route(
        text: &str,
        catalog: &[SkillManifest],
        llm: &dyn LlmClient,
    ) -> Result<RouteResult, RouterError> {
        if let Some(plan) = route_by_rules(text, catalog) {
            return Ok(RouteResult::Plan(plan));
        }

        if let Some(plan) = route_by_fuzzy(text, catalog) {
            return Ok(RouteResult::Plan(plan));
        }

        Ok(route_by_llm(text, catalog, llm).await)
    }
}

fn route_by_rules(text: &str, catalog: &[SkillManifest]) -> Option<InvocationPlan> {
    let mut best: Option<(InvocationPlan, usize)> = None;
    for manifest in catalog {
        for trigger in &manifest.triggers {
            let Ok(regex) = Regex::new(&trigger.pattern) else {
                continue;
            };
            let Some(captures) = regex.captures(text) else {
                continue;
            };
            let params = captures_to_params(&regex, &captures);
            if validate_params(manifest, &params).is_ok() {
                let candidate = plan(manifest.id.clone(), params, RouteSource::Rule, 1.0);
                if best
                    .as_ref()
                    .is_none_or(|(_, length)| trigger.pattern.len() > *length)
                {
                    best = Some((candidate, trigger.pattern.len()));
                }
            }
        }
    }
    best.map(|(plan, _)| plan)
}

fn route_by_fuzzy(text: &str, catalog: &[SkillManifest]) -> Option<InvocationPlan> {
    let normalized = normalize(text);
    let mut best: Option<(&SkillManifest, f64)> = None;

    for manifest in catalog {
        if manifest.risk == RiskLevel::Destructive || required_params(manifest) {
            continue;
        }
        for example in &manifest.examples {
            let score = jaro_winkler(&normalized, &normalize(example));
            if score >= FUZZY_THRESHOLD && best.is_none_or(|(_, best_score)| score > best_score) {
                best = Some((manifest, score));
            }
        }
    }

    best.map(|(manifest, score)| {
        plan(
            manifest.id.clone(),
            json!({}),
            RouteSource::Fuzzy,
            score as f32,
        )
    })
}

async fn route_by_llm(text: &str, catalog: &[SkillManifest], llm: &dyn LlmClient) -> RouteResult {
    let schema = json!({
        "type": "object",
        "oneOf": [
            {
                "type": "object",
                "properties": {
                    "skill_id": { "type": "string" },
                    "params": { "type": "object" }
                },
                "required": ["skill_id", "params"],
                "additionalProperties": false
            },
            {
                "type": "object",
                "properties": {
                    "clarify": { "type": "string" }
                },
                "required": ["clarify"],
                "additionalProperties": false
            }
        ]
    });
    let system = build_system_prompt(catalog);
    let response = match llm.complete_json(&system, text, &schema).await {
        Ok(response) => response,
        Err(_) => return RouteResult::Clarify("Could you rephrase that request?".to_string()),
    };
    let result = parse_llm_route_response(response, catalog);
    match result {
        Ok(route) => route,
        Err(validation_error) => {
            let retry_user = format!(
                "{text}\n\nYour previous routing response was invalid: {validation_error}. Return one valid JSON object only."
            );
            match llm.complete_json(&system, &retry_user, &schema).await {
                Ok(response) => parse_llm_route_response(response, catalog).unwrap_or_else(|_| {
                    RouteResult::Clarify("I need more details before running that.".to_string())
                }),
                Err(_) => RouteResult::Clarify("Could you rephrase that request?".to_string()),
            }
        }
    }
}

fn parse_llm_route_response(
    response: Value,
    catalog: &[SkillManifest],
) -> Result<RouteResult, String> {
    if let Some(question) = response.get("clarify").and_then(Value::as_str) {
        return Ok(RouteResult::Clarify(question.to_string()));
    }

    let Some(skill_id) = response.get("skill_id").and_then(Value::as_str) else {
        return Err("missing skill_id".to_string());
    };
    let Some(manifest) = catalog.iter().find(|manifest| manifest.id == skill_id) else {
        return Err(format!("unknown skill_id: {skill_id}"));
    };
    let params = response.get("params").cloned().unwrap_or_else(|| json!({}));

    if let Err(error) = validate_params(manifest, &params) {
        return Err(error.to_string());
    }

    Ok(RouteResult::Plan(plan(
        manifest.id.clone(),
        params,
        RouteSource::Llm,
        0.72,
    )))
}

fn build_system_prompt(catalog: &[SkillManifest]) -> String {
    let mut prompt = String::from(
        "You are Jarvis' intent router. Respond with a single JSON object and no prose.\n\
         Use {\"skill_id\":\"category.name\",\"params\":{...}} when a listed skill fits, or {\"clarify\":\"short question\"} when required information is missing.\n\
         Never invent skill ids. Never use numeric ids. Params must contain ONLY properties from the skill schema; use {} when the skill takes none.\n\
         Worked example: user='open chrome' -> {\"skill_id\":\"system.open_app\",\"params\":{\"name\":\"chrome\"}}.\n\n\
         Skill catalog:\n",
    );
    for (index, manifest) in catalog.iter().enumerate() {
        let examples = manifest
            .examples
            .iter()
            .take(2)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        prompt.push_str(&format!(
            "{}. id={} description={} params={} examples={}\n",
            index + 1,
            manifest.id,
            manifest.description,
            params_summary(manifest),
            examples
        ));
    }
    prompt
}

fn params_summary(manifest: &SkillManifest) -> String {
    let required = manifest
        .params_schema
        .get("required")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let properties = manifest
        .params_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|object| object.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    format!("properties=[{properties}] required=[{required}]")
}

fn captures_to_params(regex: &Regex, captures: &regex::Captures<'_>) -> Value {
    let mut params = Map::new();
    for name in regex.capture_names().flatten() {
        if let Some(value) = captures.name(name) {
            params.insert(name.to_string(), json!(value.as_str().trim()));
        }
    }
    Value::Object(params)
}

fn required_params(manifest: &SkillManifest) -> bool {
    manifest
        .params_schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| !required.is_empty())
}

fn normalize(text: &str) -> String {
    text.trim().to_lowercase()
}

fn plan(skill_id: String, params: Value, source: RouteSource, confidence: f32) -> InvocationPlan {
    InvocationPlan {
        steps: vec![SkillInvocation { skill_id, params }],
        source,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jarvis_llm::{MockLlm, MockResponse};
    use jarvis_types::{Permission, Trigger};

    fn skill(
        id: &str,
        required: &[&str],
        examples: &[&str],
        triggers: &[&str],
        risk: RiskLevel,
    ) -> SkillManifest {
        let mut properties = Map::new();
        for required in required {
            properties.insert((*required).to_string(), json!({ "type": "string" }));
        }
        SkillManifest {
            id: id.to_string(),
            version: semver::Version::new(0, 1, 0),
            description: format!("Skill {id}"),
            params_schema: json!({
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }),
            permissions: vec![Permission::ProcessSpawn],
            risk,
            examples: examples.iter().map(|value| (*value).to_string()).collect(),
            triggers: triggers
                .iter()
                .map(|pattern| Trigger {
                    pattern: (*pattern).to_string(),
                })
                .collect(),
        }
    }

    fn catalog() -> Vec<SkillManifest> {
        vec![
            skill(
                "system.open_app",
                &["name"],
                &["open chrome", "открой vscode"],
                &[r"(?i)^open (?P<name>.+)$", r"(?i)^открой (?P<name>.+)$"],
                RiskLevel::Safe,
            ),
            skill(
                "files.open_folder",
                &["path"],
                &["open folder downloads", "открой папку загрузки"],
                &[
                    r"(?i)^open folder (?P<path>.+)$",
                    r"(?i)^открой папку (?P<path>.+)$",
                ],
                RiskLevel::Safe,
            ),
            skill(
                "files.search",
                &["root", "pattern"],
                &["search files", "найди файлы"],
                &[
                    r"(?i)^search (?P<pattern>.+) in (?P<root>.+)$",
                    r"(?i)^найди (?P<pattern>.+) в (?P<root>.+)$",
                ],
                RiskLevel::Safe,
            ),
            skill(
                "files.convert_images",
                &["folder", "from", "to"],
                &["convert images", "конвертируй картинки"],
                &[
                    r"(?i)^convert (?P<from>png|jpg|jpeg|webp) to (?P<to>png|jpg|jpeg|webp) in (?P<folder>.+)$",
                    r"(?i)^конвертируй (?P<from>png|jpg|jpeg|webp) в (?P<to>png|jpg|jpeg|webp) в (?P<folder>.+)$",
                ],
                RiskLevel::Moderate,
            ),
            skill(
                "system.info",
                &[],
                &["system info", "show system info", "информация о системе"],
                &[],
                RiskLevel::Safe,
            ),
            skill(
                "system.processes",
                &[],
                &["show processes", "top processes", "процессы"],
                &[],
                RiskLevel::Safe,
            ),
            skill(
                "web.open_url",
                &["url"],
                &["open url", "открой сайт"],
                &[
                    r"(?i)^open (?P<url>https?://.+)$",
                    r"(?i)^открой (?P<url>https?://.+)$",
                ],
                RiskLevel::Safe,
            ),
            skill(
                "memory.remember",
                &["key", "value"],
                &["remember", "запомни"],
                &[
                    r"(?i)^remember (?P<key>[^=]+)=(?P<value>.+)$",
                    r"(?i)^запомни (?P<key>[^=]+)=(?P<value>.+)$",
                ],
                RiskLevel::Safe,
            ),
            skill(
                "memory.recall",
                &["key"],
                &["recall", "вспомни"],
                &[r"(?i)^recall (?P<key>.+)$", r"(?i)^вспомни (?P<key>.+)$"],
                RiskLevel::Safe,
            ),
        ]
    }

    async fn assert_route(text: &str, source: RouteSource, skill_id: &str, params: Value) {
        let llm = MockLlm::new(vec![]);
        let result = Router::route(text, &catalog(), &llm).await.expect("route");
        let RouteResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        assert_eq!(plan.source, source);
        assert_eq!(plan.steps[0].skill_id, skill_id);
        assert_eq!(plan.steps[0].params, params);
    }

    #[tokio::test]
    async fn routes_twenty_en_ru_utterances_deterministically() {
        let cases = [
            ("open chrome", "system.open_app", json!({"name":"chrome"})),
            ("open notepad", "system.open_app", json!({"name":"notepad"})),
            (
                "open folder C:/dev",
                "files.open_folder",
                json!({"path":"C:/dev"}),
            ),
            (
                "search *.rs in C:/dev",
                "files.search",
                json!({"pattern":"*.rs","root":"C:/dev"}),
            ),
            (
                "convert png to jpg in C:/pics",
                "files.convert_images",
                json!({"from":"png","to":"jpg","folder":"C:/pics"}),
            ),
            (
                "open https://example.com",
                "web.open_url",
                json!({"url":"https://example.com"}),
            ),
            (
                "remember project.root=C:/dev",
                "memory.remember",
                json!({"key":"project.root","value":"C:/dev"}),
            ),
            (
                "recall project.root",
                "memory.recall",
                json!({"key":"project.root"}),
            ),
            ("открой хром", "system.open_app", json!({"name":"хром"})),
            ("открой vscode", "system.open_app", json!({"name":"vscode"})),
            (
                "открой папку C:/dev",
                "files.open_folder",
                json!({"path":"C:/dev"}),
            ),
            (
                "найди *.png в D:/pics",
                "files.search",
                json!({"pattern":"*.png","root":"D:/pics"}),
            ),
            (
                "конвертируй png в webp в D:/pics",
                "files.convert_images",
                json!({"from":"png","to":"webp","folder":"D:/pics"}),
            ),
            (
                "открой https://example.com",
                "web.open_url",
                json!({"url":"https://example.com"}),
            ),
            (
                "запомни user.name=Tim",
                "memory.remember",
                json!({"key":"user.name","value":"Tim"}),
            ),
            (
                "вспомни user.name",
                "memory.recall",
                json!({"key":"user.name"}),
            ),
        ];
        for (text, skill_id, params) in cases {
            assert_route(text, RouteSource::Rule, skill_id, params).await;
        }

        assert_route("system info", RouteSource::Fuzzy, "system.info", json!({})).await;
        assert_route(
            "show system info",
            RouteSource::Fuzzy,
            "system.info",
            json!({}),
        )
        .await;
        assert_route(
            "процессы",
            RouteSource::Fuzzy,
            "system.processes",
            json!({}),
        )
        .await;
        assert_route(
            "top processes",
            RouteSource::Fuzzy,
            "system.processes",
            json!({}),
        )
        .await;
    }

    #[tokio::test]
    async fn llm_routes_when_rules_and_fuzzy_do_not_match() {
        let llm = MockLlm::new(vec![(
            "launch browser".to_string(),
            MockResponse::Json(json!({
                "skill_id": "system.open_app",
                "params": { "name": "chrome" }
            })),
        )]);
        let result = Router::route("launch browser", &catalog(), &llm)
            .await
            .expect("route");
        let RouteResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        assert_eq!(plan.source, RouteSource::Llm);
        assert_eq!(plan.steps[0].skill_id, "system.open_app");
    }

    #[tokio::test]
    async fn unknown_request_clarifies() {
        let llm = MockLlm::new(vec![(
            "make coffee".to_string(),
            MockResponse::Json(json!({ "clarify": "What should I do?" })),
        )]);
        assert!(matches!(
            Router::route("make coffee", &catalog(), &llm)
                .await
                .expect("route"),
            RouteResult::Clarify(_)
        ));
    }

    #[tokio::test]
    async fn unknown_llm_skill_clarifies() {
        let llm = MockLlm::new(vec![(
            "launch browser".to_string(),
            MockResponse::Json(json!({
                "skill_id": "unknown.skill",
                "params": {}
            })),
        )]);
        assert!(matches!(
            Router::route("launch browser", &catalog(), &llm)
                .await
                .expect("route"),
            RouteResult::Clarify(_)
        ));
    }

    #[tokio::test]
    async fn llm_garbage_clarifies() {
        let llm = MockLlm::new(vec![(
            "???".to_string(),
            MockResponse::Text("not json".to_string()),
        )]);
        assert!(matches!(
            Router::route("???", &catalog(), &llm).await.expect("route"),
            RouteResult::Clarify(_)
        ));
    }

    #[tokio::test]
    async fn invalid_trigger_does_not_abort_rules_stage() {
        let mut catalog = catalog();
        catalog[0].triggers.insert(
            0,
            Trigger {
                pattern: "(".to_string(),
            },
        );
        let llm = MockLlm::new(vec![]);

        let result = Router::route("open chrome", &catalog, &llm)
            .await
            .expect("route");

        let RouteResult::Plan(plan) = result else {
            panic!("expected plan");
        };
        assert_eq!(plan.source, RouteSource::Rule);
        assert_eq!(plan.steps[0].skill_id, "system.open_app");
    }
}
