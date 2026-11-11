use crate::model::{ModelDescriptor, ProviderKind};
use anyhow::{Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, sync::Arc};
use tokio::{
    sync::mpsc,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelResponse {
    pub message: Value,
    pub usage: Usage,
    pub finish_reason: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    Cancelled,
    Timeout,
    Network,
    RateLimited,
    Server,
    Authentication,
    Protocol,
    Configuration,
    ResponseLimit,
    OutputLimit,
    ConsumerClosed,
}

/// HTTP metadata is preserved without requiring callers to parse an error string.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub message: String,
    pub status_code: Option<u16>,
    pub retry_after: Option<String>,
    pub retry_after_ms: Option<u64>,
}
impl ProviderError {
    pub fn new(kind: ProviderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            status_code: None,
            retry_after: None,
            retry_after_ms: None,
        }
    }
    pub fn retryable(&self) -> bool {
        matches!(
            self.kind,
            ProviderErrorKind::Timeout
                | ProviderErrorKind::Network
                | ProviderErrorKind::RateLimited
                | ProviderErrorKind::Server
        )
    }
    pub fn network(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            ProviderErrorKind::Timeout
        } else {
            ProviderErrorKind::Network
        };
        Self::new(kind, error.without_url().to_string())
    }
}
impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider {:?}: {}", self.kind, self.message)?;
        if let Some(status) = self.status_code {
            write!(formatter, " (HTTP {status})")?;
        }
        Ok(())
    }
}
impl std::error::Error for ProviderError {}
pub type ProviderResult<T> = std::result::Result<T, ProviderError>;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderStreamEvent {
    Start {
        model_id: String,
    },
    TextDelta {
        delta: String,
    },
    ReasoningDelta {
        delta: String,
    },
    ToolCallDelta {
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments: Option<String>,
    },
    Usage {
        usage: Usage,
    },
    Finish {
        finish_reason: String,
    },
    Done,
}

/// Injection boundary. Implementations must honor cancellation and bounded senders.
#[async_trait::async_trait]
pub trait ModelProvider: Send + Sync {
    async fn complete(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse>;
    /// Non-streaming custom implementations inherit a terminal-event adapter.
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
        emit_completed_chunks(model, &response, &cancel, &sender, usize::MAX).await?;
        Ok(response)
    }
}

#[derive(Clone, Debug)]
pub struct ProviderLimits {
    pub max_response_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_tool_calls: usize,
    pub demo_chunk_chars: usize,
}
impl Default for ProviderLimits {
    fn default() -> Self {
        Self {
            max_response_bytes: 16 * 1024 * 1024,
            max_frame_bytes: 256 * 1024,
            max_tool_calls: 128,
            demo_chunk_chars: 32,
        }
    }
}
#[derive(Clone)]
pub struct Provider {
    client: Client,
    timeout_ms: u64,
    limits: ProviderLimits,
    backend: Option<Arc<dyn ModelProvider>>,
    /// Anthropic Messages 协议后端，首次使用时惰性构建。
    anthropic: Arc<tokio::sync::OnceCell<crate::anthropic::AnthropicProvider>>,
}
impl Provider {
    pub fn new(timeout_ms: u64) -> Result<Self> {
        anyhow::ensure!(timeout_ms > 0, "provider timeout must be positive");
        Ok(Self {
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            timeout_ms,
            limits: ProviderLimits::default(),
            backend: None,
            anthropic: Arc::new(tokio::sync::OnceCell::new()),
        })
    }
    async fn anthropic(&self) -> ProviderResult<&crate::anthropic::AnthropicProvider> {
        self.anthropic
            .get_or_try_init(|| async { crate::anthropic::AnthropicProvider::new() })
            .await
    }
    pub fn with_backend(timeout_ms: u64, backend: Arc<dyn ModelProvider>) -> Result<Self> {
        let mut provider = Self::new(timeout_ms)?;
        provider.backend = Some(backend);
        Ok(provider)
    }
    pub fn with_limits(mut self, limits: ProviderLimits) -> Result<Self> {
        anyhow::ensure!(
            limits.max_response_bytes > 0
                && limits.max_frame_bytes > 0
                && limits.max_frame_bytes <= limits.max_response_bytes
                && limits.max_tool_calls > 0
                && limits.demo_chunk_chars > 0,
            "invalid provider limits"
        );
        self.limits = limits;
        Ok(self)
    }
    /// Compatibility wrapper; typed errors remain available via downcast_ref.
    pub async fn complete(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
    ) -> Result<ModelResponse> {
        self.complete_typed(model, messages, tools, cancel)
            .await
            .map_err(Into::into)
    }
    pub async fn complete_typed(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse> {
        let operation = async {
            if let Some(backend) = &self.backend {
                return backend
                    .complete(model, messages, tools, cancel.clone())
                    .await;
            }
            if model.provider == ProviderKind::Demo {
                return Ok(self.demo(model, messages));
            }
            if model.provider == ProviderKind::Anthropic {
                let backend = self.anthropic().await?;
                return backend
                    .complete(model, messages, tools, cancel.clone())
                    .await;
            }
            if model.provider != ProviderKind::ChatCompletions {
                return Err(ProviderError::new(
                    ProviderErrorKind::Configuration,
                    "custom model requires an injected provider backend",
                ));
            }
            let response = self
                .request(model, messages, tools, false)?
                .send()
                .await
                .map_err(ProviderError::network)?;
            if !response.status().is_success() {
                return Err(http_error(response, self.limits.max_response_bytes).await);
            }
            let bytes = read_body(response, self.limits.max_response_bytes).await?;
            let value = serde_json::from_slice(&bytes)
                .map_err(|error| protocol(format!("invalid chat completion JSON: {error}")))?;
            decode_completion(value)
        };
        self.guarded(cancel.clone(), operation).await
    }
    /// The bounded channel applies backpressure. Only a complete [DONE] returns success.
    pub async fn complete_stream(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
        sender: mpsc::Sender<ProviderStreamEvent>,
    ) -> ProviderResult<ModelResponse> {
        let operation = async {
            if let Some(backend) = &self.backend {
                return backend
                    .complete_stream(model, messages, tools, cancel.clone(), sender.clone())
                    .await;
            }
            if model.provider == ProviderKind::Demo {
                let response = self.demo(model, messages);
                emit_completed_chunks(
                    model,
                    &response,
                    &cancel,
                    &sender,
                    self.limits.demo_chunk_chars,
                )
                .await?;
                return Ok(response);
            }
            if model.provider == ProviderKind::Anthropic {
                let backend = self.anthropic().await?;
                return backend
                    .complete_stream(model, messages, tools, cancel.clone(), sender.clone())
                    .await;
            }
            if model.provider != ProviderKind::ChatCompletions {
                return Err(ProviderError::new(
                    ProviderErrorKind::Configuration,
                    "custom model requires an injected provider backend",
                ));
            }
            let response = self
                .request(model, messages, tools, true)?
                .send()
                .await
                .map_err(ProviderError::network)?;
            if !response.status().is_success() {
                return Err(http_error(response, self.limits.max_response_bytes).await);
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            if !content_type
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
            {
                return Err(protocol("stream response must be text/event-stream"));
            }
            send_event(
                &sender,
                &cancel,
                ProviderStreamEvent::Start {
                    model_id: model.id.clone(),
                },
            )
            .await?;
            let mut chunks = response.bytes_stream();
            let mut parser = SseDecoder::new(self.limits.max_frame_bytes);
            let mut accumulator = StreamAccumulator::default();
            let mut received = 0usize;
            while let Some(chunk) = chunks.next().await {
                let chunk = chunk.map_err(ProviderError::network)?;
                received = received
                    .checked_add(chunk.len())
                    .ok_or_else(response_limit)?;
                if received > self.limits.max_response_bytes {
                    return Err(response_limit());
                }
                for frame in parser.feed(&chunk)? {
                    if frame.trim() == "[DONE]" {
                        let result = accumulator.finish()?;
                        send_event(&sender, &cancel, ProviderStreamEvent::Done).await?;
                        return Ok(result);
                    }
                    let value: Value = serde_json::from_str(&frame)
                        .map_err(|error| protocol(format!("invalid SSE JSON: {error}")))?;
                    for event in accumulator.apply(value, &self.limits)? {
                        send_event(&sender, &cancel, event).await?;
                    }
                }
            }
            Err(protocol("SSE disconnected before a complete [DONE] frame"))
        };
        self.guarded(cancel.clone(), operation).await
    }
    async fn guarded<T>(
        &self,
        cancel: CancellationToken,
        future: impl std::future::Future<Output = ProviderResult<T>>,
    ) -> ProviderResult<T> {
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(ProviderError::new(ProviderErrorKind::Cancelled, "cancelled")),
            result = timeout(Duration::from_millis(self.timeout_ms), future) => result.unwrap_or_else(|_| Err(ProviderError::new(ProviderErrorKind::Timeout, "provider request timeout"))),
        }
    }
    fn request(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        streaming: bool,
    ) -> ProviderResult<reqwest::RequestBuilder> {
        if model.max_output_tokens == 0
            || !["max_tokens", "max_completion_tokens"]
                .contains(&model.output_token_parameter.as_str())
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Configuration,
                "invalid output token limit or parameter",
            ));
        }
        let base = model.base_url.as_ref().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Configuration,
                format!("model {} has no base_url", model.id),
            )
        })?;
        let mut url = reqwest::Url::parse(base).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Configuration,
                "invalid provider base_url",
            )
        })?;
        if !["http", "https"].contains(&url.scheme())
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Configuration,
                "provider URL must use HTTP(S) without embedded credentials or fragment",
            ));
        }
        if !url.path().ends_with("/chat/completions") {
            url.set_path(&format!(
                "{}/chat/completions",
                url.path().trim_end_matches('/')
            ));
        }
        let mut request = json!({"model":model.model,"messages":messages,"stream":streaming,"n":1});
        request[&model.output_token_parameter] = json!(model.max_output_tokens);
        if streaming {
            request["stream_options"] = json!({"include_usage":true});
        }
        if !tools.is_empty() && model.supports_tools {
            request["tools"] = json!(tools);
        }
        if let Some(effort) = &model.reasoning_effort {
            request["reasoning_effort"] = json!(effort);
        }
        if !model.extra_body.is_object() {
            return Err(ProviderError::new(
                ProviderErrorKind::Configuration,
                "extra_body must be an object",
            ));
        }
        if let Some(map) = model.extra_body.as_object() {
            for (key, value) in map {
                if [
                    "model",
                    "messages",
                    "tools",
                    "stream",
                    "stream_options",
                    "n",
                    "max_tokens",
                    "max_completion_tokens",
                    "max_output_tokens",
                    "reasoning_effort",
                    "api_key",
                    "base_url",
                    "timeout",
                    "max_retries",
                ]
                .contains(&key.as_str())
                    || key == &model.output_token_parameter
                {
                    return Err(ProviderError::new(
                        ProviderErrorKind::Configuration,
                        format!("extra_body cannot override runtime-managed field {key}"),
                    ));
                }
                request[key] = value.clone();
            }
        }
        let mut builder = self.client.post(url).json(&request);
        if let Some(variable) = &model.api_key_env {
            let key = std::env::var(variable).map_err(|_| {
                ProviderError::new(
                    ProviderErrorKind::Configuration,
                    format!("environment variable {variable} is not set"),
                )
            })?;
            if key.trim().is_empty() {
                return Err(ProviderError::new(
                    ProviderErrorKind::Configuration,
                    format!("environment variable {variable} is empty"),
                ));
            }
            builder = builder.bearer_auth(key);
        }
        Ok(builder)
    }
    fn demo(&self, model: &ModelDescriptor, messages: &[Value]) -> ModelResponse {
        let prompt = messages
            .iter()
            .rev()
            .find(|message| message["role"] == "user")
            .and_then(|message| message["content"].as_str())
            .unwrap_or("");
        if let Some(last) = messages.last().filter(|message| message["role"] == "tool") {
            let text = format!(
                "【模拟模式；以下为真实工具结果】[DEMO simulation; real tool observation]\n{}",
                last["content"].as_str().unwrap_or("")
            );
            return ModelResponse {
                message: json!({"role":"assistant","content":text}),
                usage: Usage::default(),
                finish_reason: "stop".into(),
            };
        }
        let call = if let Some(path) = prompt.strip_prefix("/demo read ") {
            Some(("read_file", json!({"path":path.trim()})))
        } else if let Some(name) = prompt.strip_prefix("/demo skill ") {
            Some(("load_skill", json!({"name":name.trim()})))
        } else if let Some(body) = prompt.strip_prefix("/demo write ") {
            body.split_once('\n').map(|(path, content)| {
                ("write_file", json!({"path":path.trim(),"content":content}))
            })
        } else {
            None
        };
        if let Some((name, args)) = call {
            return ModelResponse {
                message: json!({"role":"assistant","content":null,"tool_calls":[{"id":uuid::Uuid::new_v4().to_string(),"type":"function","function":{"name":name,"arguments":args.to_string()}}]}),
                usage: Usage::default(),
                finish_reason: "tool_calls".into(),
            };
        }
        let text = format!(
            "【模拟模式】[DEMO simulation / {}] 本次没有调用真实语言模型。\n任务：{prompt}\n请配置 chat_completions 供应商以使用真实推理。输入 /demo read README.md 可验证实际工具调用循环。",
            model.id
        );
        ModelResponse {
            message: json!({"role":"assistant","content":text}),
            usage: Usage::default(),
            finish_reason: "stop".into(),
        }
    }
}
#[async_trait::async_trait]
impl ModelProvider for Provider {
    async fn complete(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse> {
        self.complete_typed(model, messages, tools, cancel).await
    }
    async fn complete_stream(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
        sender: mpsc::Sender<ProviderStreamEvent>,
    ) -> ProviderResult<ModelResponse> {
        Provider::complete_stream(self, model, messages, tools, cancel, sender).await
    }
}

fn response_limit() -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::ResponseLimit,
        "provider response exceeds configured byte limit",
    )
}
fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, message)
}
// OpenAI-compatible bodies may embed an `error` object with a 2xx status or
// inside an SSE frame. Classify typed errors the way the equivalent HTTP
// status would be: upstream/server failures behave like 5xx (retryable),
// rate-limit errors like 429; everything else stays a Protocol error.
fn embedded_error(error: &Value) -> ProviderError {
    let kind = match error.get("type").and_then(Value::as_str) {
        Some("server_error" | "api_error" | "upstream_error" | "internal_error") => {
            ProviderErrorKind::Server
        }
        Some("rate_limit_error" | "rate_limit_exceeded") => ProviderErrorKind::RateLimited,
        _ => ProviderErrorKind::Protocol,
    };
    let mut message = String::from("provider error: ");
    message.push_str(&error.to_string());
    ProviderError::new(kind, message)
}
async fn read_body(response: reqwest::Response, limit: usize) -> ProviderResult<Vec<u8>> {
    let mut chunks = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(ProviderError::network)?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(response_limit());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
async fn http_error(response: reqwest::Response, limit: usize) -> ProviderError {
    let status = response.status();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let retry_after_ms = retry_after
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000));
    let kind = if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        ProviderErrorKind::Authentication
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        ProviderErrorKind::RateLimited
    } else if status == StatusCode::REQUEST_TIMEOUT || status == StatusCode::GATEWAY_TIMEOUT {
        ProviderErrorKind::Timeout
    } else if status.is_server_error() {
        ProviderErrorKind::Server
    } else {
        ProviderErrorKind::Protocol
    };
    let description = read_body(response, limit.min(64 * 1024))
        .await
        .map(|bytes| {
            String::from_utf8_lossy(&bytes)
                .chars()
                .take(512)
                .collect::<String>()
        })
        .unwrap_or_else(|_| "unavailable or oversized error body".into());
    ProviderError {
        kind,
        message: format!("HTTP error: {description}"),
        status_code: Some(status.as_u16()),
        retry_after,
        retry_after_ms,
    }
}
fn parse_usage(value: &Value) -> ProviderResult<Usage> {
    if value.is_null() {
        return Ok(Usage::default());
    }
    if !value.is_object() {
        return Err(protocol("usage must be an object"));
    }
    let parse = |name: &str| -> ProviderResult<u64> {
        match value.get(name) {
            None => Ok(0),
            Some(token) => token
                .as_u64()
                .ok_or_else(|| protocol(format!("invalid usage {name}"))),
        }
    };
    Ok(Usage {
        input_tokens: parse("prompt_tokens")?,
        output_tokens: parse("completion_tokens")?,
    })
}
fn validate_message(message: &Value) -> ProviderResult<()> {
    if message["role"] != "assistant" {
        return Err(protocol("provider message must have assistant role"));
    }
    if !message["content"].is_null() && !message["content"].is_string() {
        return Err(protocol("assistant content must be a string or null"));
    }
    if let Some(calls) = message.get("tool_calls").filter(|value| !value.is_null()) {
        let calls = calls
            .as_array()
            .ok_or_else(|| protocol("tool_calls must be an array"))?;
        let mut ids = std::collections::BTreeSet::new();
        for call in calls {
            let id = call["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| protocol("tool call missing id"))?;
            if !ids.insert(id)
                || call["type"] != "function"
                || call["function"]["name"].as_str().is_none_or(str::is_empty)
            {
                return Err(protocol("invalid or duplicate function tool call"));
            }
            let arguments = call["function"]["arguments"]
                .as_str()
                .ok_or_else(|| protocol("tool arguments must be a JSON string"))?;
            let value: Value = serde_json::from_str(arguments)
                .map_err(|_| protocol("tool arguments are incomplete or invalid JSON"))?;
            if !value.is_object() {
                return Err(protocol("tool arguments must encode an object"));
            }
        }
    }
    Ok(())
}
fn validate_finish(reason: &str) -> ProviderResult<()> {
    match reason {
        "stop" | "tool_calls" => Ok(()),
        "length" => Err(ProviderError::new(
            ProviderErrorKind::OutputLimit,
            "model output reached its token limit",
        )),
        "content_filter" => Err(protocol("model output rejected by content filter")),
        _ => Err(protocol(format!("unsupported finish_reason {reason}"))),
    }
}
fn decode_completion(value: Value) -> ProviderResult<ModelResponse> {
    if let Some(error) = value.get("error") {
        return Err(embedded_error(error));
    }
    let choices = value["choices"]
        .as_array()
        .ok_or_else(|| protocol("provider response has no choices"))?;
    if choices.len() != 1 {
        return Err(protocol("provider must return exactly one choice"));
    }
    let choice = &choices[0];
    let finish = choice["finish_reason"]
        .as_str()
        .ok_or_else(|| protocol("provider response missing finish_reason"))?;
    validate_finish(finish)?;
    validate_message(&choice["message"])?;
    Ok(ModelResponse {
        message: choice["message"].clone(),
        finish_reason: finish.into(),
        usage: parse_usage(&value["usage"])?,
    })
}
async fn send_event(
    sender: &mpsc::Sender<ProviderStreamEvent>,
    cancel: &CancellationToken,
    event: ProviderStreamEvent,
) -> ProviderResult<()> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(ProviderError::new(ProviderErrorKind::Cancelled, "cancelled")), result = sender.send(event) => result.map_err(|_| ProviderError::new(ProviderErrorKind::ConsumerClosed, "stream consumer closed")) }
}
async fn emit_completed_chunks(
    model: &ModelDescriptor,
    response: &ModelResponse,
    cancel: &CancellationToken,
    sender: &mpsc::Sender<ProviderStreamEvent>,
    chunk_chars: usize,
) -> ProviderResult<()> {
    send_event(
        sender,
        cancel,
        ProviderStreamEvent::Start {
            model_id: model.id.clone(),
        },
    )
    .await?;
    if let Some(text) = response.message["content"].as_str() {
        let mut chunk = String::new();
        let mut count = 0usize;
        for character in text.chars() {
            chunk.push(character);
            count += 1;
            if count == chunk_chars {
                send_event(
                    sender,
                    cancel,
                    ProviderStreamEvent::TextDelta {
                        delta: std::mem::take(&mut chunk),
                    },
                )
                .await?;
                count = 0;
            }
        }
        if !chunk.is_empty() {
            send_event(
                sender,
                cancel,
                ProviderStreamEvent::TextDelta { delta: chunk },
            )
            .await?;
        }
    }
    if let Some(calls) = response.message["tool_calls"].as_array() {
        for (index, call) in calls.iter().enumerate() {
            send_event(
                sender,
                cancel,
                ProviderStreamEvent::ToolCallDelta {
                    index,
                    id: call["id"].as_str().map(str::to_owned),
                    name: call["function"]["name"].as_str().map(str::to_owned),
                    arguments: call["function"]["arguments"].as_str().map(str::to_owned),
                },
            )
            .await?;
        }
    }
    send_event(
        sender,
        cancel,
        ProviderStreamEvent::Usage {
            usage: response.usage.clone(),
        },
    )
    .await?;
    send_event(
        sender,
        cancel,
        ProviderStreamEvent::Finish {
            finish_reason: response.finish_reason.clone(),
        },
    )
    .await?;
    send_event(sender, cancel, ProviderStreamEvent::Done).await
}

struct SseDecoder {
    line: Vec<u8>,
    data: Vec<String>,
    frame_bytes: usize,
    max_frame_bytes: usize,
    first_line: bool,
    last_cr: bool,
}
impl SseDecoder {
    fn new(max_frame_bytes: usize) -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            frame_bytes: 0,
            max_frame_bytes,
            first_line: true,
            last_cr: false,
        }
    }
    fn feed(&mut self, chunk: &[u8]) -> ProviderResult<Vec<String>> {
        let mut frames = Vec::new();
        for byte in chunk {
            if self.last_cr && *byte == b'\n' {
                self.last_cr = false;
                continue;
            }
            self.last_cr = *byte == b'\r';
            self.frame_bytes = self.frame_bytes.saturating_add(1);
            if self.frame_bytes > self.max_frame_bytes {
                return Err(ProviderError::new(
                    ProviderErrorKind::ResponseLimit,
                    "SSE frame exceeds configured byte limit",
                ));
            }
            if *byte != b'\n' && *byte != b'\r' {
                self.line.push(*byte);
                continue;
            }
            let line = std::str::from_utf8(&self.line)
                .map_err(|_| protocol("SSE contains invalid UTF-8"))?;
            let line = if self.first_line {
                self.first_line = false;
                line.trim_start_matches('\u{feff}')
            } else {
                line
            };
            if line.is_empty() {
                if !self.data.is_empty() {
                    frames.push(self.data.join("\n"));
                    self.data.clear();
                }
                self.frame_bytes = 0;
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).to_owned());
            } else if line == "data" {
                self.data.push(String::new());
            }
            self.line.clear();
        }
        Ok(frames)
    }
}
#[derive(Default)]
struct ToolDelta {
    id: String,
    name: String,
    arguments: String,
}
#[derive(Default)]
struct StreamAccumulator {
    content: String,
    reasoning: String,
    tools: BTreeMap<usize, ToolDelta>,
    usage: Usage,
    finish_reason: Option<String>,
    saw_choice: bool,
}
// GLM and OpenRouter style streams carry reasoning under a `reasoning` delta
// key; DeepSeek style uses `reasoning_content`. Accept both, preferring
// `reasoning_content` when a provider sends the two redundantly.
fn reasoning_fragment(delta: &serde_json::Map<String, Value>) -> ProviderResult<Option<&str>> {
    let field = delta
        .get("reasoning_content")
        .filter(|value| !value.is_null())
        .map(|value| ("reasoning_content", value))
        .or_else(|| {
            delta
                .get("reasoning")
                .filter(|value| !value.is_null())
                .map(|value| ("reasoning", value))
        });
    match field {
        Some((name, value)) => value
            .as_str()
            .map(Some)
            .ok_or_else(|| protocol(format!("SSE {name} must be a string"))),
        None => Ok(None),
    }
}
fn choice_contributes(choice: &Value) -> bool {
    let text = |field: &str| {
        choice
            .get("delta")
            .and_then(|delta| delta.get(field))
            .filter(|value| !value.is_null())
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    };
    let calls = choice
        .get("delta")
        .and_then(|delta| delta.get("tool_calls"))
        .filter(|value| !value.is_null())
        .and_then(Value::as_array);
    text("content")
        || text("reasoning_content")
        || text("reasoning")
        || calls.is_some_and(|calls| !calls.is_empty())
}
impl StreamAccumulator {
    fn apply(
        &mut self,
        value: Value,
        limits: &ProviderLimits,
    ) -> ProviderResult<Vec<ProviderStreamEvent>> {
        if let Some(error) = value.get("error") {
            return Err(embedded_error(error));
        }
        let mut events = Vec::new();
        if let Some(usage) = value.get("usage").filter(|value| !value.is_null()) {
            self.usage = parse_usage(usage)?;
            events.push(ProviderStreamEvent::Usage {
                usage: self.usage.clone(),
            });
        }
        let choices = value["choices"]
            .as_array()
            .ok_or_else(|| protocol("SSE JSON has no choices array"))?;
        if choices.len() > 1 {
            return Err(protocol("SSE must contain at most one choice"));
        }
        for choice in choices {
            if choice["index"].as_u64() != Some(0) {
                return Err(protocol("SSE choice index must be zero"));
            }
            if self.finish_reason.is_some() {
                // Some OpenAI-compatible gateways append trailing frames after
                // the finish_reason chunk: a repeated choice with an empty
                // delta, or a final usage-only chunk. Tolerate empty tail
                // frames; reject choices that still carry model payload.
                if choice_contributes(choice) {
                    return Err(protocol("SSE choice continued after finish_reason"));
                }
                continue;
            }
            self.saw_choice = true;
            let delta = choice["delta"]
                .as_object()
                .ok_or_else(|| protocol("SSE choice missing delta object"))?;
            if let Some(role) = delta.get("role").filter(|value| !value.is_null())
                && role != "assistant"
            {
                return Err(protocol("SSE role must be assistant"));
            }
            if let Some(value) = delta.get("content").filter(|value| !value.is_null()) {
                let fragment = value
                    .as_str()
                    .ok_or_else(|| protocol("SSE content must be a string"))?;
                self.content.push_str(fragment);
                if !fragment.is_empty() {
                    events.push(ProviderStreamEvent::TextDelta {
                        delta: fragment.into(),
                    });
                }
            }
            if let Some(fragment) = reasoning_fragment(delta)? {
                self.reasoning.push_str(fragment);
                if !fragment.is_empty() {
                    events.push(ProviderStreamEvent::ReasoningDelta {
                        delta: fragment.into(),
                    });
                }
            }
            if let Some(calls) = delta.get("tool_calls").filter(|value| !value.is_null()) {
                let calls = calls
                    .as_array()
                    .ok_or_else(|| protocol("SSE tool_calls must be an array"))?;
                for call in calls {
                    let index = call["index"]
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| protocol("SSE tool call missing index"))?;
                    if index >= limits.max_tool_calls {
                        return Err(ProviderError::new(
                            ProviderErrorKind::ResponseLimit,
                            "too many SSE tool calls",
                        ));
                    }
                    if let Some(kind) = call.get("type")
                        && kind != "function"
                    {
                        return Err(protocol("SSE tool type must be function"));
                    }
                    let fragment = |value: Option<&Value>| -> ProviderResult<Option<String>> {
                        value
                            .filter(|value| !value.is_null())
                            .map(|value| {
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .ok_or_else(|| protocol("tool delta fields must be strings"))
                            })
                            .transpose()
                    };
                    let id = fragment(call.get("id"))?;
                    if call
                        .get("function")
                        .is_some_and(|function| !function.is_object())
                    {
                        return Err(protocol("tool delta function must be an object"));
                    }
                    let name = fragment(
                        call.get("function")
                            .and_then(|function| function.get("name")),
                    )?;
                    let arguments = fragment(
                        call.get("function")
                            .and_then(|function| function.get("arguments")),
                    )?;
                    let tool = self.tools.entry(index).or_default();
                    if let Some(value) = &id {
                        tool.id.push_str(value);
                    }
                    if let Some(value) = &name {
                        tool.name.push_str(value);
                    }
                    if let Some(value) = &arguments {
                        tool.arguments.push_str(value);
                    }
                    events.push(ProviderStreamEvent::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments,
                    });
                }
            }
            if self.finish_reason.is_none()
                && let Some(reason) = choice.get("finish_reason").filter(|value| !value.is_null())
            {
                let reason = reason
                    .as_str()
                    .ok_or_else(|| protocol("finish_reason must be a string"))?;
                validate_finish(reason)?;
                self.finish_reason = Some(reason.into());
                events.push(ProviderStreamEvent::Finish {
                    finish_reason: reason.into(),
                });
            }
        }
        Ok(events)
    }
    fn finish(self) -> ProviderResult<ModelResponse> {
        if !self.saw_choice {
            return Err(protocol("[DONE] arrived without a model choice"));
        }
        let reason = self
            .finish_reason
            .ok_or_else(|| protocol("[DONE] arrived without finish_reason"))?;
        let mut message = json!({"role":"assistant","content":if self.content.is_empty() && !self.tools.is_empty() { Value::Null } else { json!(self.content) }});
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = json!(self.reasoning);
        }
        if !self.tools.is_empty() {
            let calls = self.tools.into_iter().enumerate().map(|(expected, (index, tool))| { if expected != index { return Err(protocol("SSE tool indices must be contiguous")); } Ok(json!({"id":tool.id,"type":"function","function":{"name":tool.name,"arguments":tool.arguments}})) }).collect::<ProviderResult<Vec<_>>>()?;
            message["tool_calls"] = json!(calls);
        }
        validate_message(&message)?;
        if reason == "tool_calls" && message["tool_calls"].as_array().is_none_or(Vec::is_empty) {
            return Err(protocol("tool_calls finish_reason without tools"));
        }
        Ok(ModelResponse {
            message,
            usage: self.usage,
            finish_reason: reason,
        })
    }
}

pub struct ModelObject {
    pub descriptor: ModelDescriptor,
    pub provider: Provider,
}
#[async_trait::async_trait]
impl crate::object::Object for ModelObject {
    fn envelope(&self) -> crate::object::ObjectEnvelope {
        let mut object = crate::object::ObjectEnvelope::new(
            format!("model:{}", self.descriptor.id),
            crate::object::ObjectKind::Model,
            format!(
                "{:?} / {:?}",
                self.descriptor.provider, self.descriptor.tier
            ),
        );
        object.spec = serde_json::to_value(&self.descriptor).expect("model descriptor");
        object.capabilities.insert("complete".into(), json!({"type":"object","required":["messages"],"properties":{"messages":{"type":"array"},"tools":{"type":"array"}}}));
        object
    }
    async fn invoke(
        &self,
        method: &str,
        input: Value,
        context: crate::object::InvocationContext,
    ) -> Result<Value> {
        match method {
            "inspect" => Ok(serde_json::to_value(&self.descriptor)?),
            "complete" => {
                let messages = input["messages"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("messages array required"))?;
                let tools = input["tools"].as_array().cloned().unwrap_or_default();
                Ok(serde_json::to_value(
                    self.provider
                        .complete(&self.descriptor, messages, &tools, context.cancel)
                        .await?,
                )?)
            }
            _ => bail!("unknown model capability {method}"),
        }
    }
}
