//! Planning: what it produces, and what it is not.

use async_trait::async_trait;
use poorai_domain::{
    BackendState, ChatMessage, DeploymentDescriptor, ModelChunk, ModelInspection, ModelRequest,
    ToolCall,
};
use poorai_provider::{ModelProvider, ModelStream, ProviderError};
use poorai_store::Store;

struct PlanningProvider {
    steps: Option<serde_json::Value>,
}
#[async_trait]
impl ModelProvider for PlanningProvider {
    async fn inspect(&self, _: &DeploymentDescriptor) -> Result<ModelInspection, ProviderError> {
        unreachable!()
    }
    async fn runtime_state(&self) -> Result<BackendState, ProviderError> {
        unreachable!()
    }
    async fn chat(&self, _: ModelRequest) -> Result<ModelStream, ProviderError> {
        let chunk = match &self.steps {
            Some(steps) => ModelChunk {
                tool_calls: vec![ToolCall {
                    name: "plan".into(),
                    arguments: serde_json::json!({ "steps": steps }),
                    id: None,
                }],
                done: true,
                ..Default::default()
            },
            // A deployment that answers in prose instead of calling the tool.
            None => ModelChunk {
                content: "I will fix the bug and then run the tests.".into(),
                done: true,
                ..Default::default()
            },
        };
        Ok(Box::pin(futures_util::stream::iter([Ok(chunk)])))
    }
}

fn request() -> ModelRequest {
    ModelRequest {
        deployment: DeploymentDescriptor {
            schema_version: 1,
            id: poorai_domain::new_id(),
            provider: "fake".into(),
            endpoint: "http://localhost/".into(),
            model_ref: "fake".into(),
            backend_options: Default::default(),
            auth_ref: None,
        },
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "fix it".into(),
        }],
        context_tokens: 4096,
        tools: None,
        seed: None,
        sampling: Default::default(),
    }
}

#[tokio::test]
async fn a_plan_is_recorded_with_its_steps() {
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let provider = PlanningProvider {
        steps: Some(serde_json::json!([
            "read src/lib.rs",
            "fix discount()",
            "run tests"
        ])),
    };
    let steps = poorai_orchestrator::plan_task(&provider, &store, run_id, &request())
        .await
        .unwrap();
    assert_eq!(steps.len(), 3);
    let recorded = store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == "task.plan")
        .expect("the plan was not recorded");
    assert_eq!(recorded.payload["produced"], true);
    assert_eq!(recorded.payload["steps"][1], "fix discount()");
}

/// A deployment asked for a plan that answers in prose produced none, and that
/// is a fact about the deployment worth recording rather than an error.
#[tokio::test]
async fn a_deployment_that_produces_no_plan_is_recorded_as_such() {
    let store = Store::open(":memory:").unwrap();
    let run_id = poorai_domain::new_id();
    let steps = poorai_orchestrator::plan_task(
        &PlanningProvider { steps: None },
        &store,
        run_id,
        &request(),
    )
    .await
    .unwrap();
    assert!(steps.is_empty());
    let recorded = store
        .events_for_run(run_id)
        .unwrap()
        .into_iter()
        .find(|e| e.event_type == "task.plan")
        .unwrap();
    assert_eq!(recorded.payload["produced"], false);
}

/// A plan is context, not a script: it is bounded so it cannot become one.
#[tokio::test]
async fn a_plan_is_bounded_and_drops_empty_steps() {
    let store = Store::open(":memory:").unwrap();
    let many: Vec<String> = (1..=40).map(|i| format!("step {i}")).collect();
    let steps = poorai_orchestrator::plan_task(
        &PlanningProvider {
            steps: Some(serde_json::json!(many)),
        },
        &store,
        poorai_domain::new_id(),
        &request(),
    )
    .await
    .unwrap();
    assert!(
        steps.len() <= 8,
        "a plan of {} steps is a script",
        steps.len()
    );

    let store2 = Store::open(":memory:").unwrap();
    let steps = poorai_orchestrator::plan_task(
        &PlanningProvider {
            steps: Some(serde_json::json!(["real step", "", "   "])),
        },
        &store2,
        poorai_domain::new_id(),
        &request(),
    )
    .await
    .unwrap();
    assert_eq!(
        steps
            .steps
            .iter()
            .map(|step| step.statement.as_str())
            .collect::<Vec<_>>(),
        vec!["real step"]
    );
}

/// A plan is a graph, and a list is the graph where every step waits on the one
/// before it. Most plans are not that shape: three files can be edited in any
/// order and the fourth step needs all three, which a list cannot say.
#[tokio::test]
async fn a_plan_may_carry_dependencies_and_its_own_checks() {
    let store = Store::open(":memory:").unwrap();
    let plan = poorai_orchestrator::plan_task(
        &PlanningProvider {
            steps: Some(serde_json::json!([
                {"step": "add the parser"},
                {"step": "add the lexer"},
                {"step": "wire them together", "depends_on": [1, 2], "verify": ["cargo", "test"]},
            ])),
        },
        &store,
        poorai_domain::new_id(),
        &request(),
    )
    .await
    .unwrap();
    assert_eq!(plan.len(), 3);
    // Two steps can start; the third waits on both.
    assert_eq!(plan.ready(), vec![1, 2]);
    assert_eq!(plan.blocked(), vec![3]);
    assert_eq!(
        plan.verifier(3),
        Some(("cargo".to_string(), vec!["test".to_string()]))
    );
    assert!(plan.verifier(1).is_none(), "a step invented a check");
}

/// The older shape is still a plan. A deployment answering with a list of
/// sentences is not failing -- it is planning without a graph, which is what
/// every plan in this project has been until now.
#[tokio::test]
async fn a_plain_list_of_sentences_is_still_a_plan() {
    let store = Store::open(":memory:").unwrap();
    let plan = poorai_orchestrator::plan_task(
        &PlanningProvider {
            steps: Some(serde_json::json!(["read it", "fix it"])),
        },
        &store,
        poorai_domain::new_id(),
        &request(),
    )
    .await
    .unwrap();
    assert_eq!(plan.len(), 2);
    // Nothing waits on anything, so everything is ready.
    assert_eq!(plan.ready(), vec![1, 2]);
    assert!(plan.blocked().is_empty());
}
