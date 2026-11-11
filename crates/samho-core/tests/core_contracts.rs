//! Behavioral contracts at public runtime seams, independent of real paid models.
use anyhow::Result;
use async_trait::async_trait;
use samho_core::config::{RoutingConfig, RoutingRule};
use samho_core::events::{EventLog, Redactor, TraceEvent};
use samho_core::model::{ModelDescriptor, ModelRouter, ModelTier, ProviderKind, TaskProfile};
use samho_core::object::{InvocationContext, Object, ObjectEnvelope, ObjectKind, ObjectRegistry};
use samho_core::skills::SkillRegistry;
use samho_core::{AgentRuntime, RuntimeConfig, TaskRequest};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct LifecycleObject {
    envelope: ObjectEnvelope,
    history: Arc<Mutex<Vec<String>>>,
    fail: bool,
}
#[async_trait]
impl Object for LifecycleObject {
    fn envelope(&self) -> ObjectEnvelope {
        self.envelope.clone()
    }
    async fn invoke(
        &self,
        _method: &str,
        _input: Value,
        _context: InvocationContext,
    ) -> Result<Value> {
        anyhow::bail!("not an invokable fixture")
    }
    async fn activate(&self) -> Result<()> {
        self.history
            .lock()
            .unwrap()
            .push(format!("start:{}", self.envelope.id));
        if self.fail {
            anyhow::bail!("fixture activation refused");
        }
        Ok(())
    }
    async fn dispose(&self) -> Result<()> {
        self.history
            .lock()
            .unwrap()
            .push(format!("stop:{}", self.envelope.id));
        Ok(())
    }
}
fn lifecycle(
    id: &str,
    dependencies: &[&str],
    history: &Arc<Mutex<Vec<String>>>,
    fail: bool,
) -> Arc<dyn Object> {
    let mut envelope = ObjectEnvelope::new(id, ObjectKind::Plugin, "fixture lifecycle");
    envelope.dependencies = dependencies.iter().map(|id| id.to_string()).collect();
    Arc::new(LifecycleObject {
        envelope,
        history: history.clone(),
        fail,
    })
}
#[tokio::test]
async fn objects_activate_dependencies_once_and_dispose_in_reverse_order() -> Result<()> {
    let registry = ObjectRegistry::default();
    let history = Arc::new(Mutex::new(Vec::new()));
    // Lexical order deliberately opposes dependency order.
    registry.register(lifecycle("a-agent", &["z-model"], &history, false))?;
    registry.register(lifecycle("z-model", &["m-storage"], &history, false))?;
    registry.register(lifecycle("m-storage", &[], &history, false))?;
    assert!(
        registry
            .register(lifecycle("m-storage", &[], &history, false))
            .is_err()
    );
    registry.activate().await?;
    registry.activate().await?;
    assert_eq!(
        *history.lock().unwrap(),
        ["start:m-storage", "start:z-model", "start:a-agent"]
    );
    registry.dispose().await?;
    registry.dispose().await?;
    assert_eq!(
        *history.lock().unwrap(),
        [
            "start:m-storage",
            "start:z-model",
            "start:a-agent",
            "stop:a-agent",
            "stop:z-model",
            "stop:m-storage"
        ]
    );
    Ok(())
}
#[tokio::test]
async fn objects_fail_before_activation_for_missing_or_cyclic_dependencies() -> Result<()> {
    let history = Arc::new(Mutex::new(Vec::new()));
    let registry = ObjectRegistry::default();
    registry.register(lifecycle("consumer", &["missing"], &history, false))?;
    assert!(
        registry
            .activate()
            .await
            .unwrap_err()
            .to_string()
            .contains("missing Object dependency")
    );
    assert!(history.lock().unwrap().is_empty());
    let registry = ObjectRegistry::default();
    registry.register(lifecycle("one", &["two"], &history, false))?;
    registry.register(lifecycle("two", &["one"], &history, false))?;
    assert!(
        registry
            .activate()
            .await
            .unwrap_err()
            .to_string()
            .contains("cyclic Object dependency")
    );
    assert!(history.lock().unwrap().is_empty());
    Ok(())
}
#[tokio::test]
async fn objects_rollback_successful_dependencies_on_failed_activation() -> Result<()> {
    let history = Arc::new(Mutex::new(Vec::new()));
    let registry = ObjectRegistry::default();
    registry.register(lifecycle("agent", &["model"], &history, true))?;
    registry.register(lifecycle("model", &["storage"], &history, false))?;
    registry.register(lifecycle("storage", &[], &history, false))?;
    assert!(
        registry
            .activate()
            .await
            .unwrap_err()
            .to_string()
            .contains("activate Object agent")
    );
    assert_eq!(
        *history.lock().unwrap(),
        [
            "start:storage",
            "start:model",
            "start:agent",
            "stop:model",
            "stop:storage"
        ]
    );
    registry.dispose().await?;
    assert_eq!(history.lock().unwrap().len(), 5);
    Ok(())
}

fn models() -> Vec<ModelDescriptor> {
    [
        ModelTier::Nano,
        ModelTier::Fast,
        ModelTier::Balanced,
        ModelTier::Reasoning,
        ModelTier::Frontier,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, tier)| ModelDescriptor::demo(&format!("model-{index}"), tier))
    .collect()
}
#[test]
fn routing_uses_all_intelligence_levels_and_respects_prompt_rule_order() -> Result<()> {
    let router = ModelRouter::new(models(), RoutingConfig::default());
    for (complexity, expected) in [
        (0, ModelTier::Nano),
        (1, ModelTier::Nano),
        (2, ModelTier::Fast),
        (3, ModelTier::Fast),
        (4, ModelTier::Balanced),
        (5, ModelTier::Balanced),
        (6, ModelTier::Reasoning),
        (7, ModelTier::Reasoning),
        (8, ModelTier::Frontier),
        (10, ModelTier::Frontier),
    ] {
        let route = router.route(&TaskProfile::classify(complexity))?;
        assert_eq!(route.model.tier, expected);
        assert!(!route.rationale.is_empty());
    }
    for (prompt, expected) in [
        ("hello", ModelTier::Nano),
        ("翻译 this sentence", ModelTier::Fast),
        ("debug a parser", ModelTier::Reasoning),
        ("design architecture and debug it", ModelTier::Frontier),
    ] {
        let classified = router.classify_prompt(prompt, &TaskProfile::default());
        assert_eq!(router.route(&classified)?.model.tier, expected);
    }
    let policy = RoutingConfig {
        rules: vec![RoutingRule {
            keywords: vec!["invoice".into()],
            complexity: 3,
        }],
        ..Default::default()
    };
    let router = ModelRouter::new(models(), policy);
    assert_eq!(
        router
            .classify_prompt("INVOICE check", &TaskProfile::default())
            .complexity,
        3
    );
    let manual = TaskProfile::classify(5);
    assert_eq!(
        router.classify_prompt("architecture", &manual).complexity,
        5
    );
    Ok(())
}
#[test]
fn routing_enforces_tool_vision_reasoning_context_budget_latency_and_specialty() -> Result<()> {
    let policy = RoutingConfig {
        output_reserve_tokens: 128,
        ..Default::default()
    };
    let mut base = ModelDescriptor::demo("balanced", ModelTier::Balanced);
    base.supports_tools = false;
    base.context_window = 8_000;
    base.max_output_tokens = 512;
    let mut capable = ModelDescriptor::demo("capable", ModelTier::Frontier);
    capable.supports_vision = true;
    capable.supports_reasoning = true;
    capable.specialties = vec!["code".into()];
    capable.context_window = 32_000;
    capable.max_output_tokens = 512;
    capable.input_usd_per_million = 2.;
    capable.output_usd_per_million = 2.;
    capable.latency_ms = 500;
    let router = ModelRouter::new(vec![base, capable], policy);
    for task in [
        TaskProfile {
            requires_tools: true,
            ..TaskProfile::classify(4)
        },
        TaskProfile {
            requires_vision: true,
            ..TaskProfile::classify(4)
        },
        TaskProfile {
            requires_reasoning: true,
            ..TaskProfile::classify(4)
        },
        TaskProfile {
            context_tokens: 9_000,
            ..TaskProfile::classify(4)
        },
        TaskProfile {
            specialty: Some("code".into()),
            ..TaskProfile::classify(4)
        },
    ] {
        let route = router.route(&task)?;
        assert_eq!(route.model.id, "capable");
        assert!(
            route
                .rejected
                .iter()
                .any(|rejection| rejection.starts_with("balanced:"))
        );
    }
    let budget = router.route(&TaskProfile {
        context_tokens: 1_000,
        budget_usd: Some(0.0001),
        ..TaskProfile::classify(9)
    })?;
    assert_eq!(budget.model.id, "balanced");
    assert!(budget.estimated_cost_usd <= 0.0001);
    assert!(
        budget
            .rejected
            .iter()
            .any(|reason| reason.contains("budget exceeded"))
    );
    assert_eq!(
        router
            .route(&TaskProfile {
                max_latency_ms: Some(100),
                ..TaskProfile::classify(9)
            })?
            .model
            .id,
        "balanced"
    );
    for task in [
        TaskProfile {
            requires_vision: true,
            max_latency_ms: Some(100),
            ..TaskProfile::classify(9)
        },
        TaskProfile {
            requires_vision: true,
            budget_usd: Some(0.),
            ..TaskProfile::classify(9)
        },
        TaskProfile {
            context_tokens: 40_000,
            ..TaskProfile::classify(9)
        },
        TaskProfile {
            specialty: Some("unavailable".into()),
            ..TaskProfile::classify(4)
        },
    ] {
        assert!(router.route(&task).is_err());
    }
    Ok(())
}
#[test]
fn routing_manual_overrides_do_not_bypass_constraints_and_invalid_tasks_fail() -> Result<()> {
    let router = ModelRouter::new(models(), RoutingConfig::default());
    assert_eq!(
        router
            .route(&TaskProfile {
                model: Some("model-0".into()),
                ..TaskProfile::classify(10)
            })?
            .model
            .id,
        "model-0"
    );
    assert_eq!(
        router
            .route(&TaskProfile {
                tier: Some(ModelTier::Frontier),
                ..TaskProfile::classify(0)
            })?
            .model
            .tier,
        ModelTier::Frontier
    );
    assert!(
        router
            .route(&TaskProfile {
                model: Some("model-0".into()),
                requires_vision: true,
                ..TaskProfile::classify(10)
            })
            .is_err()
    );
    assert!(
        router
            .route(&TaskProfile {
                model: Some("unknown".into()),
                ..TaskProfile::classify(3)
            })
            .is_err()
    );
    for task in [
        TaskProfile::classify(11),
        TaskProfile {
            budget_usd: Some(-1.),
            ..TaskProfile::classify(4)
        },
        TaskProfile {
            budget_usd: Some(f64::NAN),
            ..TaskProfile::classify(4)
        },
    ] {
        assert!(router.route(&task).is_err());
    }
    Ok(())
}

#[test]
fn runtime_config_default_and_custom_toml_roundtrip_and_validate() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("samho.toml");
    let defaults = RuntimeConfig::default();
    defaults.write_toml(&path)?;
    let decoded: RuntimeConfig = toml::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(
        serde_json::to_value(&decoded)?,
        serde_json::to_value(&defaults)?
    );
    decoded.validate()?;
    let mut custom = defaults;
    custom.workspace = directory.path().join("工作区");
    custom.data_dir = directory.path().join("data");
    custom.http_bind = "127.0.0.1:399".into();
    custom.system_prompt = "custom\nline two".into();
    custom.max_turns = 9;
    custom.max_concurrent_tasks = 3;
    custom.request_timeout_ms = 4_000;
    custom.max_tool_output_bytes = 8_192;
    custom.log_content = true;
    custom.durable_logs = false;
    custom.skill_roots = vec![PathBuf::from("skills")];
    custom.tools.write = true;
    custom.tools.shell = true;
    custom.tools.shell_timeout_ms = 1_500;
    custom.models = vec![ModelDescriptor {
        id: "local".into(),
        model: "test-model".into(),
        provider: ProviderKind::ChatCompletions,
        base_url: Some("http://localhost:398/v1".into()),
        api_key_env: Some("TEST_API_KEY".into()),
        reasoning_effort: Some("high".into()),
        extra_body: json!({"temperature":0.25,"metadata":{"run":"audit"}}),
        ..ModelDescriptor::demo("local", ModelTier::Reasoning)
    }];
    custom.mcp_servers = vec![samho_core::mcp::McpServerConfig {
        name: "mcp".into(),
        command: Some("test-command".into()),
        args: vec!["arg".into()],
        ..Default::default()
    }];
    custom.write_toml(&path)?;
    let decoded: RuntimeConfig = toml::from_str(&std::fs::read_to_string(&path)?)?;
    assert_eq!(
        serde_json::to_value(&decoded)?,
        serde_json::to_value(&custom)?
    );
    decoded.validate()?;
    Ok(())
}
#[test]
fn invalid_configuration_is_rejected_without_silent_defaulting() {
    assert!(toml::from_str::<RuntimeConfig>("misspelled_option = true").is_err());
    let mut variants = Vec::new();
    let mut value = RuntimeConfig::default();
    value.models.clear();
    variants.push(value);
    let value = RuntimeConfig {
        max_concurrent_tasks: 0,
        ..RuntimeConfig::default()
    };
    variants.push(value);
    let value = RuntimeConfig {
        request_timeout_ms: 0,
        ..RuntimeConfig::default()
    };
    variants.push(value);
    let mut value = RuntimeConfig::default();
    value.models[0].max_output_tokens = value.models[0].context_window + 1;
    variants.push(value);
    let mut value = RuntimeConfig::default();
    value.models.push(value.models[0].clone());
    variants.push(value);
    let mut value = RuntimeConfig::default();
    value.models[0].input_usd_per_million = -1.;
    variants.push(value);
    let mut value = RuntimeConfig::default();
    value.models[0].provider = ProviderKind::ChatCompletions;
    value.models[0].base_url = None;
    variants.push(value);
    let mut value = RuntimeConfig::default();
    value.routing.cost_weight = f64::INFINITY;
    variants.push(value);
    for config in variants {
        assert!(config.validate().is_err());
    }
}

fn skill(
    path: &Path,
    name: &str,
    description: &str,
    disabled: bool,
    body: &str,
) -> Result<PathBuf> {
    let path = path.join(name).join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(
        &path,
        format!(
            "---\nname: {name}\ndescription: {description}\ndisable-model-invocation: {disabled}\nallowed-tools: [read_file]\nmetadata:\n  domain: contracts\n---\n{body}\n"
        ),
    )?;
    Ok(path)
}
#[tokio::test]
async fn skills_parse_real_yaml_resolve_duplicates_and_load_latest_body_lazily() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let high = directory.path().join("first");
    let low = directory.path().join("second");
    let primary = skill(
        &high,
        "multiline",
        "|\n  A multiline description\n  with a second line",
        false,
        "OLD BODY",
    )?;
    skill(
        &low,
        "multiline",
        "'Ignored duplicate'",
        false,
        "DUPLICATE BODY",
    )?;
    skill(
        &high,
        "quoted",
        "\"A colon: quoted YAML and unicode 中文\"",
        false,
        "quoted instruction",
    )?;
    skill(
        &high,
        "manual",
        "'Only explicit user use'",
        true,
        "manual instruction",
    )?;
    let registry = SkillRegistry::discover(&[high.clone(), low])?;
    assert_eq!(registry.list().len(), 3);
    assert!(
        registry
            .diagnostics()
            .iter()
            .any(|line| line.contains("duplicate skill multiline"))
    );
    let metadata = registry.get("multiline").unwrap();
    assert_eq!(metadata.path, primary);
    assert!(metadata.description.contains("with a second line"));
    assert_eq!(metadata.metadata["domain"], "contracts");
    assert_eq!(
        registry.get("quoted").unwrap().description,
        "A colon: quoted YAML and unicode 中文"
    );
    skill(
        &high,
        "multiline",
        "'Updated description'",
        false,
        "FRESH BODY AFTER DISCOVERY",
    )?;
    let instructions = registry.load("multiline", false).await?;
    assert!(instructions.contains("FRESH BODY AFTER DISCOVERY"));
    assert!(!instructions.contains("OLD BODY"));
    assert!(!instructions.contains("DUPLICATE BODY"));
    assert!(registry.load("manual", false).await.is_err());
    assert!(
        registry
            .load("manual", true)
            .await?
            .contains("manual instruction")
    );
    assert!(registry.load("unknown", true).await.is_err());
    Ok(())
}
#[tokio::test]
async fn explicit_task_skill_load_is_recorded_and_instruction_enters_session() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("skills");
    skill(
        &root,
        "manual",
        "'Explicit-only instructions'",
        true,
        "UNIQUE CONTRACT INSTRUCTION",
    )?;
    let config = RuntimeConfig {
        workspace: directory.path().to_path_buf(),
        data_dir: directory.path().join("data"),
        skill_roots: vec![root],
        log_content: true,
        ..Default::default()
    };
    let runtime = AgentRuntime::new(config).await?;
    let result = runtime
        .run(
            TaskRequest {
                session_id: None,
                prompt: "hello".into(),
                profile: TaskProfile::classify(1),
                skills: vec!["manual".into()],
            },
            CancellationToken::new(),
        )
        .await?;
    let session = runtime.session(&result.session_id).await?.unwrap();
    assert!(session.messages.iter().any(|message| {
        message["content"]
            .as_str()
            .is_some_and(|text| text.contains("UNIQUE CONTRACT INSTRUCTION"))
    }));
    let events = runtime.events.events(Some(&result.trace_id)).await?;
    assert!(
        events
            .iter()
            .any(|event| event.kind == "skill.load" && event.payload["name"] == "manual")
    );
    runtime.shutdown().await?;
    Ok(())
}

fn event(index: usize) -> TraceEvent {
    TraceEvent::new(
        if index.is_multiple_of(2) {
            "even"
        } else {
            "odd"
        },
        "session",
        &format!("span-{index}"),
        "contract",
        json!({"index":index}),
    )
}
#[tokio::test]
async fn event_append_concurrency_preserves_persisted_and_broadcast_sequence() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let log = EventLog::open(
        directory.path().join("events.jsonl"),
        false,
        Redactor::new(vec![]),
    )
    .await?;
    let mut receiver = log.subscribe();
    assert!(log.is_empty().await);
    let tasks = (0..64)
        .map(|index| {
            let log = log.clone();
            tokio::spawn(async move { log.append(event(index)).await })
        })
        .collect::<Vec<_>>();
    let mut sequences = Vec::new();
    for task in tasks {
        sequences.push(task.await??);
    }
    sequences.sort_unstable();
    assert_eq!(sequences, (1..=64).collect::<Vec<_>>());
    for sequence in 1..=64 {
        assert_eq!(receiver.recv().await?.sequence, sequence);
    }
    let events = log.events(None).await?;
    assert_eq!(events.len(), 64);
    assert_eq!(log.len().await, 64);
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1..=64).collect::<Vec<_>>()
    );
    assert_eq!(log.events(Some("even")).await?.len(), 32);
    Ok(())
}
#[tokio::test]
async fn jsonl_reopen_discards_only_unterminated_tail_and_keeps_next_sequence() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("events.jsonl");
    let log = EventLog::open(&path, false, Redactor::new(vec![])).await?;
    log.append(event(0)).await?;
    drop(log);
    let mut bytes = std::fs::read(&path)?;
    let valid_length = bytes.len();
    bytes.extend_from_slice(b"{\"sequence\":2,\"unfinished\"");
    std::fs::write(&path, bytes)?;
    let reopened = EventLog::open(&path, false, Redactor::new(vec![])).await?;
    assert_eq!(reopened.len().await, 1);
    assert_eq!(std::fs::metadata(&path)?.len(), valid_length as u64);
    assert_eq!(reopened.events(None).await?.len(), 1);
    assert_eq!(reopened.append(event(1)).await?, 2);
    assert_eq!(reopened.len().await, 2);
    let exported = directory.path().join("export.jsonl");
    reopened.export_jsonl(&exported).await?;
    assert_eq!(std::fs::read(&exported)?, std::fs::read(&path)?);
    Ok(())
}
#[tokio::test]
async fn jsonl_interior_corruption_and_non_monotonic_records_are_not_silently_repaired()
-> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("events.jsonl");
    let log = EventLog::open(&path, false, Redactor::new(vec![])).await?;
    log.append(event(0)).await?;
    drop(log);
    let valid = std::fs::read(&path)?;
    let mut corrupt = valid.clone();
    corrupt.extend_from_slice(b"{not JSON}\npartial tail");
    std::fs::write(&path, &corrupt)?;
    assert!(
        EventLog::open(&path, false, Redactor::new(vec![]))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path)?, corrupt);
    let mut duplicate = valid.clone();
    duplicate.extend_from_slice(&valid);
    std::fs::write(&path, &duplicate)?;
    assert!(
        EventLog::open(&path, false, Redactor::new(vec![]))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&path)?, duplicate);
    Ok(())
}
#[tokio::test]
async fn trace_redaction_applies_before_persistence_broadcast_and_export() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("events.jsonl");
    let log = EventLog::open(
        &path,
        false,
        Redactor::new(vec!["private-value-123".into()]),
    )
    .await?;
    let mut receiver = log.subscribe();
    let payload = json!({"message":"text private-value-123 Bearer eyJopaque123 sk-abcdefgh123456", "nested":[{"PASSWORD":"short", "safe":25}],"authorization":"Bearer secret", "normal":"ordinary text"});
    log.append(TraceEvent::new("t", "s", "span", "secret.fixture", payload))
        .await?;
    let event = receiver.recv().await?;
    assert_eq!(event.payload["nested"][0]["PASSWORD"], "[REDACTED]");
    assert_eq!(event.payload["nested"][0]["safe"], 25);
    assert_eq!(event.payload["normal"], "ordinary text");
    let text = std::fs::read_to_string(&path)?;
    for secret in [
        "private-value-123",
        "eyJopaque123",
        "sk-abcdefgh123456",
        "Bearer secret",
        "\"short\"",
    ] {
        assert!(!text.contains(secret), "leaked {secret}");
    }
    assert!(text.contains("[REDACTED]"));
    let export = directory.path().join("export.jsonl");
    log.export_jsonl(&export).await?;
    assert_eq!(text, std::fs::read_to_string(export)?);
    Ok(())
}
#[tokio::test]
async fn bounded_event_stream_reports_lag_instead_of_unbounded_buffering() -> Result<()> {
    let log = EventLog::memory();
    let mut receiver = log.subscribe();
    for index in 0..300 {
        log.append(event(index)).await?;
    }
    assert!(matches!(
        receiver.recv().await,
        Err(tokio::sync::broadcast::error::RecvError::Lagged(44))
    ));
    assert_eq!(receiver.recv().await?.sequence, 45);
    assert_eq!(log.events(None).await?.len(), 300);
    Ok(())
}
