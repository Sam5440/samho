# Anthropic 默认 API 接入说明（供评测 Agent 使用）

更新：2026-11-10。本框架（D:\ddcode\samho）已完成 Anthropic Messages 协议原生支持，并把
`http://127.0.0.1:3425` / `auto/glm-5.3-flash` 作为默认 API。

## 一、给评测 Agent 的最短可用路径

1. 构建并准备目录：

```bash
cd D:/ddcode/rpi && cargo build -p samho
mkdir -p D:/ddcode/rpi-tests/workspace-anth D:/ddcode/rpi-tests/data-anth
```

2. 设置密钥环境变量（启动 serve 的进程里）：

```bash
export SAMHO_API_KEY=magpie     # Windows 永久: setx SAMHO_API_KEY magpie
```

3. 启动服务（canonical 配置已就绪）：

```bash
D:/ddcode/rpi/target/debug/samho.exe serve --config D:/ddcode/rpi/examples/samho.anthropic.toml
```

服务地址 `http://127.0.0.1:7434`。提交任务：`POST /api/run`，body：

```json
{"prompt": "<任务书全文>", "profile": {"automatic": false, "complexity": 7}}
```

轮询：`GET /api/tasks/{task_id}` 直到 `status` 为 `done`/`failed`/`cancelled`。
会话续跑：同 body 追加 `"session_id": "<第一次返回的 session_id>"`。

4. 或者一键生成模板配置（内容即本默认 API）：

```bash
D:/ddcode/rpi/target/debug/samho.exe config init samho.toml
```

## 二、协议支持现状

- `provider = "anthropic"`：POST `{base_url}/v1/messages`，`x-api-key` 头 +
  `anthropic-version: 2023-06-01`；工具 schema 自动从 OpenAI function 格式转换
  （`parameters` → `input_schema`，同名去重）；多工具批次结果自动合并进单条 user
  消息（`tool_result` 块）；system 消息提升为顶层 `system` 参数。
- 流式与非流式都已实现并经真实网关端到端冒烟验证（多轮工具循环、中文 UTF-8、
  会话续跑回放 tool 历史）。
- 会话日志、任务 DTO、前端全部保持 OpenAI 风格消息，协议转换只发生在 provider 层。
- stop_reason 映射：`end_turn/stop_sequence→stop`、`tool_use→tool_calls`、
  `max_tokens→length`（OutputLimit 错误）、`refusal→content_filter`。

## 三、已知坑（评测时注意）

1. **thinking token 计入 `max_tokens` 与 `output_tokens`**：网关自动开思考且无法关闭，
   `max_output_tokens` 必须给足（canonical 配置 16384）。过小会在思考阶段被截断
   （`stop_reason=max_tokens`），框架会报 OutputLimit 并终止任务。
2. **流式路径 `input_tokens` 恒为 0**：网关 SSE 的 `message_start.usage` 全 0，
   `message_delta` 只带 output_tokens → 流式任务统计到的 input tokens 不可信
   （非流式路径正常）。做 token 成本统计时建议以非流式或网关侧账单为准。
3. **workspace 目录必须预先存在**：运行时只 `canonicalize` 不创建 workspace；
   data_dir 会自动创建。不存在时启动直接报 `resolve workspace` 错误。
4. **同一 data_dir 不能同时被 CLI 与 serve 使用**（文件锁）。
5. **模型 id 必须是 `glm-5.3-flash`**：RoleRouter 默认模型绑定按此 id 匹配，
   改名会导致各任务类型回退自动路由。
6. **并行任务上限**：canonical 配置 `max_concurrent_tasks = 2`；网关为本地转发，
   高并发可能放大限流/超时，评测建议顺序执行。
7. 每轮模型调用耗时含思考，通常 10–40 秒；`max_turns=40` 的硬任务整体时长
   可到 20–40 分钟，轮询超时建议 ≥40 分钟。

## 四、变更清单（本次提交范围）

- `crates/samho-core/src/anthropic.rs`（新增）：协议转换层 + SSE 累积器 + 7 个单测。
- `crates/samho-core/src/model.rs`：`ProviderKind::Anthropic`。
- `crates/samho-core/src/provider.rs`：Anthropic 分发（惰性后端）、`ProviderError::network` 公有化。
- `crates/samho-core/src/agent.rs`：网络授权检查精确限定为 `ChatCompletions | Anthropic`
  （Demo 离线、Custom 走注入后端不查 URL——曾试过放宽为"非 Demo"，导致
  Custom 模型（无 base_url）全部任务报 provider URL missing、7 个集成测试失败
  2 个挂死，已修正）。
- `crates/samho-core/src/config.rs`：anthropic 模型同样要求 base_url。
- `crates/samho-cli/src/main.rs`：`config init` 生成默认接入本网关的模板。
- `crates/samho-core/src/lib.rs`：注册 anthropic 模块。
- `examples/samho.anthropic.toml`（新增）：canonical 评测配置。
- `docs/customization.md`、`docs/中文使用说明.md`：provider 说明与快速上手更新。
