use anyhow::Result;
use async_trait::async_trait;
use rpi_core::{
    config::RuntimeConfig,
    mcp::McpRegistry,
    object::{InvocationContext, Object, ObjectEnvelope, ObjectKind},
    skills::SkillRegistry,
    tools::{NetworkPolicy, NetworkPolicyConfig, ToolAuthorization, ToolPolicy, ToolRegistry},
};
use serde_json::{Value, json};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio_util::sync::CancellationToken;
fn write_skill(root: &Path, folder: &str, name: &str, policy: &str, body: &str) -> Result<()> {
    let dir = root.join(folder);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: test skill\n{policy}\n---\n{body}"),
    )?;
    Ok(())
}
fn context() -> InvocationContext {
    InvocationContext {
        session_id: "s".into(),
        trace_id: "t".into(),
        span_id: "p".into(),
        cancel: CancellationToken::new(),
    }
}
#[tokio::test]
async fn ignores_refresh_conflicts_resources_and_permissions_are_enforced() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let high = temp.path().join("high");
    let low = temp.path().join("low");
    write_skill(
        &high,
        "keep",
        "shared",
        "allowed-tools: [read_file, 'mcp__safe__*']",
        "first",
    )?;
    write_skill(&high, "hidden", "hidden", "", "ignored")?;
    write_skill(
        &low,
        "fallback",
        "shared",
        "allowed-tools: [read_file]",
        "fallback",
    )?;
    std::fs::write(high.join(".skillignore"), "hidden/\n")?;
    std::fs::write(high.join("keep").join("data.txt"), "inert 中文资源")?;
    std::fs::write(temp.path().join("secret.txt"), "outside")?;
    let registry = SkillRegistry::discover(&[high.clone(), low.clone()])?;
    let observer = registry.clone();
    assert_eq!(registry.list().len(), 1);
    assert!(
        registry
            .diagnostics()
            .iter()
            .any(|line| line.contains("duplicate"))
    );
    registry.enforce_allowed_tools(&["shared".into()], "tool:read_file")?;
    registry.enforce_allowed_tools(&["shared".into()], "mcp__safe__echo")?;
    assert!(
        registry
            .enforce_allowed_tools(&["shared".into()], "shell")
            .is_err()
    );
    let skill = registry.get("shared").unwrap();
    assert_eq!(
        skill.read_resource("data.txt", 128).await?["content"],
        "inert 中文资源"
    );
    assert!(skill.read_resource("../../secret.txt", 128).await.is_err());
    assert!(skill.read_resource("data.txt", 1).await.is_err());
    let digest = skill.content_sha256.clone();
    write_skill(&high, "keep", "shared", "allowed-tools: []", "changed")?;
    let update = registry.refresh()?;
    assert_eq!(update.changed, vec!["shared"]);
    assert_ne!(observer.get("shared").unwrap().content_sha256, digest);
    assert!(
        observer
            .enforce_allowed_tools(&["shared".into()], "read_file")
            .is_err()
    );
    std::fs::remove_file(high.join("keep").join("SKILL.md"))?;
    registry.refresh()?;
    assert_eq!(
        observer.get("shared").unwrap().path,
        low.join("fallback/SKILL.md")
    );
    std::fs::remove_file(low.join("fallback/SKILL.md"))?;
    assert_eq!(registry.refresh()?.removed, vec!["shared"]);
    assert!(
        observer
            .enforce_allowed_tools(&["shared".into()], "read_file")
            .is_err()
    );
    Ok(())
}
#[test]
fn invalid_permission_metadata_is_diagnostic_instead_of_authorizing_tools() -> Result<()> {
    let temp = tempfile::tempdir()?;
    write_skill(temp.path(), "bad", "bad", "allowed-tools: [42]", "body")?;
    let registry = SkillRegistry::discover(&[temp.path().to_owned()])?;
    assert!(registry.list().is_empty());
    assert!(
        registry
            .diagnostics()
            .iter()
            .any(|line| line.contains("allowed-tools"))
    );
    Ok(())
}
struct CountingTool(Arc<AtomicUsize>);
#[async_trait]
impl Object for CountingTool {
    fn envelope(&self) -> ObjectEnvelope {
        let mut envelope = ObjectEnvelope::new("tool:counted", ObjectKind::Tool, "schema fixture");
        envelope.spec = json!({"type":"object","properties":{"count":{"type":"integer","minimum":0}},"required":["count"],"additionalProperties":false});
        envelope.capabilities.insert(
            "output_schema".into(),
            json!({"type":"object","properties":{"ok":{"const":true}},"required":["ok"]}),
        );
        envelope
    }
    async fn invoke(&self, _method: &str, input: Value, _ctx: InvocationContext) -> Result<Value> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"ok":input["count"]!=13}))
    }
}
struct DenyPolicy;
#[async_trait]
impl ToolPolicy for DenyPolicy {
    async fn authorize(&self, request: &ToolAuthorization) -> Result<()> {
        anyhow::bail!("policy denied {}", request.name)
    }
}
#[tokio::test]
async fn schema_and_custom_policy_reject_before_tool_side_effects() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let config = RuntimeConfig {
        workspace: temp.path().to_owned(),
        ..Default::default()
    };
    let registry = ToolRegistry::new(&config, SkillRegistry::default(), McpRegistry::default())?;
    let count = Arc::new(AtomicUsize::new(0));
    registry.register(Arc::new(CountingTool(count.clone())))?;
    assert!(
        registry
            .call("counted", json!({"count":"wrong"}), context())
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert!(
        registry
            .call("counted", json!({"count":1,"extra":true}), context())
            .await
            .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
    assert_eq!(
        registry
            .call("counted", json!({"count":2}), context())
            .await?["ok"],
        true
    );
    assert!(
        registry
            .call("counted", json!({"count":13}), context())
            .await
            .unwrap_err()
            .to_string()
            .contains("output")
    );
    registry.set_policy(Arc::new(DenyPolicy));
    assert!(
        registry
            .call("counted", json!({"count":2}), context())
            .await
            .unwrap_err()
            .to_string()
            .contains("policy denied")
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
    Ok(())
}
#[tokio::test]
async fn registry_calls_apply_active_skill_policy_even_for_builtin_tools() -> Result<()> {
    let temp = tempfile::tempdir()?;
    write_skill(
        temp.path(),
        "restricted",
        "restricted",
        "allowed-tools: []",
        "read nothing",
    )?;
    std::fs::write(temp.path().join("test.txt"), "content")?;
    let skills = SkillRegistry::discover(&[temp.path().to_owned()])?;
    let registry = ToolRegistry::new(
        &RuntimeConfig {
            workspace: temp.path().to_owned(),
            ..Default::default()
        },
        skills,
        McpRegistry::default(),
    )?;
    assert!(
        registry
            .call_authorized(
                "read_file",
                json!({"path":"test.txt"}),
                context(),
                &["restricted".into()]
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("allowed-tools")
    );
    assert_eq!(
        registry
            .call("read_file", json!({"path":"test.txt"}), context())
            .await?["content"],
        "content"
    );
    Ok(())
}
#[tokio::test]
async fn network_policy_checks_literals_allowlist_and_plaintext_boundaries() -> Result<()> {
    let policy = NetworkPolicyConfig::default();
    policy
        .authorize_url(&reqwest::Url::parse("http://127.0.0.1:399")?)
        .await?;
    assert!(
        policy
            .authorize_url(&reqwest::Url::parse("https://10.0.0.1")?)
            .await
            .is_err()
    );
    assert!(
        policy
            .authorize_url(&reqwest::Url::parse("http://8.8.8.8")?)
            .await
            .is_err()
    );
    assert!(
        policy
            .authorize_url(&reqwest::Url::parse("https://secret@127.0.0.1")?)
            .await
            .is_err()
    );
    let deny = NetworkPolicyConfig {
        allow_loopback: false,
        ..Default::default()
    };
    assert!(
        deny.authorize_url(&reqwest::Url::parse("http://127.0.0.1")?)
            .await
            .is_err()
    );
    Ok(())
}
