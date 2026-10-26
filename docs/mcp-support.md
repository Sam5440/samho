# MCP 支持说明

本文描述 `rpi-core` 当前的 MCP 客户端边界。实现基于官方 Rust SDK
[`rmcp` 3.5.1](https://crates.io/crates/rmcp)，协议编解码、JSON-RPC 请求
ID、初始化协商和取消通知均由 SDK 完成。本文记录的是代码和集成测试已经验证的
能力，不把接口声明或上游设计文档当作完整 MCP 兼容性证明。

## 传输与配置

每个配置项对应一个独立的 MCP server Object。必须且只能配置 `command`（stdio）
或 `url`（Streamable HTTP）之一；一个 registry 中的 `name` 必须唯一，并且只允许
ASCII 字母、数字、`_`、`-`，长度为 1 到 32，`__` 保留给公开工具名空间。

```toml
[[mcp_servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
timeout_ms = 60000
startup_timeout_ms = 30000
scrub_env = true

[[mcp_servers]]
name = "remote"
url = "https://mcp.example.test/mcp"
bearer_token_env = "RPI_MCP_TOKEN"
timeout_ms = 30000
```

stdio 使用 `TokioChildProcess`，支持工作目录、显式环境变量和退出时回收子进程。
启用 `scrub_env` 时，继承环境中名称包含 `KEY`、`PASSWORD`、`SECRET` 或 `TOKEN`
的变量会被移除，然后再合并配置中的 `env`。Streamable HTTP 使用 reqwest，禁止
URL 中携带用户名或密码；`bearer_token_env` 只从指定环境变量读取令牌，并通过
Authorization header 发送。HTTP 客户端禁用自动重定向。

连接建立包含 `initialize` 协商，并受 `startup_timeout_ms` 限制。连接失败会保留为
该 server 的 `failed` 状态，不会移除健康 server 或使整个 registry 初始化失败。各
连接并发建立，registry 创建仍会等待全部连接尝试完成或到达启动超时。运行中
调用受 `timeout_ms` 限制，`dispose`/registry `close` 会取消请求、等待 SDK 关闭，
并在 stdio 场景回收子进程。

## 已实现的方法

| MCP 方法 | 当前行为 |
| --- | --- |
| `initialize` | 由 rmcp 在连接阶段自动协商；记录 server capabilities/info |
| `tools/list` | 支持 `nextCursor` 分页和最大页数保护；完整成功后原子替换工具 generation |
| `tools/call` | 传递结构化参数和 rich content；`isError: true` 映射为 `McpError::ToolFailure` |
| `resources/list` | 支持分页参数并返回原始资源描述 |
| `resources/templates/list` | 支持分页参数；兼容别名 `resource_templates/list` |
| `resources/read` | 按 URI 读取并保留返回内容 |
| `prompts/list` | 支持分页参数并返回 prompt 描述 |
| `prompts/get` | 传递 prompt 参数并保留消息内容 |
| `ping` | 转换为 SDK typed request 并校验空结果 |
| `notifications/tools/list_changed` | 标记 server 为 dirty；下次 `tools()` 时重新发现。发现失败保留上一份完整 generation，并将状态置为 `degraded` |

除表中基础方法外，optional fixture 已覆盖 `completion/complete`、`roots/list`、
`sampling`、form `elicitation`、`tasks/get|update|cancel`、resource subscribe/unsubscribe、
notifications 和本地 input/output Schema 校验。OAuth PKCE loopback、refresh、revoke
与凭据存储也有独立集成测试。

公开工具名使用 `mcp__<server>__<normalized-name>` 命名空间。原始名称中的其他
字符归一化为 `_`，名称长度超过 64 或归一化后发生同一 server 内碰撞会拒绝整个
新 generation；旧 generation 继续可用。跨 server 的名称由 server 命名空间隔离。

所有请求都经过 capability 检查。server 未声明 `tools`、`resources` 或 `prompts`
时，对应请求返回显式 `Unsupported` 错误。未知 server、未知公开工具、错误参数、
远端 JSON-RPC 错误、传输关闭、超时和取消均保留为带 server/method 的结构化错误，
不会静默转换成成功响应。

## Object 与调用接口

`McpRegistry::objects()` 为每个 server 暴露一个 `ObjectKind::McpServer`，Object id
为 `mcp:<name>`。Object 的 `inspect` 方法返回连接状态、传输类型、server info 和
工具数量；其他方法映射到上表中的 MCP 方法。`McpRegistry::tools()` 返回可直接
提供给模型的 function schema，`McpRegistry::call()` 按公开工具名路由到原始 MCP
工具名，`request()` 用于调用已实现的方法。

状态可能为 `connected`、`degraded`、`failed`、`disabled` 或 `closed`。`status()`
还返回 SDK 版本、已实现方法和明确的未实现列表，便于 UI 和复盘系统展示真实能力。

## 测试覆盖

真实协议 fixture 位于
[`crates/rpi-core/tests/fixtures/mcp_server.py`](../crates/rpi-core/tests/fixtures/mcp_server.py)，
使用 Python stdio peer；HTTP 测试使用 axum 启动真实 Streamable HTTP endpoint。运行：

```powershell
cargo test -p rpi-core --test mcp_integration -- --nocapture
cargo test -p rpi-core --test core_contracts -- --nocapture
```

集成测试当前覆盖：

- initialize、stdio 子进程回收、Streamable HTTP 握手和工具调用；
- 多页工具发现、schema、并发调用的 request ID 对应关系；
- resources、resource templates、prompts 和 Object `inspect`；
- 远端错误、`isError`、参数校验、超时、取消通知及关闭；
- 单个 server 失败不影响健康 server；重复 server 名称和工具名碰撞；
- `tools/list_changed` 后失败刷新保留上一份完整 generation。

这些是协议行为测试，不是吞吐量或真实第三方 server 的兼容性基准。网络延迟、模型
推理、第三方 server 自身错误和输出大小限制仍应在部署环境单独压测。

## 明确未实现的 MCP 能力

以下能力仍明确列为后续工作；调用会返回 `Unsupported`，或在初始化时不声明对应
client capability：

- OAuth metadata discovery 与动态 client registration；
- 已连接 HTTP transport 的自动 token refresh；
- URL elicitation、modern `subscriptions/listen` 和 server/discover 生命周期；
- MRTR `input_required` 的非 task completion；
- stdio 自动重连；
- 外部 JSON Schema `$ref` 解析；
- MCP 官方 conformance 认证。

当前已实现的 OAuth PKCE loopback、refresh/revoke、sampling、roots、form elicitation、
tasks、completion、资源订阅、输入/输出 Schema 校验均以本地 fixture 验证，但不等于
完整官方兼容性认证。增加上述后续能力时，应先扩展 capability negotiation 和结构化
错误，再补充真实 server fixture 与集成测试，避免只增加 JSON 包装而遗漏取消、生命周期
和失败隔离语义。
