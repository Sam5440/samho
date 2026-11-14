# 测试端口策略

所有本地 HTTP、Facet、OAuth 回调和浏览器/Smoke 测试端口必须在 **1300–1400（含边界）**。Rust 集成测试使用 `samho_core::test_support::bind_loopback()`，实际分配 1301–1399，并在启动 server 前保持 listener，避免并发 fixture 竞争。OAuth 测试回调使用 `callback_port_range = Some((1301, 1399))`。

- `scripts/smoke-web.ps1` 默认端口 1374，并校验 1300–1400。
- `scripts/smoke_runtime.py` 默认端口 1373，并在 1300–1400 中选择可用端口。

选定该范围是为了跨平台可用：1024 以下端口在 Linux/macOS 上需要特权才能绑定，无法在 CI 与普通用户环境运行。端口范围只约束测试和本地验收工具。生产服务端口仍可由 `http_bind` 或 `samho serve --bind` 自定义。若 1300–1400 全部被占用，fixture 应失败并报告无可用测试端口，不得退回 `:0`。
