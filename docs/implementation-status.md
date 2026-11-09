# RPi 当前实现状态（2026-10-22）

本文以当前仓库源码和可复现测试证据为准。网页确认计划中的“已完成（首版已验证）”“部分完成”和“未完成”是确认时基线；本页记录本轮增量后的真实状态，未实测的平台、真实模型和长期 soak 不写成通过。

## 已实现并由本地 fixture 验证

- Rust workspace、Object 注册表和生命周期：依赖检查、循环拒绝、激活回滚、逆序销毁、scope、middleware、generation lease、热替换失败回滚。
- Agent 任务执行：取消、并发限制、恢复检查点、未完成工具调用的未知副作用标记、任务/会话 Object、显式 task id、steering/follow-up 顺序、fork 父子关系。
- Provider：Demo 模拟器和 Chat Completions；SSE 增量文本、reasoning、工具参数拼接、usage、断流、typed error、Retry-After、取消、bounded backpressure。模型流输出通过有界通道进入 TUI/Web 的同一进度快照。
- 上下文与预算：确定性投影、system 与完整工具批次保护、SHA-256 lineage、SQLite/file journal、请求前 worst-case reservation、累计 task/total ceiling、未知失败不退款。
- 路由：Nano/Fast/Balanced/Reasoning/Frontier/Specialist 六级策略；自定义 intelligence tier、能力/上下文/预算/延迟/specialty 过滤、显式模型覆盖和 retry reroute 理由。供应商 reasoning effort 仍独立于策略层级。
- MCP/Skill：stdio、Streamable HTTP、tools/resources/prompts、分页、取消/超时、roots、completion、sampling/elicitation/tasks 的本地 fixture；OAuth PKCE/refresh/revoke；SKILL.md 元数据、冲突诊断、惰性加载、resources、ignore 和 allowed-tools。未宣告的能力返回显式错误。
- 日志与复盘：JSONL/SQLite 增量 journal、序号与 span 父子关系、索引分页、rotation/retention、SSE gap 恢复、跨 chunk 脱敏、中文 timeline/export。默认不记录完整内容；开启后仍走 Redactor。
- 界面与扩展：RuntimeBuilder 注入 provider/router/projector/storage/tool policy/MCP hooks/network policy；Remote Facet 版本握手、Bearer、allowlist、traceparent、取消/断连；Web/TUI 共用 Runtime，Web 具备 progress SSE、fork、steer、follow-up、trace page/export、UI 配置和 workspace 选择。

## 本轮测试证据

`cargo test --workspace` 当前通过 **135 项**（CLI 38、core 单元 19、agent 6、contracts 15、facet 6、MCP 5、MCP optional 4、OAuth 3、provider streaming 15、runtime semantics 10、skill policy 5、storage lifecycle 9，另 0 个 doc test）。`cargo clippy --workspace --all-targets -- -D warnings` 和 `cargo fmt --all -- --check` 已通过。

测试 HTTP/Facet/OAuth fixture 使用 `rpi_core::test_support::bind_loopback()`，只在 301–399 中轮询并在服务启动前持有 listener；OAuth 测试回调为 301–399，脚本 smoke/Web/Chrome CDP 端口也强制 300–400。生产运行时仍允许用户配置任意 loopback 端口；“300–400”是测试约束，不是模型或网络协议限制。

浏览器中文页面和计划页面已有离线 Node/CDP 验收；Web API 行为测试覆盖健康检查、任务完成、live progress、fork、UI、事件分页、timeline/export 和取消。测试只使用 Demo、trait provider 和本地 HTTP/stdio/Python fixture，没有付费模型调用。

## 仍然未验收或只部分实现

- 真实供应商凭据、真实模型质量和成本：未运行；Chat Completions 协议仅以本地 fixture 验证。
- macOS/Linux 的实际构建、TUI 像素/终端差异和跨平台安装：CI workflow 已提交但本地未运行，Windows release smoke 有记录。
- 上游 Pi 全量测试：仅保留既有选择性结果；Windows TUI 的 symlink/path 差异和根 npm PATH 失败仍按原报告记录。
- 24 小时 soak、同机等价 Pi/Rust p50/p95/RSS 对照：已有模拟器样本和方法，尚未形成可宣称的相对倍率。
- MCP OAuth metadata discovery、动态 client registration、modern subscriptions/listen、automatic stdio reconnect、外部 JSON Schema ref 等能力仍明确列在 not_implemented。
- Remote Facet 当前是受认证和 allowlist 保护的 HTTP 客户端；没有宣称任意远程 Object 执行、进程隔离或生产级远程认证。

## 证据和回滚原则

每个失败保留原始输出和 trace；展示式复盘不会重新执行工具。崩溃或取消后，已发出的外部副作用标为未知，恢复只补 tool observation，不自动重跑。性能报告必须使用同机、同输入、同配置的样本，不从单次结果推导相对 Pi 倍率。
