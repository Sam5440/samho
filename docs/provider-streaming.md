# Provider 注入与流式调用说明

`rpi-core::provider` 提供两层接口：保留首版兼容的 `Provider::new`、
`Provider::complete`，以及可注入的异步 `ModelProvider`。嵌入方可以通过
`Provider::with_backend(timeout_ms, Arc<dyn ModelProvider>)` 注入自己的供应商，
不需要修改 Agent 核心循环。自定义实现必须响应 `CancellationToken`；流式实现必须
使用传入的有界 `tokio::sync::mpsc::Sender`，发送失败会返回 `ConsumerClosed`。

## 流式接口

```rust
let (sender, mut events) = tokio::sync::mpsc::channel(32);
let response = provider.complete_stream(
    &model,
    &messages,
    &tools,
    cancel.clone(),
    sender,
).await?;
```

事件顺序从 `Start` 开始，正文使用 `TextDelta`，推理字段使用
`ReasoningDelta`，函数调用增量使用 `ToolCallDelta`（包含 index、id、name、
arguments），服务端 usage 使用 `Usage`，终止原因使用 `Finish`，最后必须有
`Done`。自定义非流式 Provider 可以只实现 `complete`，trait 默认适配器会按
消息内容发出事件。

内置 Chat Completions Provider 要求响应为 `text/event-stream`，逐字节处理任意
TCP 分片、CRLF、UTF-8 BOM 和多行 `data:` 字段。只有收到完整 `[DONE]` 且已有
`finish_reason` 时才返回成功；中途断开、无效 JSON、工具参数无法合并成 JSON
对象都会返回 `Protocol`。`finish_reason=length` 返回 `OutputLimit`，不会被当作
成功结果。默认总响应上限为 16 MiB，单个 SSE 帧为 256 KiB，工具调用最多 128 个；
可用 `Provider::with_limits` 调整正数限制。

请求中的 `stream`、`stream_options`、`n`、模型、消息、工具、推理字段、输出 token
参数和 URL / 凭据字段由运行时管理。`ModelDescriptor.extra_body` 试图覆盖这些
键时返回 `Configuration`，避免插件设置绕过流式、预算或安全边界。供应商 URL 禁止
嵌入用户名、密码和 fragment；API Key 只从配置的环境变量读取。

## 错误分类

`ProviderError` 保留 `kind`、HTTP `status_code`、原始 `retry_after` 及毫秒换算值。
`Timeout`、`Network`、`RateLimited` 和 `Server` 可由上层按预算实施重试或模型降级；
认证、配置、协议、输出上限、响应上限、消费方关闭和取消均不可自动重试。HTTP 401/403
归类为 `Authentication`，429 为 `RateLimited`，5xx 为 `Server`（504 归类为超时）。
错误正文最多读取 64 KiB，错误消息最多引用 512 个字符；请求 URL（可能包含秘密）不会
进入网络错误消息。`Retry-After` 数字秒数会换算成毫秒，HTTP 日期形式保留原始头供上层解析。

Agent 层可按模型配置选择 `complete` 或 `complete_stream`，并将增量事件连接到 GUI/TUI
和调用轨迹。消费端必须与调用并发运行；若在同一任务先等待 Provider 完成再读有界队列，
会形成背压等待并在调用超时后失败。Provider 层用本地
HTTP fixture 验证网络分片、工具参数、取消、断流、错误状态、背压与 Demo 分块，不调用
付费模型。
