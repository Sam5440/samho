use std::{convert::Infallible, sync::Arc};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Response, StatusCode},
    routing::post,
};
use futures_util::stream;
use samho_core::test_support::bind_loopback;
use samho_core::{
    model::{ModelDescriptor, ProviderKind},
    provider::{
        ModelProvider, ModelResponse, Provider, ProviderErrorKind, ProviderLimits, ProviderResult,
        ProviderStreamEvent, Usage,
    },
};
use serde_json::{Value, json};
use tokio::{
    sync::{Mutex, mpsc},
    task::JoinHandle,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Fixture {
    bytes: Vec<u8>,
    chunk_size: usize,
    delay_ms: u64,
    status: StatusCode,
    content_type: &'static str,
    requests: Arc<Mutex<Vec<Value>>>,
}
async fn respond(State(state): State<Fixture>, Json(request): Json<Value>) -> Response<Body> {
    state.requests.lock().await.push(request);
    let chunks = state
        .bytes
        .chunks(state.chunk_size)
        .map(Vec::from)
        .collect::<Vec<_>>();
    let body = stream::unfold(
        (chunks.into_iter(), state.delay_ms),
        |(mut chunks, delay)| async move {
            match chunks.next() {
                Some(chunk) => {
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }
                    Some((Ok::<_, Infallible>(chunk), (chunks, delay)))
                }
                None => None,
            }
        },
    );
    Response::builder()
        .status(state.status)
        .header("content-type", state.content_type)
        .header("retry-after", "2")
        .body(Body::from_stream(body))
        .unwrap()
}
async fn fixture(
    bytes: impl Into<Vec<u8>>,
    chunk_size: usize,
    delay_ms: u64,
    status: StatusCode,
) -> (ModelDescriptor, Fixture, JoinHandle<()>) {
    let state = Fixture {
        bytes: bytes.into(),
        chunk_size,
        delay_ms,
        status,
        content_type: "text/event-stream; charset=utf-8",
        requests: Arc::default(),
    };
    let listener = bind_loopback().await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/v1/chat/completions", post(respond))
        .with_state(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        ModelDescriptor {
            id: "fixture".into(),
            provider: ProviderKind::ChatCompletions,
            base_url: Some(format!("http://{address}/v1")),
            ..ModelDescriptor::default()
        },
        state,
        server,
    )
}
fn frame(delta: Value, finish: Option<&str>) -> String {
    format!(
        "data: {}\r\n\r\n",
        json!({"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    )
}
fn text_stream(text: &str) -> String {
    format!(
        ": keep-alive\r\n\r\n{}{}data: [DONE]\r\n\r\n",
        frame(json!({"role":"assistant","content":text}), None),
        frame(json!({}), Some("stop"))
    )
}
async fn collect(
    provider: Provider,
    model: ModelDescriptor,
) -> (ProviderResult<ModelResponse>, Vec<ProviderStreamEvent>) {
    let (sender, mut receiver) = mpsc::channel(2);
    let task = tokio::spawn(async move {
        provider
            .complete_stream(
                &model,
                &[json!({"role":"user","content":"hello"})],
                &[],
                CancellationToken::new(),
                sender,
            )
            .await
    });
    let mut events = Vec::new();
    while let Some(event) = receiver.recv().await {
        events.push(event);
    }
    (task.await.unwrap(), events)
}

#[tokio::test]
async fn parses_byte_fragmented_utf8_crlf_text_tools_and_usage() {
    let stream = format!(
        "\u{feff}: ready\r\n\r\n{}{}{}{}{}data: {}\r\n\r\ndata: [DONE]\r\n\r\n",
        frame(json!({"role":"assistant","content":"你"}), None),
        frame(
            json!({"content":"好","reasoning_content":"计划","tool_calls":[{"index":0,"id":"call_","type":"function","function":{"name":"read_","arguments":"{\"pa"}}]}),
            None
        ),
        frame(
            json!({"tool_calls":[{"index":0,"id":"1","function":{"name":"file","arguments":"th\":\"文"}}]}),
            None
        ),
        frame(
            json!({"tool_calls":[{"index":0,"function":{"arguments":"件.txt\"}"}}]}),
            None
        ),
        frame(json!({}), Some("tool_calls")),
        json!({"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}})
    );
    let (model, state, server) = fixture(stream, 1, 0, StatusCode::OK).await;
    let (result, events) = collect(Provider::new(5000).unwrap(), model).await;
    let result = result.unwrap();
    assert_eq!(result.message["content"], "你好");
    assert_eq!(result.message["reasoning_content"], "计划");
    assert_eq!(result.message["tool_calls"][0]["id"], "call_1");
    assert_eq!(
        result.message["tool_calls"][0]["function"]["name"],
        "read_file"
    );
    assert_eq!(
        result.message["tool_calls"][0]["function"]["arguments"],
        "{\"path\":\"文件.txt\"}"
    );
    assert_eq!(
        result.usage,
        Usage {
            input_tokens: 11,
            output_tokens: 7
        }
    );
    assert!(matches!(
        events.first(),
        Some(ProviderStreamEvent::Start { .. })
    ));
    assert!(matches!(events.last(), Some(ProviderStreamEvent::Done)));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderStreamEvent::ToolCallDelta { .. }))
            .count(),
        3
    );
    let request = &state.requests.lock().await[0];
    assert_eq!(request["stream"], true);
    assert_eq!(request["n"], 1);
    assert_eq!(request["stream_options"]["include_usage"], true);
    server.abort();
}

#[tokio::test]
async fn parses_multiline_data_frames_and_arbitrary_chunk_boundaries() {
    let stream = format!(
        "data: {{\"choices\":\ndata: [{{\"index\":0,\"delta\":{{\"content\":\"ok\"}},\"finish_reason\":null}}]}}\n\n{}data: [DONE]\n\n",
        frame(json!({}), Some("stop"))
    );
    for size in [1, 2, 7, 31, 4096] {
        let (model, _, server) = fixture(stream.clone(), size, 0, StatusCode::OK).await;
        let (result, _) = collect(Provider::new(5000).unwrap(), model).await;
        assert_eq!(result.unwrap().message["content"], "ok");
        server.abort();
    }
}

#[tokio::test]
async fn rejects_eof_missing_finish_invalid_json_and_incomplete_arguments() {
    let samples = [
        frame(json!({"content":"partial"}), None),
        format!(
            "{}data: [DONE]\n\n",
            frame(json!({"content":"no finish"}), None)
        ),
        format!("{}data: [DONE]", frame(json!({}), Some("stop"))),
        "data: {invalid}\n\n".into(),
        "data: [DONE]\n\n".into(),
        format!(
            "{}{}data: [DONE]\n\n",
            frame(
                json!({"tool_calls":[{"index":0,"id":"a","type":"function","function":{"name":"read","arguments":"{\"a\":"}}]}),
                None
            ),
            frame(json!({}), Some("tool_calls"))
        ),
        format!(
            "{}data: [DONE]\n\n",
            frame(json!({"content":42}), Some("stop"))
        ),
    ];
    for sample in samples {
        let (model, _, server) = fixture(sample, 13, 0, StatusCode::OK).await;
        let (result, events) = collect(Provider::new(5000).unwrap(), model).await;
        assert_eq!(result.unwrap_err().kind, ProviderErrorKind::Protocol);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProviderStreamEvent::Done))
        );
        server.abort();
    }
}

#[tokio::test]
async fn output_length_and_content_filter_are_not_success() {
    for (reason, kind) in [
        ("length", ProviderErrorKind::OutputLimit),
        ("content_filter", ProviderErrorKind::Protocol),
    ] {
        let (model, _, server) = fixture(
            format!(
                "{}data: [DONE]\n\n",
                frame(json!({"content":"truncated"}), Some(reason))
            ),
            4096,
            0,
            StatusCode::OK,
        )
        .await;
        let (result, _) = collect(Provider::new(5000).unwrap(), model).await;
        let error = result.unwrap_err();
        assert_eq!(error.kind, kind);
        assert!(!error.retryable());
        server.abort();
    }
}

#[tokio::test]
async fn retains_http_error_status_retry_after_and_retry_classification() {
    for (status, kind, retryable) in [
        (
            StatusCode::TOO_MANY_REQUESTS,
            ProviderErrorKind::RateLimited,
            true,
        ),
        (
            StatusCode::UNAUTHORIZED,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            ProviderErrorKind::Authentication,
            false,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            ProviderErrorKind::Server,
            true,
        ),
        (StatusCode::BAD_REQUEST, ProviderErrorKind::Protocol, false),
    ] {
        let (model, _, server) = fixture("{\"error\":\"fixture\"}", 4096, 0, status).await;
        let provider = Provider::new(5000).unwrap();
        let (result, _) = collect(provider.clone(), model.clone()).await;
        let error = result.unwrap_err();
        assert_eq!(error.kind, kind);
        assert_eq!(error.retryable(), retryable);
        assert_eq!(error.status_code, Some(status.as_u16()));
        assert_eq!(error.retry_after.as_deref(), Some("2"));
        assert_eq!(error.retry_after_ms, Some(2000));
        let compatible = provider
            .complete(&model, &[], &[], CancellationToken::new())
            .await
            .unwrap_err();
        assert!(
            compatible
                .downcast_ref::<samho_core::provider::ProviderError>()
                .is_some()
        );
        server.abort();
    }
}

#[tokio::test]
async fn enforces_frame_total_and_tool_count_limits() {
    let cases = [
        (text_stream(&"x".repeat(300)), 1024, 100, 2),
        (text_stream("one") + &text_stream("two"), 140, 140, 2),
        (
            format!(
                "{}{}data: [DONE]\n\n",
                frame(
                    json!({"tool_calls":[{"index":2,"id":"a","function":{"name":"a","arguments":"{}"}}]}),
                    None
                ),
                frame(json!({}), Some("tool_calls"))
            ),
            4096,
            2048,
            2,
        ),
    ];
    for (sample, max_response_bytes, max_frame_bytes, max_tool_calls) in cases {
        let (model, _, server) = fixture(sample, 17, 0, StatusCode::OK).await;
        let provider = Provider::new(5000)
            .unwrap()
            .with_limits(ProviderLimits {
                max_response_bytes,
                max_frame_bytes,
                max_tool_calls,
                ..ProviderLimits::default()
            })
            .unwrap();
        assert_eq!(
            collect(provider, model).await.0.unwrap_err().kind,
            ProviderErrorKind::ResponseLimit
        );
        server.abort();
    }
}

#[tokio::test]
async fn cancellation_interrupts_network_read_and_backpressured_sender() {
    let (model, _, server) = fixture(text_stream("slow"), 1, 50, StatusCode::OK).await;
    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        Provider::new(10000)
            .unwrap()
            .complete_stream(&model, &[], &[], task_cancel, sender)
            .await
    });
    assert!(matches!(
        receiver.recv().await,
        Some(ProviderStreamEvent::Start { .. })
    ));
    cancel.cancel();
    assert_eq!(
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        ProviderErrorKind::Cancelled
    );
    server.abort();

    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let (sender, mut receiver) = mpsc::channel(1);
    let task = tokio::spawn(async move {
        Provider::new(10000)
            .unwrap()
            .complete_stream(&ModelDescriptor::default(), &[], &[], task_cancel, sender)
            .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(receiver.len(), 1);
    assert!(!task.is_finished());
    assert!(matches!(
        receiver.recv().await,
        Some(ProviderStreamEvent::Start { .. })
    ));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(receiver.len(), 1);
    assert!(!task.is_finished());
    cancel.cancel();
    assert_eq!(
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .kind,
        ProviderErrorKind::Cancelled
    );
}

#[tokio::test]
async fn dropped_consumer_and_timeout_have_distinct_errors() {
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    let error = Provider::new(1000)
        .unwrap()
        .complete_stream(
            &ModelDescriptor::default(),
            &[],
            &[],
            CancellationToken::new(),
            sender,
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ProviderErrorKind::ConsumerClosed);
    assert!(!error.retryable());
    let (model, _, server) = fixture(text_stream("slow"), 1, 50, StatusCode::OK).await;
    let (result, _) = collect(Provider::new(80).unwrap(), model).await;
    let error = result.unwrap_err();
    assert_eq!(error.kind, ProviderErrorKind::Timeout);
    assert!(error.retryable());
    server.abort();
}

#[tokio::test]
async fn reserved_extra_body_fields_cannot_override_stream_or_budget() {
    let (mut model, state, server) = fixture(text_stream("ok"), 4096, 0, StatusCode::OK).await;
    for key in [
        "stream",
        "stream_options",
        "max_tokens",
        "max_completion_tokens",
        "max_output_tokens",
        "n",
        "model",
        "messages",
        "tools",
        "reasoning_effort",
    ] {
        model.extra_body = json!({key:false});
        let (result, _) = collect(Provider::new(5000).unwrap(), model.clone()).await;
        assert_eq!(result.unwrap_err().kind, ProviderErrorKind::Configuration);
    }
    assert!(state.requests.lock().await.is_empty());
    server.abort();
}

#[tokio::test]
async fn demo_emits_multiple_unicode_safe_chunks_with_explicit_simulation_label() {
    let (result, events) = collect(Provider::new(1000).unwrap(), ModelDescriptor::default()).await;
    let response = result.unwrap();
    let chunks = events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::TextDelta { delta } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(chunks.len() > 1);
    assert_eq!(
        chunks.join(""),
        response.message["content"].as_str().unwrap()
    );
    assert!(chunks.join("").contains("[DEMO simulation"));
}

struct Injected;
#[async_trait::async_trait]
impl ModelProvider for Injected {
    async fn complete(
        &self,
        _: &ModelDescriptor,
        _: &[Value],
        _: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse> {
        assert!(!cancel.is_cancelled());
        Ok(ModelResponse {
            message: json!({"role":"assistant","content":"injected provider"}),
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
            },
            finish_reason: "stop".into(),
        })
    }
}
#[tokio::test]
async fn injected_trait_backend_and_compatibility_wrapper_work() {
    let provider = Provider::with_backend(1000, Arc::new(Injected)).unwrap();
    let model = ModelDescriptor::default();
    assert_eq!(
        provider
            .complete(&model, &[], &[], CancellationToken::new())
            .await
            .unwrap()
            .message["content"],
        "injected provider"
    );
    let (result, events) = collect(provider, model).await;
    assert_eq!(result.unwrap().usage.output_tokens, 2);
    assert!(matches!(events.last(), Some(ProviderStreamEvent::Done)));
}

// GLM 网关风格：`reasoning` 键携带推理增量，finish_reason 后追加空 delta 帧与 usage 帧。
#[tokio::test]
async fn accepts_reasoning_alias_and_empty_tail_frames_after_finish_reason() {
    let stream = format!(
        "{}{}{}{}data: {}\r\n\r\ndata: {}\r\n\r\ndata: [DONE]\r\n\r\n",
        frame(json!({"role":"assistant","content":""}), None),
        frame(json!({"reasoning":"思考"}), None),
        frame(json!({"content":"答案"}), None),
        frame(json!({}), Some("stop")),
        json!({"choices":[{"index":0,"delta":{},"finish_reason":null}]}),
        json!({"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":3}})
    );
    let (model, _state, server) = fixture(stream, 1, 0, StatusCode::OK).await;
    let (result, events) = collect(Provider::new(5000).unwrap(), model).await;
    let result = result.unwrap();
    assert_eq!(result.message["content"], "答案");
    assert_eq!(result.message["reasoning_content"], "思考");
    assert_eq!(
        result.usage,
        Usage {
            input_tokens: 5,
            output_tokens: 3
        }
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ProviderStreamEvent::Finish { .. }))
            .count(),
        1
    );
    server.abort();
}

// finish_reason 之后携带真实内容的帧仍然是协议错误。
#[tokio::test]
async fn rejects_model_payload_after_finish_reason() {
    let stream = format!(
        "{}{}data: [DONE]\r\n\r\n",
        frame(json!({"role":"assistant","content":"ok"}), Some("stop")),
        frame(json!({"content":"late"}), None)
    );
    let (model, _state, server) = fixture(stream, 1, 0, StatusCode::OK).await;
    let (result, _events) = collect(Provider::new(5000).unwrap(), model).await;
    let error = result.unwrap_err();
    assert_eq!(error.kind, ProviderErrorKind::Protocol);
    assert!(error.message.contains("continued after finish_reason"));
    server.abort();
}

// 网关 2xx + SSE/JSON 内嵌 error 对象按类型分类：upstream_error 视为可重试的 Server。
#[tokio::test]
async fn classifies_embedded_upstream_error_as_retryable_server() {
    let stream = format!(
        "data: {}\r\n\r\ndata: [DONE]\r\n\r\n",
        json!({"error":{"message":"Unable to forward upstream stream","type":"upstream_error"}})
    );
    let (model, _state, server) = fixture(stream, 1, 0, StatusCode::OK).await;
    let (result, _events) = collect(Provider::new(5000).unwrap(), model).await;
    let error = result.unwrap_err();
    assert_eq!(error.kind, ProviderErrorKind::Server);
    assert!(error.retryable());
    assert!(error.message.contains("upstream_error"));
    server.abort();
}

// 非流式 2xx + 内嵌 error 对象走同一分类器。
#[tokio::test]
async fn classifies_non_streaming_embedded_error_and_rate_limit_type() {
    let (model, _state, server) = fixture(
        br#"{"error":{"message":"slow down","type":"rate_limit_error"}}"#.to_vec(),
        1,
        0,
        StatusCode::OK,
    )
    .await;
    let provider = Provider::new(5000).unwrap();
    let error = provider
        .complete_typed(
            &model,
            &[json!({"role":"user","content":"hi"})],
            &[],
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
    assert!(error.retryable());
    server.abort();
}
