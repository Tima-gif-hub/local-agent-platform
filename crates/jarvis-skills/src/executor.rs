//! Skill execution pipeline.

use std::{collections::HashMap, sync::Arc};

use jarvis_store::{AuditOutcome, NewAuditRecord, Store};
use jarvis_types::{
    validate_params, Confirmer, InvocationPlan, MemoryPort, PathScope, Permission, RiskLevel,
    Skill, SkillContext, SkillError, SkillInvocation, SkillOutput, Spawner,
};
use serde_json::json;

/// Executor configuration.
#[derive(Clone)]
pub struct ExecutorConfig {
    /// Confirmation threshold.
    pub confirm_threshold: RiskLevel,
    /// Granted permissions.
    pub grants: Vec<Permission>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            confirm_threshold: RiskLevel::Moderate,
            grants: Vec::new(),
        }
    }
}

/// Executes invocation plans through validation, permissions, confirmation, skill execution, and audit.
pub struct Executor {
    skills: HashMap<String, Arc<dyn Skill>>,
    store: Store,
    confirmer: Arc<dyn Confirmer>,
    spawner: Arc<dyn Spawner>,
    memory: Option<Arc<dyn MemoryPort>>,
    config: ExecutorConfig,
}

impl Executor {
    /// Creates an executor.
    pub fn new(
        skills: Vec<Arc<dyn Skill>>,
        store: Store,
        confirmer: Arc<dyn Confirmer>,
        spawner: Arc<dyn Spawner>,
        memory: Option<Arc<dyn MemoryPort>>,
        mut config: ExecutorConfig,
    ) -> Self {
        if config.grants.is_empty() {
            config.grants = default_grants(&skills);
        }
        let skills = skills
            .into_iter()
            .map(|skill| (skill.manifest().id.clone(), skill))
            .collect();
        Self {
            skills,
            store,
            confirmer,
            spawner,
            memory,
            config,
        }
    }

    /// Runs an invocation plan.
    pub async fn run(&self, plan: InvocationPlan) -> ExecutionReport {
        let mut steps = Vec::new();
        for invocation in plan.steps.clone() {
            steps.push(self.run_step(&plan, invocation).await);
        }
        let success = steps
            .iter()
            .all(|step| step.outcome == AuditOutcome::Success);
        let summary = steps
            .iter()
            .filter_map(|step| step.output.as_ref().map(|output| output.summary.clone()))
            .collect::<Vec<_>>()
            .join(" ");
        ExecutionReport {
            success,
            summary,
            steps,
        }
    }

    async fn run_step(&self, plan: &InvocationPlan, invocation: SkillInvocation) -> StepReport {
        let Some(skill) = self.skills.get(&invocation.skill_id) else {
            let output = SkillOutput {
                summary: format!("Unknown skill {}.", invocation.skill_id),
                data: json!({}),
            };
            self.audit(
                &invocation,
                RiskLevel::Safe,
                AuditOutcome::InvalidParams,
                plan,
            )
            .await;
            return StepReport::error(
                invocation,
                RiskLevel::Safe,
                AuditOutcome::InvalidParams,
                output.summary,
            );
        };

        let manifest = skill.manifest();
        if let Err(error) = validate_params(manifest, &invocation.params) {
            self.audit(
                &invocation,
                manifest.risk,
                AuditOutcome::InvalidParams,
                plan,
            )
            .await;
            return StepReport::error(
                invocation,
                manifest.risk,
                AuditOutcome::InvalidParams,
                error.to_string(),
            );
        }

        if !manifest
            .permissions
            .iter()
            .all(|permission| permission_granted(permission, &self.config.grants))
        {
            self.audit(
                &invocation,
                manifest.risk,
                AuditOutcome::DeniedPermission,
                plan,
            )
            .await;
            return StepReport::error(
                invocation,
                manifest.risk,
                AuditOutcome::DeniedPermission,
                SkillError::PermissionDenied.to_string(),
            );
        }

        let ctx = self.context();
        if manifest.risk >= self.config.confirm_threshold {
            let prompt = format!("Run {} with params {}?", manifest.id, invocation.params);
            if let Err(error) = ctx.request_confirmation(prompt).await {
                self.audit(
                    &invocation,
                    manifest.risk,
                    AuditOutcome::DeniedConfirmation,
                    plan,
                )
                .await;
                return StepReport::error(
                    invocation,
                    manifest.risk,
                    AuditOutcome::DeniedConfirmation,
                    error.to_string(),
                );
            }
        }

        match skill.execute(invocation.params.clone(), &ctx).await {
            Ok(output) => {
                self.audit(&invocation, manifest.risk, AuditOutcome::Success, plan)
                    .await;
                StepReport {
                    skill_id: invocation.skill_id,
                    risk: manifest.risk,
                    outcome: AuditOutcome::Success,
                    output: Some(output),
                    error: None,
                }
            }
            Err(error) => {
                self.audit(
                    &invocation,
                    manifest.risk,
                    audit_outcome_for_error(&error),
                    plan,
                )
                .await;
                StepReport::error(
                    invocation,
                    manifest.risk,
                    audit_outcome_for_error(&error),
                    error.to_string(),
                )
            }
        }
    }

    fn context(&self) -> SkillContext {
        let ctx = SkillContext::with_spawner(
            self.config.grants.clone(),
            self.confirmer.clone(),
            self.spawner.clone(),
        );
        if let Some(memory) = &self.memory {
            ctx.with_memory(memory.clone())
        } else {
            ctx
        }
    }

    async fn audit(
        &self,
        invocation: &SkillInvocation,
        risk: RiskLevel,
        outcome: AuditOutcome,
        plan: &InvocationPlan,
    ) {
        let _ = self
            .store
            .audit()
            .append(NewAuditRecord {
                skill_id: invocation.skill_id.clone(),
                params: invocation.params.clone(),
                risk,
                outcome,
                route_source: plan.source,
            })
            .await;
    }
}

/// Execution report.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecutionReport {
    /// Whether all steps succeeded.
    pub success: bool,
    /// Combined summary.
    pub summary: String,
    /// Step reports.
    pub steps: Vec<StepReport>,
}

/// Single step report.
#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    /// Skill id.
    pub skill_id: String,
    /// Risk level.
    pub risk: RiskLevel,
    /// Audit outcome.
    pub outcome: AuditOutcome,
    /// Skill output, when successful.
    pub output: Option<SkillOutput>,
    /// Error string, when failed.
    pub error: Option<String>,
}

impl StepReport {
    fn error(
        invocation: SkillInvocation,
        risk: RiskLevel,
        outcome: AuditOutcome,
        error: String,
    ) -> Self {
        Self {
            skill_id: invocation.skill_id,
            risk,
            outcome,
            output: None,
            error: Some(error),
        }
    }
}

fn audit_outcome_for_error(error: &SkillError) -> AuditOutcome {
    match error {
        SkillError::InvalidParams(_) => AuditOutcome::InvalidParams,
        SkillError::PermissionDenied => AuditOutcome::DeniedPermission,
        SkillError::ConfirmationDenied => AuditOutcome::DeniedConfirmation,
        SkillError::Execution(_) => AuditOutcome::Failed,
    }
}

fn default_grants(skills: &[Arc<dyn Skill>]) -> Vec<Permission> {
    let mut grants = Vec::new();
    for skill in skills {
        for permission in &skill.manifest().permissions {
            if !grants.iter().any(|grant| grant == permission) {
                grants.push(permission.clone());
            }
        }
    }
    grants
}

fn permission_granted(required: &Permission, grants: &[Permission]) -> bool {
    grants.iter().any(|grant| match (required, grant) {
        (Permission::FsRead(required), Permission::FsRead(grant))
        | (Permission::FsWrite(required), Permission::FsWrite(grant)) => {
            path_scope_granted(required, grant)
        }
        (Permission::ProcessSpawn, Permission::ProcessSpawn)
        | (Permission::ProcessInspect, Permission::ProcessInspect)
        | (Permission::Network, Permission::Network)
        | (Permission::Memory, Permission::Memory) => true,
        _ => false,
    })
}

fn path_scope_granted(required: &PathScope, grant: &PathScope) -> bool {
    match (required, grant) {
        (_, PathScope::Anywhere) => true,
        (PathScope::Within(required), PathScope::Within(grant)) => required.starts_with(grant),
        (PathScope::Anywhere, PathScope::Within(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use jarvis_store::AuditFilter;
    use jarvis_types::{RouteSource, SkillInvocation};
    use serde_json::json;
    use serde_json::Value;
    use std::sync::Mutex;

    struct Confirm(bool);

    #[async_trait]
    impl Confirmer for Confirm {
        async fn confirm(&self, _prompt: String) -> bool {
            self.0
        }
    }

    #[derive(Default)]
    struct NoopSpawner;

    impl Spawner for NoopSpawner {
        fn spawn(&self, _program: &str, _args: &[String]) -> Result<(), SkillError> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct TestMemory(Mutex<HashMap<String, String>>);

    #[async_trait]
    impl MemoryPort for TestMemory {
        async fn remember(&self, key: String, value: String) -> Result<(), SkillError> {
            self.0.lock().expect("memory").insert(key, value);
            Ok(())
        }

        async fn recall(&self, key: String) -> Result<Option<String>, SkillError> {
            Ok(self.0.lock().expect("memory").get(&key).cloned())
        }
    }

    async fn executor(confirm: bool, grants: Vec<Permission>) -> (Executor, Store) {
        let store = Store::open_url("sqlite::memory:").await.expect("store");
        let config = ExecutorConfig {
            confirm_threshold: RiskLevel::Moderate,
            grants,
        };
        let executor = Executor::new(
            crate::builtin_skills(),
            store.clone(),
            Arc::new(Confirm(confirm)),
            Arc::new(NoopSpawner),
            Some(Arc::new(TestMemory::default())),
            config,
        );
        (executor, store)
    }

    fn plan(skill_id: &str, params: Value) -> InvocationPlan {
        InvocationPlan {
            steps: vec![SkillInvocation {
                skill_id: skill_id.to_string(),
                params,
            }],
            source: RouteSource::Rule,
            confidence: 1.0,
        }
    }

    #[tokio::test]
    async fn audits_success() {
        let (executor, store) = executor(true, Vec::new()).await;

        let report = executor
            .run(plan("memory.remember", json!({"key":"a","value":"b"})))
            .await;

        assert!(report.success);
        let rows = store
            .audit()
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("audit");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, AuditOutcome::Success);
    }

    #[tokio::test]
    async fn audits_invalid_params() {
        let (executor, store) = executor(true, Vec::new()).await;

        let report = executor
            .run(plan("memory.remember", json!({"key":"a"})))
            .await;

        assert!(!report.success);
        let rows = store
            .audit()
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("audit");
        assert_eq!(rows[0].outcome, AuditOutcome::InvalidParams);
    }

    #[tokio::test]
    async fn audits_confirmation_denied_and_destructive_cannot_run() {
        let destructive = Arc::new(TestSkill::new("test.destroy", RiskLevel::Destructive));
        let store = Store::open_url("sqlite::memory:").await.expect("store");
        let executor = Executor::new(
            vec![destructive],
            store.clone(),
            Arc::new(Confirm(false)),
            Arc::new(NoopSpawner),
            None,
            ExecutorConfig {
                confirm_threshold: RiskLevel::Moderate,
                grants: Vec::new(),
            },
        );

        let report = executor.run(plan("test.destroy", json!({}))).await;

        assert!(!report.success);
        assert_eq!(report.steps[0].outcome, AuditOutcome::DeniedConfirmation);
        let rows = store
            .audit()
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("audit");
        assert_eq!(rows[0].outcome, AuditOutcome::DeniedConfirmation);
    }

    #[tokio::test]
    async fn audits_skill_error_and_is_append_only() {
        let failing = Arc::new(TestSkill::failing("test.fail"));
        let store = Store::open_url("sqlite::memory:").await.expect("store");
        let executor = Executor::new(
            vec![failing],
            store.clone(),
            Arc::new(Confirm(true)),
            Arc::new(NoopSpawner),
            None,
            ExecutorConfig::default(),
        );

        executor.run(plan("test.fail", json!({}))).await;
        executor.run(plan("test.fail", json!({}))).await;

        let rows = store
            .audit()
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("audit");
        assert_eq!(rows.len(), 2);
        assert!(rows[0].id > rows[1].id);
        assert!(rows.iter().all(|row| row.outcome == AuditOutcome::Failed));
    }

    #[tokio::test]
    async fn router_and_executor_integrate_with_mock_llm() {
        use jarvis_llm::MockLlm;
        use jarvis_router::{RouteResult, Router};

        let skills = crate::builtin_skills();
        let catalog = skills
            .iter()
            .map(|skill| skill.manifest().clone())
            .collect::<Vec<_>>();
        let llm = MockLlm::new(vec![]);
        let RouteResult::Plan(plan) = Router::route("remember project.root=C:/dev", &catalog, &llm)
            .await
            .expect("route")
        else {
            panic!("expected plan");
        };
        let store = Store::open_url("sqlite::memory:").await.expect("store");
        let executor = Executor::new(
            skills,
            store.clone(),
            Arc::new(Confirm(true)),
            Arc::new(NoopSpawner),
            Some(Arc::new(TestMemory::default())),
            ExecutorConfig::default(),
        );

        let report = executor.run(plan).await;

        assert!(report.success);
        let rows = store
            .audit()
            .list(AuditFilter::default(), 10, 0)
            .await
            .expect("audit");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, AuditOutcome::Success);
    }

    struct TestSkill {
        manifest: jarvis_types::SkillManifest,
        fail: bool,
    }

    impl TestSkill {
        fn new(id: &str, risk: RiskLevel) -> Self {
            Self {
                manifest: jarvis_types::SkillManifest {
                    id: id.to_string(),
                    version: semver::Version::new(0, 1, 0),
                    description: "Test skill".to_string(),
                    params_schema: json!({
                        "type": "object",
                        "properties": {},
                        "required": [],
                        "additionalProperties": false
                    }),
                    permissions: Vec::new(),
                    risk,
                    examples: Vec::new(),
                    triggers: Vec::new(),
                },
                fail: false,
            }
        }

        fn failing(id: &str) -> Self {
            Self {
                fail: true,
                ..Self::new(id, RiskLevel::Safe)
            }
        }
    }

    #[async_trait]
    impl Skill for TestSkill {
        fn manifest(&self) -> &jarvis_types::SkillManifest {
            &self.manifest
        }

        async fn execute(
            &self,
            _params: Value,
            _ctx: &SkillContext,
        ) -> Result<SkillOutput, SkillError> {
            if self.fail {
                Err(SkillError::Execution("boom".to_string()))
            } else {
                Ok(SkillOutput {
                    summary: "ok".to_string(),
                    data: json!({}),
                })
            }
        }
    }
}
