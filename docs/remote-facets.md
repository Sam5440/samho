# 远程 Facet 与多进程协作

`rpi-core::facet` 提供经过认证和版本协商的远程 Object 适配器。它是 RPi 内部
`rpi-facet/1` 协议，不冒充 MCP，也不自动启动任意远程命令。每个连接先握手，再把
允许的方法公开为 `Object` capability。独立服务可以在另一个进程或主机实现协议；
本仓库的 Python fixture 展示了一个完整、无第三方依赖的多进程 peer。

## 配置与注入

```toml
[[remote_facets]]
name = "worker"
object_id = "facet:worker"
endpoint = "https://worker.example.test/rpi"
auth_token_env = "RPI_WORKER_TOKEN"
allowed_capabilities = ["analyze", "index"]
timeout_ms = 30000
startup_timeout_ms = 10000
max_response_bytes = 4194304
allow_insecure_loopback = false
```

`FacetConfig` 是 `RemoteFacetConfig` 的公开别名。`validate()` 可用于配置阶段预检。
正常连接使用 `RemoteFacet::connect(&config, Arc<dyn NetworkPolicy>)`；它从指定环境
变量读取 Bearer token，未设置或空 token 均拒绝连接。嵌入与测试可以显式调用
`connect_with_token(config, token, cancel, policy)`，token 不参与配置序列化和 Object
检查输出。

所有 endpoint 必须使用 HTTPS，只有明确设置 `allow_insecure_loopback=true` 才能在
loopback 使用 HTTP。URL 禁止用户名、密码、query 和 fragment，HTTP 重定向禁用。
握手、调用和取消均经过注入的 `NetworkPolicy::authorize_url` 并携带 Bearer token。
必须配置非空 capability allowlist；握手版本、服务名和公开方法与配置不一致时连接失败。
客户端不能保证远端正确实施认证；部署端必须在三个接口都验证 token，fixture 已覆盖此点。

## 线协议

所有方法都是 endpoint 路径下的 JSON HTTP POST：

| 路径 | 请求 | 响应 |
| --- | --- | --- |
| `/handshake` | `protocol`、`name` | `protocol`、`name`、`version`、`capabilities`、`metadata` |
| `/invoke` | `protocol`、唯一 `call_id`、`method`、`input`、`trace_id`、`session_id`、子 `span_id`、`parent_span_id` | 相同 `protocol` / `call_id` 与 `result`，或 `error` |
| `/cancel` | `protocol`、`call_id` | 远端自行报告取消状态 |

`RemoteFacet::call(method, input, &InvocationContext)` 保留原 trace 和 session，生成
新的 16 位十六进制 span，并把上下文 span 作为 parent。trace ID 可转换为有效 32 位
十六进制时也发送 W3C `traceparent`，否则仍完整保留 JSON 中的原始关联 ID。返回值必须
匹配调用 ID 和协议版本，防止并发响应被误配。

## 取消、断连与生命周期

整体超时覆盖 DNS 策略、HTTP 头和响应正文；响应通过流式读取限制大小，默认不超过
4 MiB。取消或超时会停止等待本地调用，并尝试发送带认证的 `/cancel`，这个请求最多
等待 250 ms。它是尽力通知，不能保证远端已经停止外部副作用。没有自动重试，调用者
应先审查调用 ID 和实际副作用，再决定恢复策略。

Object `dispose()` 取消该 Facet 的生命周期令牌，活动调用会中断并通知远端；销毁后
新调用返回 `Disconnected`。网络断连、认证拒绝、版本不符、capability 拒绝、超时、
取消、响应超限均有 `FacetErrorKind`，不会被伪装成成功。

## 验证

```powershell
cargo test -p rpi-core --test facet_integration
```

测试使用本地 Axum fixture 与实际 Python 子进程，覆盖握手、版本校验、每次请求认证、
allowlist、trace/span 传播、取消通知、销毁、响应大小和断连。Python 子进程使用自己的
环境 token、动态 loopback 端口、`kill_on_drop` 和显式回收；测试不修改父进程环境变量。
这些证据验证协议和进程隔离边界，不代表已验证远程生产服务的网络性能或长期可用性。
