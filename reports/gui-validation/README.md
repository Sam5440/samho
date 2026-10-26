# Rust RPi GUI/TUI smoke validation — 2026-10-07

环境：Windows 11，Rust debug binary，回环地址 `127.0.0.1:374`，独立 `runtime-data`，默认 Demo provider，无模型凭证、无付费请求。

浏览器使用真实服务加载内嵌 `web/index.html`、`style.css` 和 `app.js`，刷新后 10 个 API 请求均返回 200，控制台 error/warning 为 0。任务流程通过：自动分级任务选择 `demo-balanced`；同一会话再次提交时选择复杂度 7、`automatic=false`、`reasoning` tier，选择 `demo-reasoning`；会话列表和详情保留两轮 user/assistant 消息，事件面板显示 `task.start`、`model.route`、`model.start`、`model.response`、`task.done`，且 route 事件的 `object_id`、`parent_span_id` 和 trace ID 可查看。固定 `demo-frontier` 覆盖自动分级、9 项 Object 检查及 MCP/Skills 空状态通过；配置真实 MCP/Skill 的浏览器交互尚未验证。

桌面截图为 [`desktop.png`](desktop.png)，窄屏截图为 [`narrow.png`](narrow.png)。窄屏 viewport 为 560×900，无水平溢出。截图中的 `SIMULATION`、`[DEMO simulation / demo-…]` 标签是刻意的模拟标识，不能解读为真实模型质量或供应商兼容性证据。

详细结果见 [`browser-validation.md`](browser-validation.md)、[`browser-results.json`](browser-results.json) 与 [`browser-api-evidence.json`](browser-api-evidence.json)。

TUI 通过同一 API 服务启动并提交任务，在 Windows PTY 中显示 trace/model 状态，Ctrl+C 退出后终端恢复。Rust CLI 测试与静态 GUI 检查结果见 `docs/validation-plan.md`；完整 MCP conformance、真实 provider、跨平台像素一致性和上游 Pi 测试仍待后续验收。
