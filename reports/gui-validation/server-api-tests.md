# RPi 本地 Web API / SSE 验收记录

日期：2026-10-22  
范围：`crates/rpi-cli/src/server.rs` 的路由契约与内存 `Router` 集成测试  

## 已执行

```text
cargo test -p rpi --bin rpi server::tests -- --nocapture
```

结果：**10 passed，0 failed，0 ignored**。

覆盖的行为包括：

- `/api/events/page` 的 trace 过滤、游标分页和 `has_more`；
- `/api/traces/{id}/timeline` 的展示式复盘语义；
- `/api/traces/{id}/export` 的 Markdown 下载类型、文件名和中文报告；
- `/api/sessions/{id}/fork` 的父子会话关系、切点和中文标题；
- `/api/ui` 的读取、配置持久化、主题校验和非法配置拒绝；
- `/api/live/stream` 在提交任务后发送 `progress` SSE；
- `/api/tasks/{id}/steer` 与 `/follow-up` 的确定性序列、类型和排队状态；
- 现有的任务提交、取消、并发限制、事件 SSE 游标、重启关闭和凭据脱敏检查。

## 测试端口约束

HTTP Provider fixture 使用 `rpi_core::test_support::bind_loopback`，只从 `301..=399` 选择回环端口，避免使用 `:0`。本文件中的路由 `Router` 测试不监听网络端口。OAuth 回调等其他模块应继续复用同一 helper。

## 证据边界

这些是本地模拟 Provider 和 `Router` 集成检查；没有调用真实模型、没有部署，也没有证明跨平台浏览器或长时间 SSE soak。`trace export` 的大于 16 MiB 拒绝路径仍需在独立故障注入阶段覆盖。
