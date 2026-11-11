# 测试端口策略

所有本地 HTTP、Facet、OAuth 回调和浏览器/Smoke 测试端口必须在 **300–400（含边界）**。Rust 集成测试使用 `samho_core::test_support::bind_loopback()`，实际分配 301–399，并在启动 server 前保持 listener，避免并发 fixture 竞争。OAuth 测试回调使用 `callback_port_range = Some((301, 399))`。

- `scripts/smoke-web.ps1` 默认端口 374，并校验 300–400。
- `scripts/smoke_runtime.py` 默认端口 373，并在 300–400 中选择可用端口。
- `scripts/test-chinese-browser.mjs` Web 默认端口 374，Chrome CDP 默认端口 390，并拒绝范围外参数。

端口范围只约束测试和本地验收工具。生产服务端口仍可由 `http_bind` 或 `samho serve --bind` 自定义。若 300–400 全部被占用，fixture 应失败并报告无可用测试端口，不得退回 `:0`。
