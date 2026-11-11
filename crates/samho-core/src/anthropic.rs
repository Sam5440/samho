//! Anthropic Messages API 供应商实现。
//!
//! 框架内部消息/工具格式贯穿 agent 循环、会话日志与 DTO，
//! 全部是 OpenAI 风格（`tool_calls` / `role:"tool"`）。协议差异在
//! 这里收敛：请求侧转换当前投影消息，响应侧把 Anthropic 内容块
//! 转回内部表示。流式复用主 Provider 的 SSE 解码器。
use crate::model::ModelDescriptor;
use crate::provider::{
    ModelProvider, ModelResponse, ProviderError, ProviderErrorKind, ProviderResult,
    ProviderStreamEvent, Usage,
};
use futures_util::StreamExt;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct AnthropicProvider {
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(ProviderError::network)?,
        })
    }

    fn endpoint(&self, base: &str) -> ProviderResult<reqwest::Url> {
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
        if !url.path().ends_with("/messages") {
            url.set_path(&format!("{}/v1/messages", url.path().trim_end_matches('/')));
        }
        Ok(url)
    }

    /// 内部 OpenAI 风格消息 → Anthropic Messages 协议。
    /// system 消息提升为顶层 `system`；tool_calls → `tool_use` 块；
    /// `role:"tool"` → 下一条 user 消息中的 `tool_result` 块。
    pub(super) fn convert_messages(
        messages: &[Value],
    ) -> ProviderResult<(Option<String>, Vec<Value>)> {
        let mut system_parts: Vec<String> = Vec::new();
        let mut converted: Vec<Value> = Vec::new();
        for message in messages {
            let role = message["role"].as_str().unwrap_or_default();
            match role {
                "system" => {
                    // Anthropic 只有顶层 system 字符串；显式 skill 注入等
                    // 系统 role 的内容按顺序拼接。user 前缀标记保留顺序语义。
                    let content = message["content"].as_str().unwrap_or_default();
                    system_parts.push(content.to_string());
                }
                "user" => {
                    converted.push(json!({"role":"user","content":json!([{"type":"text","text":message["content"].as_str().unwrap_or_default()}])}));
                }
                "assistant" => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(text) = message["content"].as_str().filter(|t| !t.is_empty()) {
                        blocks.push(json!({"type":"text","text":text}));
                    }
                    // 注意：不回传 thinking 块——网关对空 signature 的 thinking
                    // 历史没有承诺；实测省略即可正常多轮。
                    if let Some(calls) = message["tool_calls"].as_array() {
                        for call in calls {
                            let args = call["function"]["arguments"].as_str().unwrap_or_default();
                            let parsed: Value =
                                serde_json::from_str(args).unwrap_or_else(|_| json!({}));
                            blocks.push(json!({
                                "type":"tool_use",
                                "id":call["id"],
                                "name":call["function"]["name"],
                                "input":parsed,
                            }));
                        }
                    }
                    if blocks.is_empty() {
                        blocks.push(json!({"type":"text","text":"(no content)"}));
                    }
                    converted.push(json!({"role":"assistant","content":json!(blocks)}));
                }
                "tool" => {
                    let is_error = message["content"]
                        .as_str()
                        .is_some_and(|c| c.starts_with("{\"error\":"));
                    let result = json!({
                        "type":"tool_result",
                        "tool_use_id":message["tool_call_id"],
                        "content":message["content"],
                        "is_error":is_error,
                    });
                    // tool_result 属于下一条 user 消息；若当前最后一条已是
                    // user tool_result 批次则合并，否则新建。
                    if let Some(last) = converted.last_mut()
                        && last["role"] == "user"
                        && last["content"]
                            .as_array()
                            .is_some_and(|blocks| blocks.iter().all(|b| b["type"] == "tool_result"))
                    {
                        last["content"].as_array_mut().unwrap().push(result);
                    } else {
                        converted.push(json!({"role":"user","content":json!([result])}));
                    }
                }
                other => {
                    return Err(ProviderError::new(
                        ProviderErrorKind::Protocol,
                        format!("unsupported message role {other:?} in anthropic conversion"),
                    ));
                }
            }
        }
        // 协议要求首条消息为 user。非 user 开头（异常恢复/投影产物）时
        // 注入占位 user 保证协议合法，而不是硬失败。
        if converted.first().and_then(|m| m["role"].as_str()) != Some("user") {
            converted.insert(
                0,
                json!({"role":"user","content":json!([{"type":"text","text":"(session resume)"}])}),
            );
        }
        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };
        Ok((system, converted))
    }

    fn request_body(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        streaming: bool,
    ) -> ProviderResult<Value> {
        if model.max_output_tokens == 0 {
            return Err(ProviderError::new(
                ProviderErrorKind::Configuration,
                "invalid output token limit",
            ));
        }
        let (system, converted) = Self::convert_messages(messages)?;
        let mut body = Map::new();
        body.insert("model".into(), json!(model.model));
        body.insert("max_tokens".into(), json!(model.max_output_tokens));
        body.insert("stream".into(), json!(streaming));
        if let Some(system) = system {
            body.insert("system".into(), json!(system));
        }
        body.insert("messages".into(), json!(converted));
        if !tools.is_empty() && model.supports_tools {
            let mut seen = std::collections::BTreeSet::new();
            let tools = tools
                .iter()
                .filter_map(|schema| {
                    let name = schema["function"]["name"].as_str()?;
                    if !seen.insert(name.to_string()) {
                        return None;
                    }
                    // Anthropic 要求 input_schema 为完整合法 JSON Schema；
                    // description 取自外层 function 对象。
                    let mut input_schema = schema["function"]["parameters"]
                        .as_object()
                        .cloned()
                        .unwrap_or_default();
                    if !input_schema.contains_key("type") {
                        input_schema.insert("type".into(), json!("object"));
                    }
                    let description = schema["function"]["description"]
                        .as_str()
                        .unwrap_or_default();
                    Some(json!({
                        "name":name,
                        "description":description,
                        "input_schema":input_schema,
                    }))
                })
                .collect::<Vec<_>>();
            if !tools.is_empty() {
                body.insert("tools".into(), json!(tools));
            }
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
                    "max_tokens",
                    "stream",
                    "system",
                    "stop_sequences",
                    "temperature",
                    "top_p",
                    "metadata",
                    "api_key",
                    "base_url",
                ]
                .contains(&key.as_str())
                {
                    return Err(ProviderError::new(
                        ProviderErrorKind::Configuration,
                        format!("extra_body cannot override runtime-managed field {key}"),
                    ));
                }
                body.insert(key.clone(), value.clone());
            }
        }
        Ok(Value::Object(body))
    }

    async fn send(
        &self,
        model: &ModelDescriptor,
        body: Value,
        cancel: CancellationToken,
    ) -> ProviderResult<reqwest::Response> {
        let url = self.endpoint(model.base_url.as_deref().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Configuration,
                format!("model {} has no base_url", model.id),
            )
        })?)?;
        let mut builder = self
            .client
            .post(url)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body);
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
            builder = builder.header("x-api-key", key);
        }
        let response = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(ProviderError::new(ProviderErrorKind::Cancelled, "cancelled")),
            result = builder.send() => result.map_err(ProviderError::network)?,
        };
        Ok(response)
    }

    async fn read_body(response: reqwest::Response, limit: usize) -> ProviderResult<Vec<u8>> {
        let mut chunks = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(ProviderError::network)?;
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(ProviderError::new(
                    ProviderErrorKind::ResponseLimit,
                    "provider response exceeds configured byte limit",
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    fn decode(value: Value) -> ProviderResult<ModelResponse> {
        if let Some(error) = value.get("error") {
            return Err(embedded_error(error));
        }
        let stop_reason = value["stop_reason"].as_str().unwrap_or("end_turn");
        let finish_reason = match stop_reason {
            "end_turn" | "stop_sequence" => "stop",
            "tool_use" => "tool_calls",
            "max_tokens" => "length",
            "refusal" => "content_filter",
            other => other,
        };
        let mut content_text = String::new();
        let mut reasoning_text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        if let Some(blocks) = value["content"].as_array() {
            for block in blocks {
                match block["type"].as_str().unwrap_or_default() {
                    "text" => {
                        if let Some(text) = block["text"].as_str() {
                            if !content_text.is_empty() {
                                content_text.push('\n');
                            }
                            content_text.push_str(text);
                        }
                    }
                    "thinking" => {
                        if let Some(text) = block["thinking"].as_str() {
                            if !reasoning_text.is_empty() {
                                reasoning_text.push('\n');
                            }
                            reasoning_text.push_str(text);
                        }
                    }
                    "tool_use" => {
                        let id = block["id"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .ok_or_else(|| {
                                ProviderError::new(
                                    ProviderErrorKind::Protocol,
                                    "tool_use block missing id",
                                )
                            })?;
                        let name = block["name"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .ok_or_else(|| {
                                ProviderError::new(
                                    ProviderErrorKind::Protocol,
                                    "tool_use block missing name",
                                )
                            })?;
                        let input = block["input"].clone();
                        if !input.is_object() {
                            return Err(ProviderError::new(
                                ProviderErrorKind::Protocol,
                                "tool_use input must be an object",
                            ));
                        }
                        tool_calls.push(json!({
                            "id":id,
                            "type":"function",
                            "function":{"name":name,"arguments":serde_json::to_string(&input).unwrap_or_else(|_| "{}".into())},
                        }));
                    }
                    // redacted_thinking / server_tool_use 等忽略。
                    _ => {}
                }
            }
        }
        let mut message = json!({"role":"assistant","content":content_text});
        if !reasoning_text.is_empty() {
            message["reasoning_content"] = json!(reasoning_text);
        }
        if !tool_calls.is_empty() {
            message["tool_calls"] = json!(tool_calls);
        }
        if content_text.is_empty() && tool_calls.is_empty() {
            if stop_reason == "max_tokens" {
                return Err(ProviderError::new(
                    ProviderErrorKind::OutputLimit,
                    "model output reached its token limit (thinking-only response truncated)",
                ));
            }
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "anthropic response carried neither text nor tool_use",
            ));
        }
        let usage_value = &value["usage"];
        let usage = Usage {
            input_tokens: usage_value["input_tokens"].as_u64().unwrap_or(0),
            // 网关把输出 token 计入 output_tokens（含 thinking）。
            output_tokens: usage_value["output_tokens"].as_u64().unwrap_or(0),
        };
        Ok(ModelResponse {
            message,
            usage,
            finish_reason: finish_reason.into(),
        })
    }
}

#[async_trait::async_trait]
impl ModelProvider for AnthropicProvider {
    async fn complete(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
    ) -> ProviderResult<ModelResponse> {
        let body = self.request_body(model, messages, tools, false)?;
        let response = self.send(model, body, cancel.clone()).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
        }
        let bytes = Self::read_body(
            response,
            crate::provider::ProviderLimits::default().max_response_bytes,
        )
        .await?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
            ProviderError::new(
                ProviderErrorKind::Protocol,
                format!("invalid anthropic response JSON: {error}"),
            )
        })?;
        Self::decode(value)
    }

    async fn complete_stream(
        &self,
        model: &ModelDescriptor,
        messages: &[Value],
        tools: &[Value],
        cancel: CancellationToken,
        sender: mpsc::Sender<ProviderStreamEvent>,
    ) -> ProviderResult<ModelResponse> {
        let body = self.request_body(model, messages, tools, true)?;
        let response = self.send(model, body, cancel.clone()).await?;
        if !response.status().is_success() {
            return Err(http_error(response).await);
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
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "stream response must be text/event-stream",
            ));
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
        let mut acc = AnthropicStreamAccumulator::default();
        while let Some(chunk) = chunks.next().await {
            let chunk = chunk.map_err(ProviderError::network)?;
            for event in acc.feed(&chunk)?.into_iter().flatten() {
                send_event(&sender, &cancel, event).await?;
            }
        }
        let result = acc.finish()?;
        send_event(&sender, &cancel, ProviderStreamEvent::Done).await?;
        Ok(result)
    }
}

async fn send_event(
    sender: &mpsc::Sender<ProviderStreamEvent>,
    cancel: &CancellationToken,
    event: ProviderStreamEvent,
) -> ProviderResult<()> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(ProviderError::new(ProviderErrorKind::Cancelled, "cancelled")), result = sender.send(event) => result.map_err(|_| ProviderError::new(ProviderErrorKind::ConsumerClosed, "stream consumer closed")) }
}

async fn http_error(response: reqwest::Response) -> ProviderError {
    let status = response.status();
    let kind = if status.as_u16() == 401 || status.as_u16() == 403 {
        ProviderErrorKind::Authentication
    } else if status.as_u16() == 429 {
        ProviderErrorKind::RateLimited
    } else if status.as_u16() == 408 || status.as_u16() == 504 {
        ProviderErrorKind::Timeout
    } else if status.is_server_error() {
        ProviderErrorKind::Server
    } else {
        ProviderErrorKind::Protocol
    };
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let retry_after_ms = retry_after
        .as_deref()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|seconds| seconds.saturating_mul(1000));
    let body = AnthropicProvider::read_body(response, 64 * 1024)
        .await
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|text| text.chars().take(512).collect::<String>())
        .unwrap_or_else(|| "unavailable or oversized error body".into());
    ProviderError {
        kind,
        message: format!("HTTP error: {body}"),
        status_code: Some(status.as_u16()),
        retry_after,
        retry_after_ms,
    }
}

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

/// Anthropic SSE 事件累积器：
/// message_start(content_block_start) → content_block_delta(text/thinking/input_json_delta)
/// → content_block_stop → message_delta(stop_reason, usage) → message_stop。
#[derive(Default)]
struct AnthropicStreamAccumulator {
    text: String,
    reasoning: String,
    tools: std::collections::BTreeMap<u64, AnthropicToolAccumulator>,
    input_tokens: u64,
    output_tokens: u64,
    stop_reason: Option<String>,
    saw_message_start: bool,
    finished: bool,
    /// SSE 增量解析状态：跨 chunk 的残余字节、当前 event 名与 data 聚合。
    pending: Vec<u8>,
    current_event: String,
    current_data: String,
}

#[derive(Default)]
struct AnthropicToolAccumulator {
    id: String,
    name: String,
    input_json: String,
}

impl AnthropicStreamAccumulator {
    fn feed(&mut self, chunk: &[u8]) -> ProviderResult<Vec<Option<ProviderStreamEvent>>> {
        let mut events = Vec::new();
        let chunks = self.parse_sse(chunk)?;
        for (event_type, data) in chunks {
            let value: Value = serde_json::from_str(&data).map_err(|error| {
                ProviderError::new(
                    ProviderErrorKind::Protocol,
                    format!("invalid anthropic SSE JSON: {error}"),
                )
            })?;
            if let Some(error) = value.get("error") {
                return Err(embedded_error(error));
            }
            match event_type.as_str() {
                "message_start" => {
                    if self.saw_message_start {
                        return Err(ProviderError::new(
                            ProviderErrorKind::Protocol,
                            "duplicate anthropic message_start",
                        ));
                    }
                    self.saw_message_start = true;
                    self.input_tokens = value["message"]["usage"]["input_tokens"]
                        .as_u64()
                        .unwrap_or(0);
                }
                "content_block_start" => {
                    let index = value["index"].as_u64().unwrap_or(0);
                    let block_type = value["content_block"]["type"].as_str().unwrap_or_default();
                    if block_type == "tool_use" {
                        let tool = self.tools.entry(index).or_default();
                        tool.id = value["content_block"]["id"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                        tool.name = value["content_block"]["name"]
                            .as_str()
                            .unwrap_or_default()
                            .into();
                    }
                }
                "content_block_delta" => {
                    let index = value["index"].as_u64().unwrap_or(0);
                    let delta = &value["delta"];
                    match delta["type"].as_str().unwrap_or_default() {
                        "text_delta" => {
                            let fragment = delta["text"].as_str().unwrap_or_default();
                            self.text.push_str(fragment);
                            if !fragment.is_empty() {
                                events.push(Some(ProviderStreamEvent::TextDelta {
                                    delta: fragment.into(),
                                }));
                            }
                        }
                        "thinking_delta" => {
                            let fragment = delta["thinking"].as_str().unwrap_or_default();
                            self.reasoning.push_str(fragment);
                            if !fragment.is_empty() {
                                events.push(Some(ProviderStreamEvent::ReasoningDelta {
                                    delta: fragment.into(),
                                }));
                            }
                        }
                        "input_json_delta" => {
                            let fragment = delta["partial_json"].as_str().unwrap_or_default();
                            let tool = self.tools.entry(index).or_default();
                            tool.input_json.push_str(fragment);
                            let index_usize = usize::try_from(index).unwrap_or(usize::MAX);
                            events.push(Some(ProviderStreamEvent::ToolCallDelta {
                                index: index_usize,
                                id: if tool.id.is_empty() {
                                    None
                                } else {
                                    Some(tool.id.clone())
                                },
                                name: if tool.name.is_empty() {
                                    None
                                } else {
                                    Some(tool.name.clone())
                                },
                                arguments: if fragment.is_empty() {
                                    None
                                } else {
                                    Some(fragment.into())
                                },
                            }));
                        }
                        _ => {}
                    }
                }
                "content_block_stop" => {}
                "message_delta" => {
                    if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                        self.stop_reason = Some(reason.into());
                    }
                    if let Some(output) = value["usage"]["output_tokens"].as_u64() {
                        self.output_tokens = output;
                    }
                }
                "message_stop" => {
                    self.finished = true;
                }
                "ping" => {}
                "error" => {
                    return Err(embedded_error(&value));
                }
                _ => {}
            }
        }
        Ok(events)
    }

    fn finish(self) -> ProviderResult<ModelResponse> {
        if !self.saw_message_start {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "anthropic stream ended without message_start",
            ));
        }
        if !self.finished {
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "anthropic stream ended before message_stop",
            ));
        }
        let stop_reason = self
            .stop_reason
            .clone()
            .unwrap_or_else(|| "end_turn".into());
        let finish_reason = match stop_reason.as_str() {
            "end_turn" | "stop_sequence" => "stop",
            "tool_use" => "tool_calls",
            "max_tokens" => "length",
            "refusal" => "content_filter",
            other => other,
        };
        let mut message = json!({"role":"assistant","content":self.text});
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = json!(self.reasoning);
        }
        if !self.tools.is_empty() {
            let calls = self
                .tools
                .values()
                .map(|tool| {
                    // 空参输入规范化为 "{}"，避免产出非法 JSON 参数串。
                    let args_str = if tool.input_json.is_empty() {
                        "{}".to_string()
                    } else {
                        tool.input_json.clone()
                    };
                    let parsed: Value = serde_json::from_str(&args_str).map_err(|_| {
                        ProviderError::new(
                            ProviderErrorKind::Protocol,
                            "tool arguments are incomplete or invalid JSON",
                        )
                    })?;
                    if !parsed.is_object() {
                        return Err(ProviderError::new(
                            ProviderErrorKind::Protocol,
                            "tool arguments must encode an object",
                        ));
                    }
                    Ok(json!({
                        "id":tool.id,
                        "type":"function",
                        "function":{"name":tool.name,"arguments":args_str},
                    }))
                })
                .collect::<ProviderResult<Vec<_>>>()?;
            message["tool_calls"] = json!(calls);
        }
        if self.text.is_empty() && self.tools.is_empty() {
            // 只产出 thinking 且被 max_tokens 截断 → 按输出上限错误分类。
            if self.stop_reason.as_deref() == Some("max_tokens") {
                return Err(ProviderError::new(
                    ProviderErrorKind::OutputLimit,
                    "model output reached its token limit (thinking-only response truncated)",
                ));
            }
            return Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "anthropic stream carried neither text nor tool_use",
            ));
        }
        Ok(ModelResponse {
            message,
            usage: Usage {
                input_tokens: self.input_tokens,
                output_tokens: self.output_tokens,
            },
            finish_reason: finish_reason.into(),
        })
    }
}

/// 手写增量 SSE 行解析状态机，保序产出 (event, data) 对；跨 chunk
/// 的残余字节存放在 accumulator 的 `pending` 字段。
impl AnthropicStreamAccumulator {
    fn parse_sse(&mut self, chunk: &[u8]) -> ProviderResult<Vec<(String, String)>> {
        let mut pending = std::mem::take(&mut self.pending);
        pending.extend_from_slice(chunk);
        let mut frames = std::collections::VecDeque::new();
        while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=position).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]);
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                if !self.current_event.is_empty() || !self.current_data.is_empty() {
                    frames.push_back((
                        std::mem::take(&mut self.current_event),
                        std::mem::take(&mut self.current_data),
                    ));
                }
            } else if let Some(value) = line.strip_prefix("event:") {
                self.current_event = value.strip_prefix(' ').unwrap_or(value).to_string();
            } else if let Some(value) = line.strip_prefix("data:") {
                let value = value.strip_prefix(' ').unwrap_or(value);
                if !self.current_data.is_empty() {
                    self.current_data.push('\n');
                }
                self.current_data.push_str(value);
            } else if line.starts_with(':') {
                // SSE comment line; ignore.
            }
        }
        self.pending = pending;
        Ok(frames.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ProviderKind;

    #[test]
    fn convert_messages_maps_roles_and_blocks() {
        let messages = vec![
            json!({"role":"system","content":"policy"}),
            json!({"role":"user","content":"task"}),
            json!({"role":"assistant","content":null,"tool_calls":[
                {"id":"a","type":"function","function":{"name":"shell","arguments":"{\"command\":\"ls\"}"}}
            ]}),
            json!({"role":"tool","tool_call_id":"a","content":"Cargo.toml\nsrc"}),
            json!({"role":"assistant","content":"done"}),
        ];
        let (system, converted) = AnthropicProvider::convert_messages(&messages).unwrap();
        assert_eq!(system.as_deref(), Some("policy"));
        assert_eq!(
            converted[0]["content"][0]["text"],
            json!("task"),
            "messages[0] should be a text block"
        );
        // assistant tool_calls → tool_use
        assert_eq!(
            converted[1]["content"][0],
            json!({"type":"tool_use","id":"a","name":"shell","input":{"command":"ls"}})
        );
        // role:tool → user tool_result
        assert_eq!(converted[2]["content"][0]["type"], json!("tool_result"));
        // final assistant text block
        assert_eq!(converted[3]["content"][0]["text"], json!("done"));
    }

    #[test]
    fn convert_merges_two_tool_results_into_one_user_message() {
        let messages = vec![
            json!({"role":"user","content":"go"}),
            json!({"role":"assistant","content":null,"tool_calls":[
                {"id":"a","type":"function","function":{"name":"x","arguments":"{}"}},
                {"id":"b","type":"function","function":{"name":"y","arguments":"{}"}}
            ]}),
            json!({"role":"tool","tool_call_id":"a","content":"ra"}),
            json!({"role":"tool","tool_call_id":"b","content":"rb"}),
        ];
        let (_, converted) = AnthropicProvider::convert_messages(&messages).unwrap();
        assert_eq!(
            converted.len(),
            3,
            "tool results must merge into one user message"
        );
        assert_eq!(converted[2]["content"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn decode_maps_stop_reasons_and_blocks() {
        let response = json!({
            "content":[
                {"type":"thinking","thinking":"reason","signature":"sig"},
                {"type":"text","text":"hello"},
                {"type":"tool_use","id":"t1","name":"shell","input":{"command":"ls"}}
            ],
            "stop_reason":"tool_use",
            "usage":{"input_tokens":10,"output_tokens":5}
        });
        let decoded = AnthropicProvider::decode(response).unwrap();
        assert_eq!(decoded.finish_reason, "tool_calls");
        assert_eq!(decoded.message["content"], json!("hello"));
        assert_eq!(decoded.message["reasoning_content"], json!("reason"));
        let calls = decoded.message["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["id"], json!("t1"));
        assert_eq!(
            serde_json::from_str::<Value>(calls[0]["function"]["arguments"].as_str().unwrap())
                .unwrap(),
            json!({"command":"ls"})
        );
        assert_eq!(decoded.usage.output_tokens, 5);
    }

    #[test]
    fn stream_finish_rejects_truncated_json() {
        let mut acc = AnthropicStreamAccumulator::default();
        let sse = concat!(
            "event: message_start\n",
            "data: {\"message\":{\"usage\":{\"input_tokens\":3}}}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t\",\"name\":\"x\"}}\n",
            "\n",
            "event: content_block_delta\n",
            "data: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"a\\\":\"}}\n",
            "\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n",
            "\n",
            "event: message_stop\n",
            "data: {}\n",
            "\n"
        );
        acc.feed(sse.as_bytes()).unwrap();
        assert!(acc.finish().is_err(), "truncated tool JSON must fail");
    }

    #[test]
    fn stream_finish_normalizes_missing_tool_input_to_empty_object() {
        let mut acc = AnthropicStreamAccumulator::default();
        let sse = concat!(
            "event: message_start\n",
            "data: {\"message\":{\"usage\":{\"input_tokens\":1}}}\n",
            "\n",
            "event: content_block_start\n",
            "data: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"todo_read\"}}\n",
            "\n",
            "event: message_delta\n",
            "data: {\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":4}}\n",
            "\n",
            "event: message_stop\n",
            "data: {}\n",
            "\n"
        );
        acc.feed(sse.as_bytes()).unwrap();
        let response = acc.finish().expect("no-delta tool_use must be valid");
        let calls = response.message["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["function"]["arguments"], json!("{}"));
    }

    #[test]
    fn convert_inserts_placeholder_user_when_history_starts_non_user() {
        let messages = vec![json!({"role":"assistant","content":"recovered tail"})];
        let (_, converted) = AnthropicProvider::convert_messages(&messages).unwrap();
        assert_eq!(converted[0]["role"], json!("user"));
        assert_eq!(converted[1]["role"], json!("assistant"));
    }

    #[test]
    fn request_body_maps_tool_schemas_with_description_and_dedup() {
        let model = ModelDescriptor {
            id: "glm-5.3-flash".into(),
            model: "auto/glm-5.3-flash".into(),
            provider: ProviderKind::Anthropic,
            base_url: Some("http://127.0.0.1:3425".into()),
            supports_tools: true,
            max_output_tokens: 4096,
            ..ModelDescriptor::default()
        };
        let messages = vec![json!({"role":"user","content":"hi"})];
        let tools = vec![
            json!({"type":"function","function":{"name":"shell","description":"Run a shell command","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}}}),
            // 同名重复注册必须被去重。
            json!({"type":"function","function":{"name":"shell","description":"dup","parameters":{"type":"object"}}}),
            json!({"type":"function","function":{"name":"read_file","description":"Read file window","parameters":{"type":"object"}}}),
        ];
        let provider = AnthropicProvider::new().unwrap();
        let body = provider
            .request_body(&model, &messages, &tools, false)
            .unwrap();
        let tools_out = body["tools"].as_array().unwrap();
        assert_eq!(tools_out.len(), 2, "duplicate tool names must be deduped");
        assert_eq!(tools_out[0]["name"], json!("shell"));
        assert_eq!(tools_out[0]["description"], json!("Run a shell command"));
        assert_eq!(tools_out[0]["input_schema"]["type"], json!("object"));
        assert_eq!(tools_out[0]["input_schema"]["required"], json!(["command"]));
        assert_eq!(body["max_tokens"], json!(4096));
        assert_eq!(body["model"], json!("auto/glm-5.3-flash"));
        assert_eq!(body["system"], Value::Null, "no system messages → absent");
    }
}
