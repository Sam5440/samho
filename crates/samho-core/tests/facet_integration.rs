use axum::{
    Router,
    extract::{Json, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use samho_core::test_support::bind_loopback;
use samho_core::{
    facet::{FACET_PROTOCOL_VERSION, FacetConfig, FacetErrorKind, RemoteFacet},
    object::{InvocationContext, Object},
    tools::NetworkPolicyConfig,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::{
    sync::Mutex,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct StateData {
    calls: Arc<Mutex<Vec<Value>>>,
    cancelled: Arc<Mutex<Vec<String>>>,
    delay_ms: u64,
    bad_protocol: bool,
}
async fn handshake(
    State(state): State<StateData>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    state.calls.lock().await.push(json!({"kind":"handshake","auth":headers.get("authorization").and_then(|v|v.to_str().ok()),"request":request}));
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer facet-secret")
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"auth"})));
    }
    (
        StatusCode::OK,
        Json(
            json!({"protocol":if state.bad_protocol {"samho-facet/0"} else {FACET_PROTOCOL_VERSION},"name":"worker","version":"0.1","capabilities":["echo","slow"],"metadata":{"fixture":true}}),
        ),
    )
}
async fn invoke(
    State(state): State<StateData>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer facet-secret")
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"auth"})));
    }
    state.calls.lock().await.push(json!({"kind":"invoke","auth":headers.get("authorization").and_then(|v|v.to_str().ok()),"traceparent":headers.get("traceparent").and_then(|v|v.to_str().ok()),"request":request}));
    if state.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(state.delay_ms)).await;
    }
    (
        StatusCode::OK,
        Json(
            json!({"protocol":FACET_PROTOCOL_VERSION,"call_id":request["call_id"],"result":{"echo":request["input"],"trace_id":request["trace_id"],"parent_span_id":request["parent_span_id"]}}),
        ),
    )
}
async fn cancel(
    State(state): State<StateData>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some("Bearer facet-secret")
    {
        return (StatusCode::UNAUTHORIZED, Json(json!({"error":"auth"})));
    }
    state
        .cancelled
        .lock()
        .await
        .push(request["call_id"].as_str().unwrap_or_default().into());
    (StatusCode::OK, Json(json!({"ok":true})))
}
async fn server(state: StateData) -> (String, StateData, tokio::task::JoinHandle<()>) {
    let listener = bind_loopback().await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/handshake", post(handshake))
        .route("/invoke", post(invoke))
        .route("/cancel", post(cancel))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), state, task)
}
fn context(cancel: CancellationToken) -> InvocationContext {
    InvocationContext {
        session_id: "session-1".into(),
        trace_id: "0123456789abcdef0123456789abcdef".into(),
        span_id: "span-parent".into(),
        cancel,
    }
}
fn config(endpoint: String) -> FacetConfig {
    FacetConfig {
        name: "worker".into(),
        endpoint,
        auth_token_env: "SAMHO_FACET_TEST_TOKEN".into(),
        allowed_capabilities: vec!["echo".into(), "slow".into()],
        object_id: "facet:worker".into(),
        allow_insecure_loopback: true,
        ..FacetConfig::default()
    }
}
async fn connect(config: FacetConfig) -> samho_core::facet::FacetResult<RemoteFacet> {
    RemoteFacet::connect_with_token(
        config,
        "facet-secret".into(),
        CancellationToken::new(),
        Arc::new(NetworkPolicyConfig::default()),
    )
    .await
}

#[tokio::test]
async fn handshake_auth_allowlist_and_trace_propagation_work() {
    let (endpoint, state, task) = server(StateData::default()).await;
    let facet = connect(config(endpoint)).await.unwrap();
    assert_eq!(facet.handshake().await.protocol, FACET_PROTOCOL_VERSION);
    let result = facet
        .call(
            "echo",
            json!({"message":"hello"}),
            &context(CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(result["echo"]["message"], "hello");
    assert_eq!(result["trace_id"], "0123456789abcdef0123456789abcdef");
    let calls = state.calls.lock().await;
    let invoke = calls.iter().find(|call| call["kind"] == "invoke").unwrap();
    assert_eq!(invoke["auth"], "Bearer facet-secret");
    assert_eq!(invoke["request"]["protocol"], FACET_PROTOCOL_VERSION);
    assert_eq!(invoke["request"]["parent_span_id"], "span-parent");
    assert!(
        invoke["traceparent"]
            .as_str()
            .unwrap()
            .starts_with("00-0123456789abcdef0123456789abcdef-")
    );
    assert_eq!(facet.envelope().id, "facet:worker");
    task.abort();
}

#[tokio::test]
async fn capability_denial_and_missing_auth_are_explicit() {
    let (endpoint, _, task) = server(StateData::default()).await;
    let mut denied = config(endpoint.clone());
    denied.allowed_capabilities = vec!["other".into()];
    let error = connect(denied).await.unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::CapabilityDenied);
    let mut missing = config(endpoint);
    missing.auth_token_env = format!("SAMHO_MISSING_{}", uuid::Uuid::new_v4().simple());
    let error = RemoteFacet::connect(&missing, Arc::new(NetworkPolicyConfig::default()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Authentication);
    let (endpoint, _, other) = server(StateData::default()).await;
    let error = RemoteFacet::connect_with_token(
        config(endpoint.clone()),
        String::new(),
        CancellationToken::new(),
        Arc::new(NetworkPolicyConfig::default()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Authentication);
    let error = RemoteFacet::connect_with_token(
        config(endpoint.clone()),
        "wrong-token".into(),
        CancellationToken::new(),
        Arc::new(NetworkPolicyConfig::default()),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Authentication);
    let facet = connect(config(endpoint)).await.unwrap();
    assert_eq!(
        facet
            .call("unlisted", json!({}), &context(CancellationToken::new()))
            .await
            .unwrap_err()
            .kind,
        FacetErrorKind::CapabilityDenied
    );
    other.abort();
    task.abort();
}

#[tokio::test]
async fn cancellation_sends_best_effort_cancel_and_dispose_breaks_calls() {
    let state = StateData {
        delay_ms: 500,
        ..StateData::default()
    };
    let (endpoint, state, task) = server(state).await;
    let facet = connect(config(endpoint)).await.unwrap();
    let cancel = CancellationToken::new();
    let child = cancel.clone();
    let remote = facet.clone();
    let call = tokio::spawn(async move { remote.call("slow", json!({}), &context(child)).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    let error = timeout(Duration::from_secs(1), call)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Cancelled);
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert_eq!(state.cancelled.lock().await.len(), 1);
    facet.dispose().await.unwrap();
    let error = facet
        .call("echo", json!({}), &context(CancellationToken::new()))
        .await
        .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Disconnected);
    task.abort();
}

#[tokio::test]
async fn handshake_version_network_policy_and_response_limit_fail_closed() {
    let bad = StateData {
        bad_protocol: true,
        ..StateData::default()
    };
    let (endpoint, _, task) = server(bad).await;
    let error = connect(config(endpoint)).await.unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Protocol);
    task.abort();
    let mut cfg = config("http://example.invalid".into());
    cfg.allow_insecure_loopback = false;
    let error = connect(cfg).await.unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::Configuration);
    let (endpoint, _, denied) = server(StateData::default()).await;
    let policy = NetworkPolicyConfig {
        allow_loopback: false,
        ..NetworkPolicyConfig::default()
    };
    let error = RemoteFacet::connect_with_token(
        config(endpoint),
        "facet-secret".into(),
        CancellationToken::new(),
        Arc::new(policy),
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, FacetErrorKind::NetworkDenied);
    denied.abort();
    let (endpoint, _, task) = server(StateData::default()).await;
    let mut cfg = config(endpoint);
    cfg.max_response_bytes = 1;
    let error = connect(cfg).await.unwrap_err();
    assert!(matches!(
        error.kind,
        FacetErrorKind::ResponseLimit | FacetErrorKind::Protocol
    ));
    task.abort();
}

#[tokio::test]
async fn disconnected_endpoint_is_network_error_without_any_unauthenticated_retry() {
    // 1300 is reserved for the intentionally disconnected fixture; all test
    // endpoints stay inside the 1300-1400 policy range.
    let cfg = config("http://127.0.0.1:1300".into());
    let error = connect(cfg).await.unwrap_err();
    assert!(matches!(
        error.kind,
        FacetErrorKind::Network | FacetErrorKind::Timeout
    ));
}

#[tokio::test]
async fn separate_process_handshake_call_cancel_and_disconnect() {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    // 环境探针：区分“解释器异常”与“fixture 自身问题”。
    let probe = timeout(Duration::from_secs(15), async {
        tokio::process::Command::new("python")
            .arg("-c")
            .arg("print('ok')")
            .output()
            .await
    })
    .await;
    match &probe {
        Ok(Ok(output)) => assert!(
            String::from_utf8_lossy(&output.stdout).contains("ok"),
            "python probe failed: {output:?}"
        ),
        other => panic!("python environment probe did not respond in time: {other:?}"),
    }
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/facet_server.py");
    let mut child = tokio::process::Command::new("python")
        .arg(path)
        .env("SAMHO_FIXTURE_FACET_TOKEN", "facet-secret")
        .env("SAMHO_FIXTURE_FACET_PORT", "1399")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("Python is required for local Facet process fixture");
    let process_id = child.id().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let port = match timeout(Duration::from_secs(30), stdout.next_line()).await {
        Ok(line) => line
            .expect("fixture stdout closed")
            .expect("read fixture stdout"),
        Err(_) => {
            let _ = child.start_kill();
            let mut message = String::new();
            if let Some(mut stream) = child.stderr.take() {
                let _ = stream.read_to_string(&mut message).await;
            }
            let status = child.wait().await;
            panic!("fixture did not report its port in 30s; status={status:?} stderr={message}");
        }
    };
    let endpoint = format!("http://127.0.0.1:{}", port.trim());
    let response = reqwest::Client::new()
        .post(format!("{endpoint}/handshake"))
        .json(&json!({"protocol":FACET_PROTOCOL_VERSION}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let facet = connect(config(endpoint)).await.unwrap();
    assert_eq!(facet.handshake().await.metadata["pid"], process_id);
    let result = facet
        .call(
            "echo",
            json!({"value":"跨进程"}),
            &context(CancellationToken::new()),
        )
        .await
        .unwrap();
    assert_eq!(result["echo"]["value"], "跨进程");
    assert_eq!(result["pid"], process_id);
    assert_ne!(process_id, std::process::id());
    let cancel = CancellationToken::new();
    let child_cancel = cancel.clone();
    let remote = facet.clone();
    let call =
        tokio::spawn(async move { remote.call("slow", json!({}), &context(child_cancel)).await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    assert_eq!(
        timeout(Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        FacetErrorKind::Cancelled
    );
    child.kill().await.unwrap();
    child.wait().await.unwrap();
    let error = facet
        .call("echo", json!({}), &context(CancellationToken::new()))
        .await
        .unwrap_err();
    assert!(matches!(
        error.kind,
        FacetErrorKind::Network | FacetErrorKind::Timeout
    ));
}
