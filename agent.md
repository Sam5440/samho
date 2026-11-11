# agent.md — Samho 智能体工作框架完整参考

本文件是 Samho 仓库的权威工程参考，供人类开发者与 AI 编码代理（agent）使用。内容基于 2026-10-27 对全部源码、文档、脚本与 CI 配置的完整阅读。凡与旧文档冲突，以本文件与源码为准。

---

## 一、项目定位

Samho 是 **Pi coding agent（TypeScript）的 Rust 增量重写工程**，目标是把 Pi 的智能体执行语义（Agent 循环、流式、工具、会话树、MCP、技能）移植到 Rust，并加入自有设计：统一 Object 生命周期、六级模型路由策略、确定性上下文投影、预算预留、调用轨迹与脱敏。

- **语言/形态**：Rust workspace（两个 crate）+ 本地 Web GUI（中文）+ Ratatui TUI，单二进制 `samho`。
- **上游参考**（`upstream/`，gitignore，完整 clone，仅作分析不参与构建）：
  - Pi `earendil-works/pi` @ `2db5e359`（MIT）
  - DSH `deepseek-ai/deepseek-harness` @ `5badb150`（MIT）
  - ZCode `zai-org/ZCode` @ `29628c9a`（Apache-2.0）
- **默认 Provider**：明确标注 `[DEMO simulation]` 的离线模拟器；真实推理需配置 `chat_completions` 供应商。
- **诚实边界**：文档严禁把"静态清单/fixture 通过"写成"真实模型/跨平台/conformance 已验收"。写报告时必须区分"已实现并由本地 fixture 验证"与"未验收"。

## 二、仓库结构

```
D:\ddcode\samho\
├── Cargo.toml               # workspace：resolver=2，edition 2024，rust-version 1.88
├── crates/
│   ├── samho-core/            # 异步运行时库（无二进制）
│   │   └── src/{agent,model,provider,tools,session,events,context,budget,
│   │          object,skills,mcp,mcp/oauth.rs,facet,config,lib,test_support}.rs
│   │   └── tests/           # 12 个集成测试文件 + fixtures/（Python MCP fixture）
│   └── samho-cli/             # 包名 samho，二进制 samho
│       └── src/{main,server,tui}.rs
├── web/                     # 内嵌进二进制的中文 Web GUI（include_str!）
│   ├── index.html + app.js + style.css + chinese.css     # 工作台 /
│   ├── docs.html + docs.js + docs.css + documents-data.js # 文档中心 /docs（离线可用）
│   └── plan.html + plan.js + plan.css                     # 确认计划页 /plan（纯离线）
├── docs/                    # 中文文档（13 份），architecture/implementation-status 为状态权威
├── examples/samho.chat.toml   # 真实 Provider 配置示例
├── scripts/                 # 测试/基准/打包脚本（见 §13）
├── reports/                 # 验证证据（json/log/md/截图）；*.log 例外地入库
├── .github/workflows/ci.yml # 三平台 CI
├── agent.md                 # 本文件
└── upstream/                # 上游参考源码（不入库）
```

数据目录（默认 `.samho/`，可用 `data_dir` 配置）：`sessions.jsonl`（或 `sessions.sqlite`）、`events.jsonl`（或 `events.sqlite`）、`budget.jsonl`、`runtime.lock`（fs2 独占锁，单写者）、`ui.json`。

## 三、构建与运行

工具链：Rust ≥1.88（CI 用 current stable；本机 1.96.0）。MCP stdio 集成测试需 Python 3；中文网页契约测试需 Node 22。

```powershell
# 构建
cargo build --release --locked -p samho          # 产物 target/release/samho.exe（bundled SQLite）

# 运行（CLI 直接开本地数据目录；两个进程不能同时写同一 data_dir）
cargo run -p samho -- run "/demo read README.md" --complexity 2 --json
cargo run -p samho -- run "任务" --tier reasoning            # 内置六级
cargo run -p samho -- run "任务" --intelligence-tier architect # 自定义层级（须在 routing.tiers 定义）
cargo run -p samho -- run "任务" --model demo-fast --skill my-skill
cargo run -p samho -- inspect                                # 查看运行时状态（脱敏 JSON）
cargo run -p samho -- replay [trace_id] [--export trace.jsonl]
cargo run -p samho -- fork <session_id> --cutoff 3 --title "分支"
cargo run -p samho -- config init samho.toml                   # 已存在则报错

# 共享服务 + 两个客户端
cargo run -p samho -- serve --bind 0.0.0.0:7432            # 默认局域网可达；--bind 127.0.0.1:7432 可收回本机
cargo run -p samho -- tui --url http://127.0.0.1:7432        # TUI 客户端；浏览器打开 http://127.0.0.1:7432

# 配置
cargo run -p samho -- --config samho.toml serve
# 环境覆盖：SAMHO_HTTP_BIND、SAMHO_WORKSPACE、SAMHO_DATA_DIR（优先于 TOML）
```

验证命令（提交前必须全绿）：

```powershell
cargo fmt --all -- --check
cargo test --workspace --locked                            # 135 项（2026-10-31 基线）
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
node --test scripts/test-chinese-pages.mjs                 # 13 项中文网页契约
node --check web/app.js
python scripts/build-chinese-docs.py                       # 改 docs/*.md 后必须重跑，重新生成 web/documents-data.js
```

## 四、架构总览

```
Web(中文工作台) ─┐
TUI(Ratatui) ────┤→ Axum 本地服务(HTTP+SSE, 默认 0.0.0.0, 拒跨源) → AgentRuntime(并发许可)
CLI run/inspect/replay/fork ─┘                              │
   AgentRuntime → ModelRouter(六级策略/评分) → Provider(模拟器|Chat Completions)
              → ToolRegistry(内置工具+自定义+MCP+Skill 门控)
              → SessionStore(v2 增量日志, SHA-256 校验链) + EventLog(JSONL/SQLite, 单调序号)
              → BudgetTracker(预留/结算/重启恢复) + ObjectRegistry(拓扑激活/租约/热替换)
```

- `samho-core` 是全部业务逻辑；`samho-cli` 只是适配层（HTTP 服务、TUI、CLI 三个入口共享同一运行时）。
- Web 与 TUI 不各自维护 Agent 循环，全部经 HTTP API 与服务端共享状态。
- 服务安全：默认监听 `0.0.0.0`；`Origin` 必须等于 `http://{Host}`；`Host` 只接受 loopback、本机网卡 IP 或本机主机名（防 DNS rebinding）；无 CORS；响应带 CSP 与 `X-Content-Type-Options: nosniff`；请求体上限 1 MiB。没有身份认证，只应在可信局域网使用。

## 五、samho-core 模块参考

### 5.1 config.rs — RuntimeConfig（全部 TOML 键，`deny_unknown_fields`）

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `workspace` | `"."` | 工作区根，工具路径约束在其内 |
| `display_name` | `"Samho"` | 工作区显示名（页面顶栏），不依赖磁盘目录名 |
| `data_dir` | `".samho"` | 会话/事件/预算/锁存放处 |
| `http_bind` | `"0.0.0.0:7432"` | serve 地址（局域网可达；回环用 `127.0.0.1:7432`） |
| `system_prompt` | 见源码 | 会话 system 消息；自动追加可用技能清单 |
| `max_turns` | `20` | Agent 循环轮次上限 |
| `max_concurrent_tasks` | `4` | 全局并发任务信号量 |
| `request_timeout_ms` | `120000` | 单次模型请求超时 |
| `max_tool_output_bytes` | `65536` | 工具输出截断阈值 |
| `log_content` | `false` | false 时事件正文替换为 `"[content omitted; …]"`；会话恢复消息不受影响 |
| `durable_logs` | `true` | true = 每次追加 `sync_data`/`synchronous=FULL` |
| `skill_roots` | `[".agents/skills", ".pi/skills"]` | 相对路径基于 workspace |
| `models` | 6 个 demo 模型 | 至少 1 个 |
| `routing` | 见下 | 分类规则与评分 |
| `tools` | read=T, write=T, edit=T, list_files=T, glob=T, grep=T, webfetch=T, todo=T, shell=T, mcp=T, shell_timeout_ms=30000 | `ToolPolicy`（完整编码工具集默认全开，参照 OpenCode/ZCode/Pi/omp） |
| `mcp_servers` | `[]` | `[[mcp_servers]]` |
| `streaming` / `stream_buffer` | `true` / `32`（≤65536） | 流式开关与通道容量 |
| `context` | enabled=T, target_fraction=0.7, retain_recent_groups=4, summary_max_bytes=4096 | 投影/压缩 |
| `retry` | max_attempts=3, initial=100ms, max=5000ms, reroute=true | 指数退避+重路由 |
| `budget` | max_task_tokens=1_000_000，其余 None | 预算上限 |
| `storage.backend` | `"file"` | 或 `"sqlite"` |
| `logs` | rotation_bytes=64MiB, max_archives=8 | 事件日志轮转 |
| `ui` | theme=dark, submit=Ctrl+Enter, new=Alt+N, cancel=Ctrl+K | TUI 快捷键 |
| `workspaces` | `[]` | `[[workspaces]]` id/name/path/data_dir（id 不得为 default） |
| `network` | allowed/denied_hosts=[], allow_loopback=T, allow_private=F, allow_http_loopback=T | URL 准入策略（DNS 解析后逐地址检查） |
| `remote_facets` | `[]` | `[[remote_facets]]` |

环境变量名规则：秘密值只能放环境变量；`Redactor::from_env` 会把名字含 `api_key/token/secret/password`（大小写不敏感）且值长度 ≥8 的环境变量值纳入脱敏。

### 5.2 model.rs — 模型描述与路由

`[[models]]` 键：`id`、`model`（物理模型 ID）、`provider`（`demo`|`chat_completions`|`custom`）、`tier`（六选一）、`base_url`、`api_key_env`、`context_window`（128000）、`max_output_tokens`（4096，必须 ≤ context_window）、`supports_tools`（T）、`supports_vision`（F）、`supports_reasoning`（F）、`reasoning_effort`、`streaming`（F）、`intelligence_tier`、`output_token_parameter`（`max_tokens`|`max_completion_tokens`）、`input_usd_per_million`、`output_usd_per_million`、`latency_ms`（50）、`specialties`、`extra_body`（不能覆盖运行时保留键）。

六级策略：`nano(0-1)` `fast(2-3)` `balanced(4-5)` `reasoning(6-7,requires_reasoning)` `frontier(8-10)` `specialist(255=仅手动)`。可用 `[routing.tiers]` 自定义（必须覆盖 0..10 且不重叠）。

路由算法（确定性，全部写入 `model.route` 事件）：
1. 自动分类（`profile.automatic` 且未指定 tier/model 时）：按 `[[routing.rules]]` 关键词子串匹配（默认：架构/证明/重写→9，调试/重构→7，翻译/摘要→2；其余 ≤32 字符→1，否则→4）。**注意：automatic=true 时 `profile.complexity` 输入被忽略**（自动分级只看关键词与长度）；需精确控制层级用 `automatic=false` + `complexity`/`tier`/`model`。
2. 复杂度→期望层级；候选过滤（工具/视觉/推理能力、上下文容量、预算、延迟、specialty、重试排除表），拒绝理由记录在 `rejected`。
3. 评分 `100 - tier_penalty×层级差 - latency_ms×0.001 - estimated_usd×5`，按分排序，同分按 id 字典序。
4. `TaskProfile` 支持手动 `tier`/`model`/`intelligence_tier`/`context_tokens`/`requires_*`/`budget_usd`/`max_latency_ms`/`specialty`。

### 5.3 agent.rs — AgentRuntime

- **启动**：验证配置 → 规范化路径 → data_dir 文件锁（冲突报 `"data directory is already in use…"`）→ 打开会话/事件存储 → 网络策略 → 技能发现 → MCP 连接 → 工具注册 → 模型路由 → 预算账本 → Provider → 注册全部 Object（runtime、model:*、skill:*、tool:*、mcp:*、session:*、facet:*）→ 拓扑激活。
- **任务流**（`run_with_task_id`）：空 prompt 拒绝 → 信号量排队（可取消）→ 建会话（每会话同时只允许 1 个活动任务）→ `task.start` → 崩溃恢复（给未配对工具调用补 `"Interrupted previous tool call. Its effect is unknown…"` 观察，不重放外部副作用）→ 显式技能加载 → 每轮：取消检查 → 应用 steering → 轮界刷新工具 schema → 上下文投影 → 预算预留（worst-case，失败不退款）→ `model.start` → 流式/非流式调用 → 结算 → 工具批次（严格顺序执行，逐个校验 id/名称/参数 JSON）→ 检查点 → 无工具调用则进入控制队列结算（steering 随时应用；terminal 时最多应用一条 follow_up 后继续循环）→ 终态 `task.done|cancelled|failed`（span `terminal`）。
- **取消**：贯穿排队/请求/退避/工具执行；取消后报错明确提示外部副作用可能未知。
- **错误重试**：`retryable()` = Timeout|Network|RateLimited|Server；退避 `retry_after_ms` 或 `initial << (attempt-1)` 封顶；`retry.reroute=true` 时把失败模型加入排除表重新路由，发 `model.reroute`。
- **控制队列**：`control(task_id, prompt, follow_up)`；prompt 1B–1MiB；steering+follow_up 合计 ≤128。
- **API**：`register_tool`（自定义 Rust 工具，须 `tool:<name>` ID + Tool kind + JSON Schema spec）、`fork`、`subscribe_progress`（广播容量 256）、`progress(task_id)`、`state()`、`sessions()/session_summaries()/session(id)`、`shutdown()`。

### 5.4 provider.rs — 模型服务

- `ProviderErrorKind`：Cancelled/Timeout/Network/RateLimited/Server/Authentication/Protocol/Configuration/ResponseLimit/OutputLimit/ConsumerClosed。
- **Chat Completions**：URL 校验（http/https、无 userinfo/fragment、自动补 `/chat/completions`）；流式要求 `text/event-stream` + `[DONE]` 结尾（缺则 `"SSE disconnected before a complete [DONE] frame"`）；限制 16 MiB 响应/256 KiB 帧/128 工具调用；usage 来自 `usage.prompt_tokens/completion_tokens`；`finish_reason` 只接受 stop/tool_calls（length→OutputLimit）；SSE 工具索引必须连续；推理增量同时接受 `reasoning_content`（DeepSeek 风格）与 `reasoning`（GLM/OpenRouter 风格），`finish_reason` 后的空尾帧/usage 尾帧容忍、携带新负载仍拒绝。注意：GLM 类思考模型 `max_tokens` 过小时 token 被思考消耗导致 content 为空——给真实模型配置 ≥512 的 `max_output_tokens`。
- **Demo 模拟器**：`/demo read {path}`、`/demo write {path}\n{content}`、`/demo skill {name}` 触发真实工具调用循环；普通 prompt 返回 `【模拟模式】[DEMO simulation / {model.id}]…`；工具观察后回复 `【模拟模式；以下为真实工具结果】…`；流式按 32 字符安全分块。
- 注入扩展：`RuntimeBuilder.provider(...)` 可替换为任意 `ModelProvider` 实现。

### 5.5 tools.rs — 内置工具与策略

| 工具 | 开关 | 输入 | 行为 |
| --- | --- | --- | --- |
| `read_file` | tools.read | `{path, offset?, limit?≤2000}` | 仅 workspace 相对路径，禁止绝对/父目录/symlink；可选 1 起始行窗口（默认整文件），返回 `total_lines/start_line/end_line/truncated_lines` |
| `write_file` | tools.write | `{path, content}` | 建父目录后写入 |
| `edit_file` | tools.edit | `{path, old_string, new_string, replace_all?}` | 精确字符串替换；不匹配/多处匹配（未开 replace_all）报错；不创建文件；上限 16 MiB；展示用调用方相对路径 |
| `list_files` | tools.list_files | `{path?, depth?≤16, max_entries?≤2000}` | workspace 内目录枚举（默认根、深 4、500 条，目录优先按名排序）；跳过 `.git`/`node_modules`/`target`/symlink；`truncated` 标记截断 |
| `glob` | tools.glob | `{pattern, path?, max_entries?≤2000}` | 文件模式匹配：段内 `*`/`?`，`**` 跨任意层目录；不含 `/` 的模式按文件名任意深度匹配；大小写敏感；只返回文件；同款跳过清单 |
| `grep` | tools.grep | `{pattern, path?, glob?(*?通配文件名), ignore_case?, max_results?≤1000}` | workspace 内正则内容搜索（默认根，返回 `{file,line,text}`，行文本截 240 字符）；跳过二进制（NUL 探测）、>1MiB 文件、`.git`/`node_modules`/`target`/symlink；计数器如实回报 |
| `webfetch` | tools.webfetch | `{url, max_bytes?≤1MiB(默认128KiB)}` | 经 `NetworkPolicy` 逐跳校验（含重定向，≤5 跳，禁自动重定向）；30s 总超时；HTML 剥离 script/style/标签解码常见实体为纯文本（非完整 Markdown 转换）；二进制 MIME 拒绝；流式读取硬顶截断 |
| `todo_write` / `todo_read` | tools.todo | `{items:[{task,status:pending\|in_progress\|completed,note?}]}` / `{}` | 会话级任务清单（`data_dir/todos/{session}.json`，temp+rename 原子发布，≤200 项）；write 全量替换并回读，read 返回 `{items,exists}` |
| `load_skill` | 恒开 | `{name}` | 模型侧技能加载；`disable-model-invocation` 技能拒绝 |
| `shell` | tools.shell | `{command}` | Windows powershell -NoProfile（CREATE_NO_WINDOW）/其他 sh -c；超时/取消杀死子进程 |
| `mcp__{server}__{tool}` | tools.mcp | 按 schema | 路由到 MCP |

调用顺序：取消检查 → 活动技能 `allowed-tools` 门控 → 策略授权 → schema 校验（禁外部 `$ref`）→ 经 ObjectRegistry 租约调用 → 输出 schema 复验。`list_files`/`glob`/`grep` 的目录遍历在 `spawn_blocking` 中执行并响应取消；`webfetch` 复用运行时 `NetworkPolicy`（`RuntimeBuilder.network_policy` 定制同样生效）。工具 schema 全量计入上下文投影与预算 worst-case 预留（ConservativeTokenCounter 按序列化 JSON 字节计），默认工具集扩大会线性抬高每次派发的预留——预算/投影校准类测试需固定自己的工具集（`pin_minimal_tools`）。

### 5.6 session.rs — 会话存储

- Schema v2 增量日志：每条记录 `Delta{schema_version, sequence, previous_checksum, metadata, truncate_to, append_messages, checksum(SHA-256)}`；校验链断裂即拒绝打开。旧版全量快照自动迁移（保留 `.legacy.jsonl`）。
- 状态机：`ready → running → done|cancelled|failed`；重启时 `running` 改 `interrupted`。
- `fork(parent, cutoff, title)`：cutoff 为消息条数（≥1 保留 system），不得分割未完成工具调用组，父会话 running 时禁止。
- 后端：file（JSONL，原子追加/回滚/临时文件改名发布）或 sqlite（WAL+FULL）。

### 5.7 events.rs — 事件与轨迹

- `TraceEvent{sequence, timestamp_ms, trace_id, session_id, span_id, parent_span_id, kind, object_id, payload}`；序号单调（1 起），持久化前脱敏。
- 运行中事件种类：`task.start, model.route, context.project, model.start, model.response, model.error, model.reroute, model.retry, tool.start, tool.result, skill.load, task.control.queued, task.control.applied, task.done|cancelled|failed`。span 命名：root=task_id，子 span `{task_id}:route|model:{turn}|tool:{call_id}|terminal…`。
- 查询：`after_sequence` 独占游标，limit 1..=10000，可按 trace/session/kind 过滤；`EventPage` 带 `next_cursor/has_more/first_available_sequence`（检测 retention gap）；`timeline(trace_id)` 聚合 span。
- 轮转：文件模式支持 `.segment-{id:020}` 归档；自定义 backend 必须关闭轮转。
- 广播：tokio broadcast 容量 256，落后即 `Lagged`（SSE 报 gap 后重建游标追平）。

### 5.8 context.rs — 上下文投影

- 计数器 `ConservativeTokenCounter`（UTF-8 字节数，刻意保守）。投影仅作用于模型输入，不改会话记录。
- 算法：system 全保留 → 工具批次与其观察为不可拆分组 → 保护最后用户消息所在组 → 摘要头部含源索引与 SHA-256 → 多级摘要预算（4096/512/256B）逐步尝试 → 全部失败才报错（明确列出原因）。
- lineage：`{schema_version:1, source_indices, source_sha256, summary_sha256, before/after tokens, counter}`，写入 `context.project` 事件。

### 5.9 budget.rs — 预算

- `budget.jsonl` 每行全量快照；重启恢复。每次派发前按 worst-case 预留（输入 tokens + `budget_output_reserve()` + 对应美元）；失败调用不退款；实际 usage 超预留 → 记账真实用量并发 `budget.overage` 事件（payload：turn/attempt/model_id/reserved_usd/actual/note）后继续任务，**不再中止派发**——硬顶（`max_task_*`/`max_total_*`/`profile.budget_usd`）仍在派发前拒绝。
- 思考预留：思考模型（`supports_reasoning`）的网关不把 `max_tokens` 应用于思考词元，`ModelDescriptor.reasoning_reserve_tokens`（缺省 `max_output_tokens ×2`）计入预留，防「任务在成功边缘被超预留终止」。
- 上限：`max_task_usd/max_total_usd/max_task_tokens/max_total_tokens` + 任务级 `profile.budget_usd`。

### 5.10 object.rs — Object 注册表

- `ObjectKind`：Agent/Model/Tool/Skill/McpServer/Session/Task/Plugin/Custom。
- `ObjectEnvelope{id, kind, version, description, dependencies, capabilities, labels, spec}`；`Object` trait：`envelope/invoke/activate/dispose`。
- 生命周期：拓扑激活（缺依赖/环拒绝；失败回滚本次新增并逆序 dispose）→ `acquire` 租约（in_flight 计数）→ `hot_replace`（同 id 同 kind、依赖不变、无 dependents、排空超时回滚）→ `dispose_scope` 逆序排空。middleware before/after（after 逆序，出错把 Ok 变 Err）。

### 5.11 skills.rs — 技能

- `SKILL.md` YAML frontmatter：`name`（1..64，`[a-z0-9-]`，无首尾-与--）、`description`（≤1024）、`disable-model-invocation`、`allowed-tools`（空白分隔字符串或数组，模式 `*?` 通配，`tool:` 前缀可省）、`metadata`。文件 >1MiB 报错；路径任何层级 symlink 拒绝。
- 发现：递归 ≤32 层，跳过 `.git/node_modules/target`，支持 `.skillignore`；重名首个胜出并记诊断；非法文件仅诊断不致命。`refresh()` 全量原子重发现（generation 递增）。
- 加载：显式（用户/任务）绕过 disable-model-invocation；模型经 `load_skill` 工具受其约束。活动技能的 allowed-tools 对所有后续工具生效（`"skill {name} denied tool {tool} by allowed-tools"`）。

### 5.12 mcp.rs — MCP 客户端（rmcp 3.5.1）

`[[mcp_servers]]`：`name`（1..32，`[A-Za-z0-9_-]`，禁 `__`）、`command`+`args` 或 `url`（二选一）、`env`、`bearer_token_env`、`enabled`、`timeout_ms`(60000)、`startup_timeout_ms`(30000)、`cwd`、`scrub_env`(T，剔除继承环境中含 KEY/PASSWORD/SECRET/TOKEN 的变量)、`max_discovery_pages`(100)、`client{roots, enable_sampling, enable_elicitation, enable_tasks, max_sampling_tokens}`。

- 已实现：initialize、tools/list（分页+代际替换）、tools/call（isError→ToolFailure，任务轮询 SEP-2663）、resources/list|templates|read|subscribe、prompts/list|get、completion、ping、roots、sampling/elicitation（须启用+宿主 hook）、通知队列、OAuth PKCE（端口 300–400 策略）。
- 明确未实现（`status()` 如实列出）：OAuth metadata discovery/动态注册、已连 HTTP 自动刷新、URL elicitation、modern subscriptions/listen、server/discover、非 task MRTR、stdio 自动重连、外部 `$ref`、官方 conformance。
- 状态：`connected|degraded|failed|disabled|closed`；工具命名 `mcp__{server}__{normalized}`（≤64 字符）。

### 5.13 facet.rs — 远程 Facet

`samho-facet/1` 协议：HTTPS（回环可 `allow_insecure_loopback`）+ Bearer（`auth_token_env`）+ 能力白名单（必填）；`/handshake /invoke /cancel`；traceparent 透传；取消 best-effort ≤250ms；dispose 后调用返回 Disconnected。这是受控远程对象适配器，**不是**任意远程执行。

## 六、HTTP API（samho-cli/src/server.rs）

静态：`/`、`/docs`、`/plan` 及对应 css/js（`include_str!` 内嵌）。

| 方法 路径 | 说明 |
| --- | --- |
| GET `/api/health` | `{"status":"ok","api_version":1}` |
| GET `/api/state` | 运行时全量状态（脱敏，含 simulation/tiers/mcp/workspace/ui） |
| GET `/api/models` `/api/objects` `/api/skills` | 列表（全部脱敏） |
| POST `/api/skills/refresh` | 重新发现技能 |
| GET `/api/sessions`；GET `/api/sessions/{id}`；POST `/api/sessions/{id}/fork` | 会话列表/详情/分支 `{cutoff?, title?}`（deny_unknown_fields） |
| POST `/api/run` | `TaskRequest` → 202 `{task_id, status:"running"}`；空 prompt 400；并发满 429；关停中 503 |
| GET `/api/tasks/{id}` | 快照 `{task_id,status,result,error,progress,error_details}`；完成条目重启后 404 |
| POST `/api/tasks/{id}/cancel` `/steer` `/follow-up` | 取消/引导/追加（`{prompt}`） |
| GET `/api/events` | 裸数组；query：trace_id, session_id, kind, after, limit(≤5000) |
| GET `/api/events/page` | 分页封装 `{events, has_more, next_cursor, first_available_sequence}` |
| GET `/api/events/stream` | SSE `event: trace`（`id:`=序号）+ `gap` + keepalive 15s；支持 `Last-Event-ID` |
| GET `/api/live/stream` | SSE `event: progress`（累计快照，revision 单调）+ `gap` |
| GET `/api/traces/{id}/timeline` `/export` | 展示式复盘（不重执行）；导出 Markdown（固定文件名 Samho-trace.md，>16MiB 413） |
| GET/PUT `/api/ui` | 主题 dark/light/system + 三个快捷键；非法 400 |
| GET `/api/workspaces` | default + 配置多工作区；`/w/{id}` 挂载**独立运行时**（独立 data_dir/会话/预算），`/w/{id}/` 尾斜杠由 fallback 308 重定向（axum 0.8 nest 不匹配尾斜杠且路由先于中间件） |

## 七、TUI（samho-cli/src/tui.rs）

纯 HTTP 客户端：`/api/health` 连通性检查 → 800ms 轮询 8 个资源端点 → 提交后 250ms 轮询任务。

- 键位（可经 `/api/ui` 改）：Ctrl+Enter 提交（默认时裸 Enter 亦可）、Alt+N 新会话、Ctrl+K 取消（空闲时退出）、Ctrl+Q 退出、F2 层级、F3 模型、F4 会话、F5 新草稿、F6 技能、F7 工作区、Tab 检查器面板、PgUp/PgDn 滚动、Shift+Enter 换行。
- 斜杠命令：`/help`、`/steer <内容>`、`/follow-up <内容>`、`/fork [截止数] [标题]`、`/theme dark|light|contrast`（注意：服务端只接受 dark/light/system，`contrast` 会被 400 拒绝——已知不一致）、`/bind submit|new|cancel <键>`。
- 渲染：三栏（会话 22% / 轨迹 51% / 检查器 27%），CJK/组合字符/emoji 安全（TestBackend 覆盖 12x6..160x48）。

## 八、Web GUI

- **工作台 `/`**：任务提交（自动分级/复杂度滑条/层级/模型/技能勾选）、会话侧栏、steer/follow-up 模式选择、实时进度（SSE）、轨迹面板（分页+SSE+按 trace 过滤+导出）、对象/MCP/技能面板、UI 设置、多工作区切换。
- **文档中心 `/docs`**：13 份中文文档，搜索/分类/hash 路由/.md 下载（BOM）。数据源 `web/documents-data.js` 由 `python scripts/build-chinese-docs.py` 生成——改 docs 后必须重跑。
- **确认计划 `/plan`**：纯离线（不发任何 API 请求），47 个范围项（已完成/部分完成/未完成），依赖检查告警，导出 `samho-确认执行计划-YYYY-MM-DD.txt`（UTF-8 BOM）。勾选只是范围选择，回传完整文本才执行。

## 九、测试与端口策略

- 135 项 Rust 测试（CLI 38、core 单元 19、agent 6、contracts 15、facet 6、MCP 5、MCP optional 4、OAuth 3、provider streaming 15、runtime semantics 10、skill policy 5、storage lifecycle 9）+ 13 项 Node 网页契约。
- **测试端口策略**：所有测试/fixture/OAuth 回调/浏览器/smoke 只用 **300–400**（Rust `test_support::bind_loopback()` 实际 301–399）；生产端口不限。
- 集成测试全部使用本地 fixture（axum/Python stdio MCP/faux provider），零真实模型调用、零付费。
- 已知未验收：真实供应商质量、官方 MCP conformance、macOS/Linux 实机、24h soak、同机 Pi/Rust 性能对照。

## 十、CI（.github/workflows/ci.yml）

三平台矩阵（ubuntu/windows/macos），步骤：fmt check → `cargo test --workspace --locked` → clippy `-D warnings` → `node --test scripts/test-chinese-pages.mjs` → release 构建 → `python scripts/smoke_runtime.py` → 上传 `reports/ci-smoke.json`。

## 十一、编码规范（agent 必须遵守）

1. 所有配置/请求结构 `#[serde(deny_unknown_fields)]` + `#[serde(default)]`；新增配置键必须给默认值并通过 `validate()` 校验（错误信息具体到字段）。
2. 一律 `cargo fmt`；clippy 零警告（`-D warnings`）。
3. 秘密只能经环境变量；新代码路径凡接触 payload/错误信息必须过 `Redactor`（持久化、广播、progress、错误返回四条路径都要）。
4. 取消语义：所有长操作 `select!` 取消分支；副作用已发出时错误信息必须含"外部副作用未知/需人工核对"。
5. 错误要具体（含字段名/原因），禁止静默回退（尤其 provider 失败不得回退到 demo）。
6. 测试端口 300–400；新功能先补 fixture 测试再宣称完成；"已实现"必须有对应测试证据。
7. 改 `docs/*.md` 后跑 `python scripts/build-chinese-docs.py`；改 `web/*.js` 后跑 `node --check` 与 `node --test scripts/test-chinese-pages.mjs`。
8. 数据文件写入模式：临时文件 + 改名发布（JSONL 校验链/SQLite 事务），崩溃尾部只在全量校验后截断。
9. 中文用户可见文案；事件/payload/日志键名保持英文。
10. 提交信息风格见 git log：`类型: 中文摘要`（feat/fix/docs/test/ci/chore）。

## 十二、已知不一致与边界（本轮实测确认）

1. TUI `/theme contrast` 与服务端 `validate_ui`（dark/light/system）词汇不一致 → 400。
2. TUI 进度视图工具计数行存在字面 `format!("?????{}", n)` 疑似乱码图标。
3. 完成任务索引仅内存（1024 上限、重启清空），`GET /api/tasks/{id}` 404 属预期。
4. trace 导出文件名固定 `Samho-trace.md`。
5. GLM 思考模型在 `max_tokens` 过小时 content 为空（网关侧行为），配置需 ≥512。
