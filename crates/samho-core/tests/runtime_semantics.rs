use anyhow::Result;
use async_trait::async_trait;
use samho_core::model::{ModelDescriptor, ModelTier, ProviderKind};
use samho_core::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind};
use samho_core::provider::{
    ModelProvider, ModelResponse, ProviderError, ProviderErrorKind, ProviderResult,
    ProviderStreamEvent, Usage,
};
use samho_core::{AgentRuntime, RuntimeConfig, TaskProfile, TaskRequest};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_util::sync::CancellationToken;

fn request(prompt: &str) -> TaskRequest {
    TaskRequest {
        session_id: None,
        prompt: prompt.into(),
        profile: TaskProfile::default(),
        skills: vec![],
    }
}
fn config(dir: &std::path::Path) -> RuntimeConfig {
    let mut config = RuntimeConfig {
        workspace: dir.into(),
        data_dir: dir.join("data"),
        durable_logs: false,
        system_prompt: "受保护的系统指令".into(),
        ..Default::default()
    };
    config.models = vec![
        ModelDescriptor {
            provider: ProviderKind::Custom,
            streaming: true,
            max_output_tokens: 512,
            ..ModelDescriptor::demo("first", ModelTier::Nano)
        },
        ModelDescriptor {
            provider: ProviderKind::Custom,
            streaming: true,
            max_output_tokens: 512,
            ..ModelDescriptor::demo("fallback", ModelTier::Fast)
        },
    ];
    config.retry.initial_backoff_ms = 0;
    config.retry.max_backoff_ms = 1;
    config.tools.read = false;
    config
}
// 预算预留与上下文投影的输入估算都包含广告的工具 schema；
// 校准类测试固定精简工具集，避免默认工具集变化移动边界。
fn pin_minimal_tools(config: &mut RuntimeConfig) {
    config.tools.write = false;
    config.tools.edit = false;
    config.tools.shell = false;
    config.tools.list_files = false;
    config.tools.glob = false;
    config.tools.grep = false;
    config.tools.webfetch = false;
    config.tools.todo = false;
}
struct Script {
    mode: &'static str,
    calls: AtomicUsize,
    requests: Mutex<Vec<(String, Vec<Value>)>>,
    started: Notify,
    release: Notify,
}
impl Script {
    fn new(mode: &'static str) -> Arc<Self> {
        Arc::new(Self {
            mode,
            calls: AtomicUsize::new(0),
            requests: Mutex::new(vec![]),
            started: Notify::new(),
            release: Notify::new(),
        })
    }
}
#[async_trait]
impl ModelProvider for Script {
    async fn complete(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        _tools: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .await
            .push((model.id.clone(), messages.to_vec()));
        self.started.notify_one();
        if self.mode == "gate" && n == 0 {
            tokio::select! {_=cancel.cancelled()=>return Err(ProviderError::new(ProviderErrorKind::Cancelled,"cancelled")),_=self.release.notified()=>{}}
        }
        if self.mode == "retry" && n == 0 || self.mode == "always_fail" {
            return Err(ProviderError::new(
                ProviderErrorKind::RateLimited,
                "fixture 429",
            ));
        }
        if self.mode == "auth" {
            return Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "fixture auth",
            ));
        }
        let last = messages
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .and_then(|m| m["content"].as_str())
            .unwrap_or("");
        if self.mode == "reasoning_overage" || self.mode == "massive_overage" {
            // 思考 token 不受 max_output_tokens 约束的网关行为：
            // 轻度超额应被思考预留吸收，重度超额仍中止派发。
            return Ok(ModelResponse {
                finish_reason: "stop".into(),
                message: json!({"role":"assistant","content":format!("fixture {last}")}),
                usage: Usage {
                    input_tokens: 50,
                    output_tokens: if self.mode == "reasoning_overage" {
                        750
                    } else {
                        5000
                    },
                },
            });
        }
        let message = if self.mode == "effect" && n == 0 {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"effect-1","type":"function","function":{"name":"effect","arguments":"{}"}}]})
        } else if self.mode == "skill" && n == 0 {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"load-1","type":"function","function":{"name":"load_skill","arguments":"{\"name\":\"locked\"}"}}]})
        } else if self.mode == "skill" && n == 1 {
            json!({"role":"assistant","content":null,"tool_calls":[{"id":"write-1","type":"function","function":{"name":"write_file","arguments":"{\"path\":\"forbidden.txt\",\"content\":\"not allowed\"}"}}]})
        } else {
            json!({"role":"assistant","content":format!("fixture {last}")})
        };
        Ok(ModelResponse {
            finish_reason: if message["tool_calls"].is_array() {
                "tool_calls"
            } else {
                "stop"
            }
            .into(),
            message,
            usage: Usage {
                input_tokens: 50,
                output_tokens: 20,
            },
        })
    }
    async fn complete_stream(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
        sender: mpsc::Sender<ProviderStreamEvent>,
    ) -> ProviderResult<ModelResponse> {
        let response = self
            .complete(model, messages, tools, cancel.clone())
            .await?;
        if self.mode == "secret" {
            for delta in ["安全文字 sk-", "abcdefgh", "12345678 ", "结束"] {
                sender
                    .send(ProviderStreamEvent::TextDelta {
                        delta: delta.into(),
                    })
                    .await
                    .map_err(|_| ProviderError::new(ProviderErrorKind::ConsumerClosed, "closed"))?;
                tokio::task::yield_now().await;
            }
        } else if let Some(text) = response.message["content"].as_str() {
            sender
                .send(ProviderStreamEvent::TextDelta { delta: text.into() })
                .await
                .map_err(|_| ProviderError::new(ProviderErrorKind::ConsumerClosed, "closed"))?;
        }
        Ok(response)
    }
}
#[tokio::test]
async fn retry_reroutes_with_reasons_and_does_not_hide_unknown_usage() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("retry");
    let runtime = AgentRuntime::builder(config(dir.path()))
        .provider(script.clone())
        .build()
        .await?;
    let result = runtime
        .run(request("hello"), CancellationToken::new())
        .await?;
    let requests = script.requests.lock().await;
    assert_eq!(requests[0].0, "first");
    assert_eq!(requests[1].0, "fallback");
    assert!(result.reserved_consumption.tokens > 70);
    let events = runtime.events.events(Some(&result.trace_id)).await?;
    assert!(
        events
            .iter()
            .any(|e| e.kind == "model.reroute" && e.payload["reason"] == "rate_limited")
    );
    assert!(
        events
            .iter()
            .any(|e| e.kind == "model.error" && e.payload["unknown_consumption_reserved"] == true)
    );
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn non_retryable_errors_keep_typed_metadata_and_never_dispatch_again() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("auth");
    let runtime = AgentRuntime::builder(config(dir.path()))
        .provider(script.clone())
        .build()
        .await?;
    let error = runtime
        .run(request("hello"), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        error.downcast_ref::<ProviderError>().unwrap().kind,
        ProviderErrorKind::Authentication
    );
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn task_and_total_token_ceilings_apply_before_dispatch_and_survive_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("always_fail");
    let mut c = config(dir.path());
    pin_minimal_tools(&mut c);
    c.budget.max_task_tokens = 1500;
    c.budget.max_total_tokens = Some(1500);
    c.models.iter_mut().for_each(|m| m.max_output_tokens = 1024);
    let runtime = AgentRuntime::builder(c.clone())
        .provider(script.clone())
        .build()
        .await?;
    let error = runtime
        .run(request("hello"), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("ceiling"));
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    runtime.shutdown().await?;
    drop(runtime);
    let runtime = AgentRuntime::builder(c)
        .provider(script.clone())
        .build()
        .await?;
    assert!(
        runtime
            .run(request("again"), CancellationToken::new())
            .await
            .unwrap_err()
            .to_string()
            .contains("ceiling")
    );
    assert_eq!(script.calls.load(Ordering::SeqCst), 1);
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn steering_and_followups_are_ordered_at_safe_boundaries_and_task_ids_match() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("gate");
    let runtime = AgentRuntime::builder(config(dir.path()))
        .provider(script.clone())
        .build()
        .await?;
    let running = {
        let r = runtime.clone();
        tokio::spawn(async move {
            r.run_with_task_id(
                "task-fixture".into(),
                request("initial"),
                CancellationToken::new(),
            )
            .await
        })
    };
    script.started.notified().await;
    assert_eq!(
        runtime
            .control("task-fixture", "steer one".into(), false)
            .await?
            .sequence,
        1
    );
    assert_eq!(
        runtime
            .control("task-fixture", "steer two".into(), false)
            .await?
            .sequence,
        2
    );
    assert_eq!(
        runtime
            .control("task-fixture", "follow one".into(), true)
            .await?
            .sequence,
        3
    );
    assert_eq!(
        runtime
            .control("task-fixture", "follow two".into(), true)
            .await?
            .sequence,
        4
    );
    assert!(
        runtime
            .objects
            .list()
            .iter()
            .any(|e| e.id == "task:task-fixture" && e.kind == ObjectKind::Task)
    );
    script.release.notify_one();
    let result = running.await??;
    assert_eq!(result.task_id, "task-fixture");
    assert_eq!(result.turns, 4);
    let session = runtime.session(&result.session_id).await?.unwrap();
    let users = session
        .messages
        .iter()
        .filter(|m| m["role"] == "user")
        .map(|m| m["content"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        users,
        [
            "initial",
            "steer one",
            "steer two",
            "follow one",
            "follow two"
        ]
    );
    assert!(
        runtime
            .control("task-fixture", "late".into(), false)
            .await
            .is_err()
    );
    assert!(
        !runtime
            .objects
            .list()
            .iter()
            .any(|e| e.id == "task:task-fixture")
    );
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn long_sessions_project_context_without_losing_journal_or_fork_parent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("echo");
    let mut c = config(dir.path());
    pin_minimal_tools(&mut c);
    c.models.truncate(1);
    c.models[0].context_window = 5000;
    c.models[0].max_output_tokens = 256;
    c.context.summary_max_bytes = 512;
    let runtime = AgentRuntime::builder(c)
        .provider(script.clone())
        .build()
        .await?;
    let mut id = None;
    for i in 0..60 {
        let mut req = request(&format!("第{i}轮 {}", "资料".repeat(20)));
        req.session_id = id.clone();
        id = Some(runtime.run(req, CancellationToken::new()).await?.session_id);
    }
    let id = id.unwrap();
    let parent = runtime.session(&id).await?.unwrap();
    assert_eq!(parent.messages.len(), 121);
    assert!(runtime.events.events(None).await?.iter().any(|e| {
        e.kind == "context.project"
            && e.payload["lineage"]["source_sha256"]
                .as_str()
                .is_some_and(|s| s.len() == 64)
    }));
    for (_, messages) in script.requests.lock().await.iter() {
        assert!(serde_json::to_vec(messages)?.len() < 5000);
        assert!(messages.iter().any(|m| {
            m["role"] == "system"
                && m["content"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("受保护的系统指令"))
        }));
    }
    let fork = runtime.fork(&id, 41, Some("分支".into())).await?;
    let mut req = request("fork change");
    req.session_id = Some(fork.id.clone());
    runtime.run(req, CancellationToken::new()).await?;
    assert_eq!(
        runtime.session(&id).await?.unwrap().messages,
        parent.messages
    );
    assert_eq!(
        runtime.session(&fork.id).await?.unwrap().parent_id,
        Some(id)
    );
    let bytes = tokio::fs::metadata(dir.path().join("data/sessions.jsonl"))
        .await?
        .len();
    assert!(bytes < 200_000, "incremental journal grew to {bytes}");
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn split_secrets_never_enter_live_snapshots_or_default_content_logs() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("secret");
    let runtime = AgentRuntime::builder(config(dir.path()))
        .provider(script)
        .build()
        .await?;
    let mut live = runtime.subscribe_progress();
    let result = runtime
        .run(request("hello"), CancellationToken::new())
        .await?;
    let mut snapshots = 0;
    while let Ok(p) = live.try_recv() {
        snapshots += 1;
        assert!(!p.text.contains("abcdefgh"));
        assert!(!p.text.contains("sk-"));
    }
    assert!(snapshots > 3);
    let events = runtime.events.events(Some(&result.trace_id)).await?;
    assert!(
        events
            .iter()
            .filter(|e| e.kind == "model.response")
            .all(|e| e.payload["message"].is_string())
    );
    runtime.shutdown().await?;
    Ok(())
}
struct Effect {
    count: Arc<AtomicUsize>,
    started: Arc<Notify>,
}
#[async_trait]
impl Object for Effect {
    fn envelope(&self) -> ObjectEnvelope {
        let mut e = ObjectEnvelope::new("tool:effect", ObjectKind::Tool, "fixture external effect");
        e.spec = json!({"type":"object","additionalProperties":false});
        e
    }
    async fn invoke(&self, _: &str, _: Value, _: InvocationContext) -> Result<Value> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        std::future::pending().await
    }
}
#[tokio::test]
async fn cancellation_after_external_effect_leaves_checkpoint_and_resume_does_not_replay()
-> Result<()> {
    let dir = tempfile::tempdir()?;
    let c = config(dir.path());
    let script = Script::new("effect");
    let runtime = AgentRuntime::builder(c.clone())
        .provider(script)
        .build()
        .await?;
    let count = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(Notify::new());
    runtime
        .register_tool(Arc::new(Effect {
            count: count.clone(),
            started: started.clone(),
        }))
        .await?;
    let cancel = CancellationToken::new();
    let running = {
        let r = runtime.clone();
        let c = cancel.clone();
        tokio::spawn(async move { r.run(request("run effect"), c).await })
    };
    started.notified().await;
    cancel.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), running)
        .await??
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let id = runtime.sessions().await?[0].id.clone();
    runtime.shutdown().await?;
    drop(runtime);
    let runtime = AgentRuntime::builder(c)
        .provider(Script::new("echo"))
        .build()
        .await?;
    let mut req = request("reconcile interrupted work");
    req.session_id = Some(id.clone());
    runtime.run(req, CancellationToken::new()).await?;
    let messages = runtime.session(&id).await?.unwrap().messages;
    assert!(messages.iter().any(|m| {
        m["role"] == "tool"
            && m["tool_call_id"] == "effect-1"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains("effect is unknown"))
    }));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn loaded_skill_permissions_remain_active_for_later_tools() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().join("skills/locked");
    tokio::fs::create_dir_all(&root).await?;
    tokio::fs::write(root.join("SKILL.md"),"---\nname: locked\ndescription: fixture policy\nallowed-tools: [load_skill, read_file]\n---\n不要写文件").await?;
    let mut c = config(dir.path());
    c.tools.write = true;
    c.skill_roots = vec![dir.path().join("skills")];
    let script = Script::new("skill");
    let runtime = AgentRuntime::builder(c).provider(script).build().await?;
    let result = runtime
        .run(request("use skill"), CancellationToken::new())
        .await?;
    assert!(!dir.path().join("forbidden.txt").exists());
    let session = runtime.session(&result.session_id).await?.unwrap();
    assert!(session.messages.iter().any(|m| {
        m["role"] == "tool"
            && m["content"]
                .as_str()
                .is_some_and(|s| s.contains("allowed-tools"))
    }));
    assert!(
        runtime
            .events
            .events(None)
            .await?
            .iter()
            .any(|e| e.kind == "skill.load")
    );
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn sqlite_runtime_sessions_traces_and_budget_survive_restart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut c = config(dir.path());
    c.storage.backend = "sqlite".into();
    let runtime = AgentRuntime::builder(c.clone())
        .provider(Script::new("echo"))
        .build()
        .await?;
    let result = runtime
        .run(request("persist sqlite"), CancellationToken::new())
        .await?;
    runtime.shutdown().await?;
    drop(runtime);
    let runtime = AgentRuntime::builder(c)
        .provider(Script::new("echo"))
        .build()
        .await?;
    assert!(runtime.session(&result.session_id).await?.is_some());
    assert!(
        runtime
            .events
            .events(Some(&result.trace_id))
            .await?
            .iter()
            .any(|e| e.kind == "task.done")
    );
    runtime.shutdown().await?;
    Ok(())
}
#[tokio::test]
async fn reasoning_overage_is_absorbed_and_massive_overage_warns_but_completes() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let script = Script::new("reasoning_overage");
    let mut c = config(dir.path());
    pin_minimal_tools(&mut c);
    c.models
        .iter_mut()
        .for_each(|m| m.supports_reasoning = true);
    let runtime = AgentRuntime::builder(c.clone())
        .provider(script.clone())
        .build()
        .await?;
    let result = runtime
        .run(request("hello"), CancellationToken::new())
        .await?;
    assert_eq!(result.status, "done");
    runtime.shutdown().await?;
    drop(runtime);
    // 重度超额：真实账目持久化、发出 budget.overage 告警，任务不丢弃已完成轮次；
    // 硬上限仍由下一次派发前的 ceiling 检查把关。
    let script = Script::new("massive_overage");
    let runtime = AgentRuntime::builder(c)
        .provider(script.clone())
        .build()
        .await?;
    let result = runtime
        .run(request("hello"), CancellationToken::new())
        .await?;
    assert_eq!(result.status, "done");
    let events = runtime.events.events(Some(&result.trace_id)).await?;
    assert!(events.iter().any(|e| e.kind == "budget.overage"));
    runtime.shutdown().await?;
    Ok(())
}
