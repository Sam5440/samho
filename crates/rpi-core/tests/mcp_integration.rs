use rpi_core::mcp::{McpError, McpRegistry, McpServerConfig};
use rpi_core::test_support::bind_loopback;
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn python() -> String {
    std::env::var("RPI_TEST_PYTHON").unwrap_or_else(|_| "python".into())
}
fn stdio_config() -> McpServerConfig {
    McpServerConfig {
        name: "fixture".into(),
        command: Some(python()),
        args: vec![
            "-u".into(),
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/mcp_server.py")
                .to_string_lossy()
                .into_owned(),
        ],
        timeout_ms: 1_000,
        ..Default::default()
    }
}
fn cancel() -> CancellationToken {
    CancellationToken::new()
}

fn process_is_running(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .expect("tasklist is available on Windows");
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("process status probe")
            .success()
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = pid;
        false
    }
}
async fn assert_child_exited(pid: u32) {
    for _ in 0..20 {
        if !process_is_running(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("MCP child {pid} survived cleanup");
}

#[tokio::test]
async fn stdio_reaps_children_after_explicit_close_and_initialize_timeout() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let pid_path = directory.path().join("child.pid");
    let mut config = stdio_config();
    config.env.insert(
        "MCP_PID_FILE".into(),
        pid_path.to_string_lossy().into_owned(),
    );
    let registry = McpRegistry::connect(&[config.clone()]).await?;
    let pid = std::fs::read_to_string(&pid_path)?.parse::<u32>()?;
    assert!(process_is_running(pid));
    registry.close().await?;
    assert_child_exited(pid).await;
    config.env.insert("MCP_TEST_MODE".into(), "hang".into());
    config.startup_timeout_ms = 200;
    let registry = McpRegistry::connect(&[config]).await?;
    assert_eq!(
        registry.status().await["servers"][0]["error"]["kind"],
        "timeout"
    );
    let pid = std::fs::read_to_string(&pid_path)?.parse::<u32>()?;
    assert_child_exited(pid).await;
    registry.close().await?;
    Ok(())
}

#[tokio::test]
async fn stdio_negotiates_pages_tools_resources_prompts_and_object_calls() -> anyhow::Result<()> {
    let registry = McpRegistry::connect(&[stdio_config()]).await?;
    assert_eq!(registry.status().await["servers"][0]["state"], "connected");
    let schemas = registry.tools().await;
    assert_eq!(schemas.len(), 3);
    assert!(
        schemas
            .iter()
            .any(|schema| schema["function"]["name"] == "mcp__fixture__echo")
    );
    let result = registry
        .call(
            "mcp__fixture__echo",
            json!({"value":"real protocol"}),
            cancel(),
        )
        .await?;
    assert_eq!(result["structuredContent"]["value"], "real protocol");
    let calls =
        (0..20).map(|index| registry.call("mcp__fixture__echo", json!({"value":index}), cancel()));
    for (index, result) in futures_util::future::join_all(calls)
        .await
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            result?["structuredContent"]["value"],
            json!(index),
            "concurrent replies must match request IDs"
        );
    }
    let resources = registry
        .request("fixture", "resources/list", json!({}), cancel())
        .await?;
    assert_eq!(resources["resources"][0]["uri"], "test://hello");
    let templates = registry
        .request("fixture", "resource_templates/list", json!({}), cancel())
        .await?;
    assert_eq!(
        templates["resourceTemplates"][0]["uriTemplate"],
        "test://{name}"
    );
    let resource = registry
        .request(
            "fixture",
            "resources/read",
            json!({"uri":"test://hello"}),
            cancel(),
        )
        .await?;
    assert_eq!(resource["contents"][0]["text"], "fixture resource");
    assert_eq!(
        registry
            .request("fixture", "prompts/list", json!({}), cancel())
            .await?["prompts"][0]["name"],
        "greet"
    );
    let prompt = registry
        .request(
            "fixture",
            "prompts/get",
            json!({"name":"greet","arguments":{"name":"Rust"}}),
            cancel(),
        )
        .await?;
    assert_eq!(prompt["messages"][0]["content"]["text"], "Hello Rust");
    let object = registry.objects().pop().unwrap();
    assert_eq!(object.envelope().id, "mcp:fixture");
    let inspected = object
        .invoke(
            "inspect",
            Value::Null,
            rpi_core::object::InvocationContext {
                session_id: "s".into(),
                trace_id: "t".into(),
                span_id: "span".into(),
                cancel: cancel(),
            },
        )
        .await?;
    assert_eq!(inspected["state"], "connected");
    registry
        .request(
            "fixture",
            "tools/call",
            json!({"name":"change_collision"}),
            cancel(),
        )
        .await?;
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        registry.tools().await.len(),
        3,
        "failed refresh must retain the last complete tool generation"
    );
    assert_eq!(registry.status().await["servers"][0]["state"], "degraded");
    registry.close().await?;
    assert!(
        registry
            .call("mcp__fixture__echo", json!({}), cancel())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn stdio_preserves_errors_and_sends_timeout_and_cancellation() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    let cancellation_path = directory.path().join("cancel.jsonl");
    let mut config = stdio_config();
    config.timeout_ms = 100;
    config.env.insert(
        "MCP_CANCEL_FILE".into(),
        cancellation_path.to_string_lossy().into_owned(),
    );
    let registry = McpRegistry::connect(&[config]).await?;
    let error = registry
        .call("mcp__fixture__fail", json!({}), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(error.downcast_ref::<McpError>(),Some(McpError::ToolFailure{result,..}) if result["isError"]==true)
    );
    let error = registry
        .request(
            "fixture",
            "tools/call",
            json!({"name":"remote_error"}),
            cancel(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error.downcast_ref::<McpError>(),Some(McpError::Remote{code,data,..}) if *code == -32602 && data.as_ref().unwrap()["fixture"]==true)
    );
    let error = registry
        .call("mcp__fixture__slow", json!({}), cancel())
        .await
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<McpError>(),
            Some(McpError::Timeout { .. })
        ),
        "{error}"
    );
    let token = cancel();
    let to_cancel = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(25)).await;
        to_cancel.cancel();
    });
    let error = registry
        .call("mcp__fixture__slow", json!({}), token)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<McpError>(),
            Some(McpError::Cancelled { .. })
        ),
        "{error}"
    );
    let invalid = registry
        .request("fixture", "resources/read", json!({"uri":25}), cancel())
        .await
        .unwrap_err();
    assert!(matches!(
        invalid.downcast_ref::<McpError>(),
        Some(McpError::InvalidParams { .. })
    ));
    let unsupported = registry
        .request("fixture", "tasks/list", json!({}), cancel())
        .await
        .unwrap_err();
    assert!(matches!(
        unsupported.downcast_ref::<McpError>(),
        Some(McpError::Unsupported { .. })
    ));
    for _ in 0..20 {
        if std::fs::read_to_string(&cancellation_path)
            .unwrap_or_default()
            .lines()
            .count()
            >= 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(std::fs::read_to_string(&cancellation_path)?.lines().count() >= 2);
    registry.close().await?;
    Ok(())
}

#[tokio::test]
async fn failed_servers_do_not_block_healthy_servers_and_name_collisions_are_rejected()
-> anyhow::Result<()> {
    let mut bad = stdio_config();
    bad.name = "bad".into();
    bad.command = Some("rpi-mcp-fixture-command-does-not-exist".into());
    let registry = McpRegistry::connect(&[bad, stdio_config()]).await?;
    let status = registry.status().await;
    assert_eq!(status["servers"][0]["state"], "failed");
    assert_eq!(registry.tools().await.len(), 3);
    registry.close().await?;
    let mut collision = stdio_config();
    collision
        .env
        .insert("MCP_TEST_MODE".into(), "collision".into());
    let registry = McpRegistry::connect(&[collision]).await?;
    assert!(registry.tools().await.is_empty());
    assert_eq!(registry.status().await["servers"][0]["state"], "degraded");
    registry.close().await?;
    assert!(
        McpRegistry::connect(&[stdio_config(), stdio_config()])
            .await
            .is_err()
    );
    assert!(serde_json::from_value::<McpServerConfig>(json!({"name":"bad","typo":1})).is_err());
    Ok(())
}

#[derive(Clone, Default)]
struct HttpState(Arc<tokio::sync::Mutex<Vec<Value>>>);
async fn http_post(
    axum::extract::State(state): axum::extract::State<HttpState>,
    headers: axum::http::HeaderMap,
    axum::Json(request): axum::Json<Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    state.0.lock().await.push(json!({"request":request,"authorization":headers.get("authorization").and_then(|value|value.to_str().ok())}));
    let Some(id) = request.get("id") else {
        return axum::http::StatusCode::ACCEPTED.into_response();
    };
    let result = match request["method"].as_str().unwrap_or_default() {
        "initialize" => {
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{},"resources":{},"prompts":{}},"serverInfo":{"name":"http-test","version":"1"}})
        }
        "tools/list" => json!({"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}),
        "tools/call" => {
            json!({"content":[{"type":"text","text":"HTTP real response"}],"structuredContent":request["params"]["arguments"]})
        }
        "resources/list" => json!({"resources":[]}),
        "resources/templates/list" => json!({"resourceTemplates":[]}),
        "prompts/list" => json!({"prompts":[]}),
        "ping" => json!({}),
        _ => return axum::Json(
            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"not implemented"}}),
        )
        .into_response(),
    };
    axum::Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}
#[tokio::test]
async fn streamable_http_initializes_and_calls_real_server() -> anyhow::Result<()> {
    let state = HttpState::default();
    let listener = bind_loopback().await?;
    let address = listener.local_addr()?;
    let stop = cancel();
    let stopped = stop.clone();
    let app = axum::Router::new()
        .route("/mcp", axum::routing::post(http_post))
        .with_state(state.clone());
    let running = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
    });
    let config = McpServerConfig {
        name: "http".into(),
        url: Some(format!("http://{address}/mcp")),
        timeout_ms: 1_000,
        ..Default::default()
    };
    let registry = McpRegistry::connect(&[config]).await?;
    assert_eq!(
        registry.status().await["servers"][0]["state"],
        "connected",
        "{}",
        registry.status().await
    );
    assert_eq!(registry.tools().await.len(), 1);
    let result = registry
        .call("mcp__http__echo", json!({"value":42}), cancel())
        .await?;
    assert_eq!(result["structuredContent"]["value"], 42);
    assert_eq!(
        registry
            .request("http", "resources/list", json!({}), cancel())
            .await?["resources"],
        json!([])
    );
    assert_eq!(
        registry
            .request("http", "prompts/list", json!({}), cancel())
            .await?["prompts"],
        json!([])
    );
    assert!(
        state
            .0
            .lock()
            .await
            .iter()
            .any(|call| call["request"]["method"] == "initialize")
    );
    registry.close().await?;
    stop.cancel();
    running.await??;
    Ok(())
}
