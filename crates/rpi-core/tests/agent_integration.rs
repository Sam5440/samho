use anyhow::Result;
use axum::{Json, Router, extract::State, routing::post};
use rpi_core::model::{ModelDescriptor, ModelTier, ProviderKind};
use rpi_core::test_support::bind_loopback;
use rpi_core::{AgentRuntime, RuntimeConfig, TaskProfile, TaskRequest};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio_util::sync::CancellationToken;

fn request(prompt: &str) -> TaskRequest {
    TaskRequest {
        session_id: None,
        prompt: prompt.into(),
        profile: TaskProfile::default(),
        skills: Vec::new(),
    }
}
fn config(dir: &std::path::Path) -> RuntimeConfig {
    RuntimeConfig {
        workspace: dir.into(),
        data_dir: dir.join("state"),
        durable_logs: false,
        log_content: true,
        ..RuntimeConfig::default()
    }
}
#[derive(Clone)]
struct Fixture {
    mode: String,
    requests: Arc<tokio::sync::Mutex<Vec<Value>>>,
    counter: Arc<AtomicUsize>,
}
async fn respond(
    State(fixture): State<Fixture>,
    Json(body): Json<Value>,
) -> (axum::http::StatusCode, Json<Value>) {
    fixture.requests.lock().await.push(body.clone());
    let turn = fixture.counter.fetch_add(1, Ordering::SeqCst);
    if fixture.mode == "slow" {
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    }
    if fixture.mode == "failure" {
        return (
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error":{"message":"rate limited"}})),
        );
    }
    if fixture.mode == "missing_choices" {
        return (axum::http::StatusCode::OK, Json(json!({"unexpected":true})));
    }
    let call = fixture.mode == "loop"
        || fixture.mode == "truncated"
        || fixture.mode == "malformed"
        || (fixture.mode == "tools" && turn == 0);
    let message = if call {
        json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("call-{turn}"),"type":"function","function":{"name":"read_file","arguments":if fixture.mode=="malformed" {"{\"path\":"} else {"{\"path\":\"input.txt\"}"}}}]})
    } else {
        json!({"role":"assistant","content":"answer from real local HTTP fixture"})
    };
    (
        axum::http::StatusCode::OK,
        Json(
            json!({"choices":[{"message":message,"finish_reason":if fixture.mode=="truncated" {"length"}else if call{"tool_calls"}else{"stop"}}],"usage":{"prompt_tokens":7,"completion_tokens":3}}),
        ),
    )
}
async fn fixture(
    mode: &str,
    dir: &std::path::Path,
) -> (Arc<AgentRuntime>, Fixture, tokio::task::JoinHandle<()>) {
    let listener = bind_loopback().await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = Fixture {
        mode: mode.into(),
        requests: Default::default(),
        counter: Default::default(),
    };
    let app = Router::new()
        .route("/v1/chat/completions", post(respond))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let mut settings = config(dir);
    settings.request_timeout_ms = if mode == "slow" { 50 } else { 3000 };
    settings.max_turns = 3;
    settings.models = vec![ModelDescriptor {
        id: "fixture".into(),
        model: "fixture-model".into(),
        provider: ProviderKind::ChatCompletions,
        tier: ModelTier::Balanced,
        base_url: Some(format!("http://{address}/v1")),
        ..ModelDescriptor::default()
    }];
    (AgentRuntime::new(settings).await.unwrap(), state, server)
}
#[tokio::test]
async fn http_provider_executes_tools_and_persists_correlated_transcript() -> Result<()> {
    let dir = tempfile::tempdir()?;
    tokio::fs::write(dir.path().join("input.txt"), "actual file bytes").await?;
    let (runtime, state, server) = fixture("tools", dir.path()).await;
    let result = runtime
        .run(request("inspect the file"), CancellationToken::new())
        .await?;
    assert_eq!(result.turns, 2);
    assert_eq!(result.usage.input_tokens, 14);
    assert_eq!(result.usage.output_tokens, 6);
    let calls = state.requests.lock().await;
    assert!(
        calls[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["function"]["name"] == "read_file")
    );
    assert!(
        calls[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool"
                && message["content"]
                    .as_str()
                    .unwrap()
                    .contains("actual file bytes"))
    );
    drop(calls);
    let events = runtime.events.events(Some(&result.trace_id)).await?;
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "tool.result")
            .count(),
        1
    );
    let start = events
        .iter()
        .find(|event| event.kind == "tool.start")
        .unwrap();
    let end = events
        .iter()
        .find(|event| event.kind == "tool.result")
        .unwrap();
    assert_eq!(start.span_id, end.span_id);
    assert!(
        events
            .iter()
            .filter(|event| event.kind != "task.start")
            .all(|event| event.parent_span_id.as_deref() == Some(result.task_id.as_str()))
    );
    assert_eq!(
        runtime.session(&result.session_id).await?.unwrap().status,
        "done"
    );
    runtime.shutdown().await?;
    server.abort();
    Ok(())
}
#[tokio::test]
async fn failures_cancellation_truncated_arguments_and_turn_limit_are_not_success() -> Result<()> {
    for mode in [
        "failure",
        "missing_choices",
        "truncated",
        "malformed",
        "loop",
        "slow",
    ] {
        let dir = tempfile::tempdir()?;
        tokio::fs::write(dir.path().join("input.txt"), "x").await?;
        let (runtime, _, server) = fixture(mode, dir.path()).await;
        let error = runtime
            .run(request("probe"), CancellationToken::new())
            .await
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        let sessions = runtime.sessions().await?;
        assert_eq!(sessions[0].status, "failed", "{mode}");
        let events = runtime.events.events(None).await?;
        assert!(
            events.iter().any(|event| event.kind == "task.failed"),
            "{mode}"
        );
        if ["truncated", "malformed"].contains(&mode) {
            assert!(!events.iter().any(|event| event.kind == "tool.start"));
        }
        if mode == "slow" || mode == "failure" {
            assert!(events.iter().any(|event| event.kind == "model.error"));
        }
        runtime.shutdown().await?;
        server.abort();
    }
    Ok(())
}
#[tokio::test]
async fn demo_read_write_policy_skills_and_resume_use_real_objects() -> Result<()> {
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("input.txt"), "hello Rust")?;
    let root = dir.path().join("skills/check");
    std::fs::create_dir_all(&root)?;
    std::fs::write(
        root.join("SKILL.md"),
        "---\nname: check\ndescription: A file inspection workflow.\n---\nInspect carefully.",
    )?;
    let mut settings = config(dir.path());
    settings.skill_roots = vec![dir.path().join("skills")];
    let runtime = AgentRuntime::new(settings.clone()).await?;
    let result = runtime
        .run(request("/demo read input.txt"), CancellationToken::new())
        .await?;
    assert_eq!(result.turns, 2);
    assert!(result.output.contains("hello Rust"));
    let skill = runtime
        .run(request("/demo skill check"), CancellationToken::new())
        .await?;
    assert!(skill.output.contains("Inspect carefully"));
    let denied = runtime
        .run(
            request("/demo write input.txt\nchanged"),
            CancellationToken::new(),
        )
        .await;
    assert!(denied.is_err());
    assert_eq!(
        std::fs::read_to_string(dir.path().join("input.txt"))?,
        "hello Rust"
    );
    let session_id = result.session_id.clone();
    runtime.shutdown().await?;
    drop(runtime);
    let runtime = AgentRuntime::new(settings).await?;
    let mut followup = request("continue");
    followup.session_id = Some(session_id.clone());
    let result = runtime.run(followup, CancellationToken::new()).await?;
    assert_eq!(result.session_id, session_id);
    assert!(runtime.session(&session_id).await?.unwrap().messages.len() > 4);
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn data_directory_lock_prevents_concurrent_process_writers_and_unknown_sessions_fail()
-> Result<()> {
    let dir = tempfile::tempdir()?;
    let runtime = AgentRuntime::new(config(dir.path())).await?;
    assert!(AgentRuntime::new(config(dir.path())).await.is_err());
    let mut input = request("bad session");
    input.session_id = Some("../does-not-exist".into());
    assert!(runtime.run(input, CancellationToken::new()).await.is_err());
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn parallel_tasks_are_isolated_and_route_different_complexities() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let runtime = AgentRuntime::new(config(dir.path())).await?;
    let mut tasks = Vec::new();
    for i in 0..20 {
        let runtime = runtime.clone();
        tasks.push(tokio::spawn(async move {
            let mut input = request(&format!("task {i}"));
            input.profile = TaskProfile::classify((i % 10) as u8);
            runtime.run(input, CancellationToken::new()).await
        }));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut models = std::collections::BTreeSet::new();
    for task in tasks {
        let result = task.await??;
        assert!(ids.insert(result.session_id));
        models.insert(result.model_id);
    }
    assert_eq!(ids.len(), 20);
    assert_eq!(models.len(), 5);
    let summaries = runtime.session_summaries().await;
    assert_eq!(summaries.len(), 20);
    for summary in summaries {
        let session = runtime.session(&summary.id).await?.unwrap();
        assert_eq!(summary.message_count, session.messages.len());
        assert_eq!(summary.status, session.status);
        assert_eq!(summary.last_model, session.last_model);
        assert!(serde_json::to_value(summary)?.get("messages").is_none());
    }
    let events = runtime.events.events(None).await?;
    assert!(
        events
            .windows(2)
            .all(|pair| pair[1].sequence == pair[0].sequence + 1)
    );
    runtime.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn model_object_and_custom_tool_are_invocable_and_disposed() -> Result<()> {
    use rpi_core::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind};
    struct Custom(Arc<AtomicUsize>);
    #[async_trait::async_trait]
    impl Object for Custom {
        fn envelope(&self) -> ObjectEnvelope {
            let mut descriptor = ObjectEnvelope::new(
                "tool:custom_echo",
                ObjectKind::Tool,
                "A registered test tool",
            );
            descriptor.spec = json!({"type":"object","properties":{"value":{"type":"string"}},"required":["value"]});
            descriptor
        }
        async fn invoke(
            &self,
            _method: &str,
            input: Value,
            _context: InvocationContext,
        ) -> Result<Value> {
            Ok(json!({"echo":input["value"]}))
        }
        async fn dispose(&self) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let dir = tempfile::tempdir()?;
    let runtime = AgentRuntime::new(config(dir.path())).await?;
    let disposed = Arc::new(AtomicUsize::new(0));
    runtime
        .register_tool(Arc::new(Custom(disposed.clone())))
        .await?;
    let context = InvocationContext {
        session_id: "s".into(),
        trace_id: "t".into(),
        span_id: "p".into(),
        cancel: CancellationToken::new(),
    };
    assert_eq!(
        runtime
            .tools
            .call("custom_echo", json!({"value":"hello"}), context.clone())
            .await?["echo"],
        "hello"
    );
    assert!(
        runtime
            .tools
            .schemas()
            .await
            .iter()
            .any(|schema| schema["function"]["name"] == "custom_echo")
    );
    let response = runtime
        .objects
        .get("model:demo-balanced")?
        .invoke(
            "complete",
            json!({"messages":[{"role":"user","content":"hello"}]}),
            context,
        )
        .await?;
    assert!(
        response["message"]["content"]
            .as_str()
            .unwrap()
            .contains("DEMO simulation")
    );
    runtime.shutdown().await?;
    assert_eq!(disposed.load(Ordering::SeqCst), 1);
    Ok(())
}
