#![allow(deprecated)]
use anyhow::{Context, Result};
use async_trait::async_trait;
use rmcp::model::{
    CreateMessageRequestParams, CreateMessageResult, ElicitRequestParams, ElicitResult,
};
use rpi_core::mcp::{
    McpCallbackContext, McpClientHooks, McpClientOptions, McpError, McpRegistry, McpServerConfig,
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
fn config() -> McpServerConfig {
    McpServerConfig {
        name: "optional".into(),
        command: Some(std::env::var("RPI_TEST_PYTHON").unwrap_or_else(|_| "python".into())),
        args: vec![
            "-u".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/mcp_optional.py")
                .to_string_lossy()
                .into_owned(),
        ],
        timeout_ms: 1000,
        ..Default::default()
    }
}
fn cancel() -> CancellationToken {
    CancellationToken::new()
}
struct Hooks {
    elicitation: Value,
}
#[async_trait]
impl McpClientHooks for Hooks {
    async fn sample(
        &self,
        params: CreateMessageRequestParams,
        context: McpCallbackContext,
    ) -> std::result::Result<CreateMessageResult, rmcp::ErrorData> {
        assert_eq!(context.server, "optional");
        assert_eq!(params.max_tokens, 16);
        Ok(serde_json::from_value(json!({"role":"assistant","content":{"type":"text","text":"sampled locally"},"model":"fixture","stopReason":"endTurn"})).unwrap())
    }
    async fn elicit(
        &self,
        _: ElicitRequestParams,
        _: McpCallbackContext,
    ) -> std::result::Result<ElicitResult, rmcp::ErrorData> {
        Ok(serde_json::from_value(self.elicitation.clone()).unwrap())
    }
    async fn task_input(
        &self,
        requests: Value,
        _: McpCallbackContext,
    ) -> std::result::Result<Value, rmcp::ErrorData> {
        assert!(requests["question"].is_object());
        Ok(json!({"question":{"result":{"action":"accept","content":{"answer":"local"}}}}))
    }
}
fn hooks(value: Value) -> Arc<dyn McpClientHooks> {
    Arc::new(Hooks { elicitation: value })
}
async fn call(registry: &McpRegistry, name: &str, args: Value) -> Result<Value> {
    registry
        .call(&format!("mcp__optional__{name}"), args, cancel())
        .await
        .with_context(|| format!("fixture tool {name}"))
}
#[tokio::test]
async fn optional_callbacks_are_absent_by_default_and_fail_explicitly() -> Result<()> {
    let registry = McpRegistry::connect(&[config()]).await?;
    let info = call(&registry, "inspect_caps", json!({})).await?;
    for name in ["sampling", "elicitation", "roots"] {
        assert!(info["structuredContent"]["client"]["capabilities"][name].is_null());
    }
    for name in ["ask_sampling", "ask_elicitation", "ask_roots"] {
        assert_eq!(
            call(&registry, name, json!({})).await?["structuredContent"]["error"]["code"],
            -32601
        );
    }
    let mut enabled = config();
    enabled.client.enable_sampling = true;
    let rejected = McpRegistry::connect(&[enabled]).await?;
    assert_eq!(
        rejected.status().await["servers"][0]["error"]["kind"],
        "configuration"
    );
    registry.close().await?;
    rejected.close().await?;
    Ok(())
}
#[tokio::test]
async fn sampling_elicitation_roots_completion_and_resource_notifications_are_real() -> Result<()> {
    let root = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let mut enabled = config();
    enabled.client = McpClientOptions {
        roots: vec![root.path().to_owned()],
        enable_sampling: true,
        enable_elicitation: true,
        max_sampling_tokens: 32,
        ..Default::default()
    };
    let registry = McpRegistry::connect_with_client(
        &[enabled],
        Some(hooks(
            json!({"action":"accept","content":{"answer":"中文确认"}}),
        )),
    )
    .await?;
    assert_eq!(
        call(&registry, "ask_sampling", json!({})).await?["structuredContent"]["result"]["content"]
            ["text"],
        "sampled locally"
    );
    assert_eq!(
        call(&registry, "ask_sampling", json!({"maxTokens":33})).await?["structuredContent"]["error"]
            ["code"],
        -32602
    );
    assert_eq!(
        call(&registry, "ask_elicitation", json!({})).await?["structuredContent"]["result"]["content"]
            ["answer"],
        "中文确认"
    );
    assert!(call(&registry,"ask_roots",json!({})).await?["structuredContent"]["result"]["roots"][0]["uri"].as_str().unwrap().starts_with("file://"));
    registry
        .update_roots("optional", &[second.path().to_owned()])
        .await?;
    assert_eq!(
        call(&registry, "inspect_caps", json!({})).await?["structuredContent"]["roots_changed"],
        1
    );
    assert_eq!(registry.request("optional","completion/complete",json!({"ref":{"type":"ref/prompt","name":"fixture"},"argument":{"name":"x","value":"a"}}),cancel()).await?["completion"]["values"],json!(["alpha","beta"]));
    registry
        .request(
            "optional",
            "resources/subscribe",
            json!({"uri":"test://events"}),
            cancel(),
        )
        .await?;
    call(&registry, "emit_changes", json!({})).await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let notifications = registry.drain_notifications("optional")?;
    for method in [
        "notifications/resources/updated",
        "notifications/resources/list_changed",
        "notifications/tools/list_changed",
        "notifications/prompts/list_changed",
    ] {
        assert!(
            notifications.iter().any(|event| event["method"] == method),
            "missing {method}"
        );
    }
    registry
        .request(
            "optional",
            "resources/unsubscribe",
            json!({"uri":"test://events"}),
            cancel(),
        )
        .await?;
    registry.close().await?;
    Ok(())
}
#[tokio::test]
async fn schema_invalid_accept_and_decline_are_preserved() -> Result<()> {
    for (value, expected) in [
        (
            json!({"action":"accept","content":{"answer":42}}),
            json!(-32602),
        ),
        (json!({"action":"decline"}), json!("decline")),
        (json!({"action":"cancel"}), json!("cancel")),
    ] {
        let mut enabled = config();
        enabled.client.enable_elicitation = true;
        let registry = McpRegistry::connect_with_client(&[enabled], Some(hooks(value))).await?;
        let result = call(&registry, "ask_elicitation", json!({})).await?;
        let outcome = if result["structuredContent"]["error"].is_null() {
            result["structuredContent"]["result"]["action"].clone()
        } else {
            result["structuredContent"]["error"]["code"].clone()
        };
        assert_eq!(outcome, expected);
        assert!(
            format!(
                "{:#}",
                call(&registry, "schema", json!({"value":"wrong"}))
                    .await
                    .unwrap_err()
            )
            .contains("input")
        );
        assert_eq!(
            call(&registry, "schema", json!({"value":7})).await?["structuredContent"]["value"],
            7
        );
        assert!(
            format!(
                "{:#}",
                call(&registry, "wrong_output", json!({"value":7}))
                    .await
                    .unwrap_err()
            )
            .contains("output")
        );
        registry.close().await?;
    }
    Ok(())
}
#[tokio::test]
async fn modern_tasks_finish_handle_input_and_honor_total_timeout_and_cancel() -> Result<()> {
    let mut enabled = config();
    enabled.env.insert("MCP_TASKS".into(), "1".into());
    enabled.client.enable_tasks = true;
    enabled.timeout_ms = 250;
    let registry =
        McpRegistry::connect_with_client(&[enabled], Some(hooks(json!({"action":"decline"}))))
            .await?;
    assert_eq!(registry.status().await["servers"][0]["state"], "connected");
    assert_eq!(
        call(&registry, "task", json!({})).await?["structuredContent"]["completed"],
        true
    );
    assert_eq!(
        call(&registry, "task_input", json!({})).await?["structuredContent"]["updated"],
        true
    );
    assert!(matches!(
        call(&registry, "slow_task", json!({}))
            .await
            .unwrap_err()
            .downcast_ref::<McpError>(),
        Some(McpError::Timeout { .. })
    ));
    let token = cancel();
    let later = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(40)).await;
        later.cancel();
    });
    assert!(matches!(
        registry
            .call("mcp__optional__slow_task", json!({}), token)
            .await
            .unwrap_err()
            .downcast_ref::<McpError>(),
        Some(McpError::Cancelled { .. })
    ));
    assert!(matches!(
        registry
            .request("optional", "tasks/list", json!({}), cancel())
            .await
            .unwrap_err()
            .downcast_ref::<McpError>(),
        Some(McpError::Unsupported { .. })
    ));
    registry.close().await?;
    Ok(())
}
