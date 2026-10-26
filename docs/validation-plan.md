# Pi → Rust Agent 验收与性能验证计划

本文档基于 upstream Pi commit `2db5e359bf84c1c0be51d2c5c5c5c7cf27072c2b` 的静态审计。审计范围包括 `agent`、`ai`、`coding-agent`、`mcp`、`tui`、`durable`、`telemetry`、测试和基准脚本。本文档定义 Rust 重写的可复现实验、兼容性门槛和多场景验收方法。

本文的初始上游结论来自源码审计，未以真实模型或真实 MCP 服务验证；性能矩阵中的数值是验收门槛或测量方法。后续 Rust 实测记录见下一节，上游 Windows 测试尝试的原始结果另存于 `reports/pi-*.json` 与对应日志，不能把静态文件清单当作已通过测试。

## 本轮 Rust 增量实测记录（2026-10-22）

以下结果只代表当前 `rpi` 原型，不能替代上游 Pi 或真实供应商的兼容性报告。当前 `cargo test --workspace` 通过 93 项：CLI 16、core 单元 4、Agent 6、契约 15、Facet 6、MCP 5、MCP optional 4、OAuth 3、Provider streaming 11、runtime semantics 9、Skill policy 5、storage lifecycle 9。`cargo clippy --workspace --all-targets -- -D warnings` 与 `cargo fmt --all -- --check` 通过。

在 Windows PTY 上启动 `rpi serve` 后，`rpi tui` 连接同一个服务并提交翻译任务，任务完成且显示 `demo-fast` 与 `[DEMO simulation]` 标记；退出后终端恢复。该运行使用独立临时数据目录和回环地址，未发送付费模型请求。GUI 的静态检查确认脚本无 `innerHTML`/`eval`/内联事件，资源请求使用文本节点和同源 API。真实浏览器已完成自动路由、会话续接、reasoning tier、model pin、重启恢复与trace/Object检查；桌面和560×900窄屏均无水平溢出，末次10个启动请求全部200且console无error/warning。截图与报告已保存于 `reports/gui-validation/`；此fixture未配置实际MCP/Skill，浏览器慢任务取消竞态仍未验收。

本轮新增流式 Provider、上下文 lineage、重试/预算、会话 fork/steering/follow-up、SQLite journal、轨迹索引分页、Skill 权限、MCP optional/OAuth、Remote Facet 和 Web API 行为测试。所有本地 HTTP fixture 与 OAuth 回调只使用 300–400 范围（Rust 分配器实际使用 301–399）；真实模型、完整 MCP conformance、macOS/Linux 实跑、24 小时 soak 和等价 Pi/Rust 相对性能仍未验收。持久事件查询已经提供游标分页，但大规模长时间并发写入仍需独立 soak。

## 现有实现的可观测边界

Pi 已经是多包运行时，而非单一 CLI：

- `packages/agent/src/agent-loop.ts` 在每轮执行上下文变换、消息转换、模型请求、流事件合并和工具调用。`runLoop` 同时处理 steering/follow-up、继续回合、截断工具调用和终止；`executeToolCalls` 根据配置或工具属性选择顺序/并行执行（约第 161–320、508–620 行）。
- `packages/agent/src/types.ts` 暴露 `prepareRequest`、`prepareNextTurn`、`finishTurn`、`getApiKey` 等钩子，已经允许每个请求切换模型和 thinking level。
- `packages/coding-agent/src/core/virtual-models.ts` 定义了虚拟模型路由：路由原因有 `user`、`continuation`、`retry`、`direct`，输入包含上次成功模型、失败消息和持久化 state；`ModelRuntime.resolveModel` 验证物理模型、凭证并限制 thinking level。Rust 版本应扩展为可解释的“智力等级/预算/SLA/能力”策略，而不是删除这套能力。
- `packages/coding-agent/src/core/session-manager.ts` 使用 JSONL、树状 `parentId`、分支、compaction、context edit、model/thinking change 和 usage entry；会话上下文按叶路径投影。会话列表发现使用并发上限（`MAX_CONCURRENT_SESSION_DISCOVERY_LOADS = 64`，信息加载上限 10），并支持取消。
- `packages/mcp/src/protocol/types.ts` 接受 MCP `2025-11-25`、`2025-06-18`、`2025-03-26` 和旧版 `2024-11-05`；支持 stdio 和 streamable HTTP、OAuth、资源/模板、进度和取消。`packages/coding-agent/test/mcp-conformance/README.md` 的基线覆盖前三个协议版本；基线列出的失败包括 elicitation 默认值、CIMD client-id 检查、scope retry-limit，且 2026-07-28 stateless 不实现。配置代码已存在 `oauth.clientRegistration: "cimd"`，因此 README/基线不能代表最新实现已通过认证测试。旧式独立 SSE transport 被配置校验拒绝；这里的 SSE retry 指 streamable HTTP 内的 SSE。重写时须逐功能和版本报告支持情况。
- `packages/coding-agent/src/core/skills.ts` 递归发现 `SKILL.md`、读取 frontmatter、遵循 `.gitignore/.ignore/.fdignore`、校验名称（小写字母/数字/连字符，最长 64）和 description（最长 1024），处理来源优先级和冲突诊断。
- `packages/tui/src/tui.ts` 使用组件树、overlay、焦点管理、`requestRender` 合并和差分输出；`ScrollView` 管理滚动/跟随状态，Markdown 组件有按文本/宽度缓存。TUI 不应只以“能显示文字”作为兼容目标。
- `packages/telemetry/src` 提供类型化 span、事件、属性和内存 backend；`packages/coding-agent/src/extensions/mcp/log.ts` 将 MCP 日志同步追加到文件并在 5 MiB 后轮转；`crash-log.ts` 保留最近 5 条，崩溃提示筛选 7 天内记录。类型/适配器存在不等于已对全部核心调用自动埋点。Rust 版需把调用轨迹扩展到模型选择、MCP、skill、工具、存储和 UI。

### 上游测试和平台覆盖证据

静态清单找到 627 个 `.test/.spec` 文件（不包含 benchmark、helper、fixture）：coding-agent 312、ai 166、durable 49、tui 42、chord 21；其余 37 分布在 agent/client/codemode/env/evals/mcp/protocol/server/telemetry。文件数不等于测试用例数、执行成功数或覆盖率。

`test.sh` 隔离 HOME、临时目录、缓存和凭证环境；`test/suite/README.md` 明确采用 faux provider，禁止真实模型 API/付费 token。`.github/workflows/ci.yml` 主测试和 MCP conformance 都运行 Ubuntu，不能据此推断 Windows/macOS 已完整通过；`mcp/test/stdio.test.ts` 还明确跳过 Windows 的 stubborn server 子进程树清理测试。`scripts/check-browser-smoke.mjs` 检查 core 的 browser bundle 和选择性 provider 引入，不能代替真实浏览器/GUI 端到端测试。

## 热路径与瓶颈假设

以下是根据调用图得出的待验证假设，标记为“推断”，不能作为已测性能结论。

1. **模型流事件热路径（推断，高影响）**：`streamAssistantResponse` 对每个事件更新 `partialMessage`、替换上下文尾部并发出 `message_update`；`EventStream` 使用双数组 FIFO，出队为摊还常数时间，不能误称 `Array.shift()` 导致平方级开销。但它的 `push` 没有容量上限或异步 backpressure，慢消费者可能积压；这是可直接观察的设计边界，积压的实际内存影响尚未测量。测试 1 万/10 万/100 万事件、消费者延迟 0/1/10 ms、消费者中途退出。
2. **工具调度热路径（推断，高影响）**：`executeToolCallsParallel` 先逐个 await 参数校验/`beforeToolCall`，然后通过 `Promise.all` 执行准备完的调用并按原调用顺序写入 result；函数内没有通用 semaphore。因此慢 preflight 会推迟整个批次，大批次会同时起多个工具。必须分离 preflight 与执行阶段测量 1/4/16/64 工具，含一个慢工具和一个失败工具；Rust 增加可配置全局/会话/MCP server 并发预算，不能改变有依赖工具的顺序语义。
3. **上下文构建（推断，中高影响）**：每次请求可能执行 `transformContext`、`convertToLlm`、`normalizeContext`；长分支和 compaction 会触发树遍历、编辑投影与消息复制。应测 1k/10k/100k 消息、0/1/多次 compaction 和 8 层分支。
4. **会话 I/O（推断，中高影响）**：`SessionManager._persist` 使用 `appendFileSync`，首次 flush 和 `_rewriteFile` 逐行同步序列化写入；在同一事件循环下可阻塞 UI。会话列表已经用并发上限和流式读取，不能把所有 I/O 称为同步。SQLite/JSONL 重开、历史 delta 重放也要单独测。Rust 将 blocking 文件/数据库操作隔离，比较 buffered append、fsync 每次/按批次的延迟与断电恢复边界。
5. **TUI 重组/分配（推断，中影响）**：`TuiAltScreen.renderNow` 组合布局和差分输出；Markdown 在文本/宽度变化后重新 lex/render/wrap，ScrollView 的普通 `render` 路径仍调用 child.render。大 transcript、滚动、流式追加和 resize 的实际复杂度必须用现有大 transcript benchmark 与采样证明。
6. **MCP 协议边界（推断，外部 I/O 主导）**：stdio 子进程、HTTP 重连/OAuth、JSON-RPC 编解码和进度通知决定延迟；必须分别报告握手、listTools、callTool、取消、重连和认证刷新。

## 性能测量协议

所有 Rust 与 TypeScript/Pi 对比应使用同一机器、同一输入 fixture、同一并发度和相同网络替身。记录 OS/内核、CPU 型号与核心数、内存、Rust/Node 版本、编译 profile、commit、环境变量和时钟源。每个场景先预热，再做至少 20 个独立运行样本；操作/帧尾延迟至少累积 1,000 个样本并报告样本数，否则 p99 标为“样本不足”。报告 p50/p95/p99、平均、标准差、吞吐和峰值 RSS；单次结果不能作为结论。

固定 faux provider 输出和工具脚本建立正确性基线，归一化时间戳/随机 ID 后比较最终消息、usage、工具调用参数和会话投影；耗时和内存只对语义等价结果比较。真实模型质量评测另建固定任务集（检索摘要、单文件修复、跨文件重构、工具规划、长文推理），重复运行并盲评正确率、完成率、成本和时间；不同供应商响应不可作为协议回归的确定性断言。

必须区分：

- **端到端延迟**：从用户输入到最终 agent_end。
- **分段延迟**：context 构建、路由、认证、首 token（TTFT）、流完、工具排队、工具执行、持久化提交、TUI 帧。
- **外部等待**：模型/MCP 网络等待单独计时，避免把供应商波动归因于 Rust。
- **分配与 I/O**：Rust allocator 分配次数/字节、写入字节、fsync 次数、队列长度、事件丢弃数。

建议统一事件 schema：`trace_id`、`span_id`、`parent_span_id`、`session_id`、`turn_id`、`task_id`、`attempt`、`model_tier`、`provider`、`model`、`reason`、`input_tokens`、`output_tokens`、`duration_ms`、`status`、`error_class`。敏感字段（prompt、API key、工具参数中的 secret）默认哈希或截断，并提供采样率和禁用开关。

现有基准可复用为基线：

- `packages/tui/test/render-churn-bench.ts`：100×30 终端、150 个 Text transcript 组件（换行后不一定是 150 行）、预热 20 帧、测量 300 帧，报告总分配、KiB/帧、ms/帧和写入字节/帧；static 与每帧追加字符两种场景。
- `packages/tui/test/alt-screen-large-transcript-bench.ts`：80×50、2,500 Markdown 组件，测 steady、scroll、streaming、resize；预热 20 帧后分别测 200/100/10 帧。
- `packages/durable/test/storage.bench.ts`：memory/SQLite/JSONL；读基准 warmup 75 ms、time 300 ms、iterations 10；写入 warmup 5、iterations 20，重开 warmup 3、iterations 20；覆盖精确查找、100 条分页、过滤任务、文档 replay tail 0/16/128/1024、历史读取和 8 层 fork。
- `packages/durable/test/tool-output-bench.ts`：low/normal/high 输出速率、head/tail 保留策略、1/4 工具、3 秒和 30 秒长轮次、1 GiB firehose/cat，报告 commit p50/p99、replay、存储占用、RSS 和 GC 后堆。
- `scripts/profile-coding-agent-node.mjs`：TUI/RPC 启动基准，支持 warmup/runs、隔离 agent dir、CPU profile，输出 `METRIC startup_time_ms`；Rust 版应提供同名指标。

### Rust 版本的性能门槛（相对基线）

以下是待真实基线校准的初始目标，不构成速度承诺，也不是上游项目的既有 SLA。CI 首先锁定同等行为下不得回归，再据测量决定优化目标。不同硬件结果不能直接推导相对加速；分开报告并保存原始数据。

| 类别 | 目标 |
| --- | --- |
| Agent loop（faux provider、无网络） | p95 端到端延迟 ≤ Pi 基线的 0.7 倍；事件吞吐 ≥ 2 倍；无事件丢失或乱序 |
| 工具并行 | 4 个独立工具的墙钟时间接近最长单工具时间（并发开销 ≤ 20%）；顺序模式保持严格顺序 |
| 长上下文投影 | 固定每条平均 256 B 的 100k 消息构建 p95 ≤ 500 ms；内存峰值随消息数线性增长，不能出现平方级拐点 |
| 会话启动/重开 | 固定 10k 条同构数据，memory/SQLite/JSONL 分别报告 p50/p95；memory 无重开持久化承诺；Rust p95 目标 ≤ 同后端 Pi 基线的 0.7 倍 |
| TUI | 与对应 Pi fixture 同帧数下 p95 ms/帧 ≤ 0.7 倍；分配字节/帧 ≤ 0.5 倍；终端单元格语义一致，差分写入字节不得增加 |
| MCP | 热连接本地 callTool 协议栈 p95 ≤ 20 ms（不含服务处理）；冷子进程握手单独测，不适用该门槛；HTTP/OAuth 单独报告外部等待 |
| 稳定性 | 24 小时 soak 无泄漏趋势；取消、重试、重连后任务最终状态与轨迹一致 |

任何门槛失败都要附 flamegraph、trace 和最小复现 fixture；不能用提高超时或降低日志级别掩盖回归。

## 多场景、多任务验收矩阵

每格至少包含成功、取消、超时、错误重试和进程重启变体；任务结果、持久化状态和轨迹必须三者一致。

| 场景 | 任务组合 | 关键断言 | 采集指标 |
| --- | --- | --- | --- |
| 纯对话 | 短 prompt、长上下文、流式输出 | 首 token、最终文本、usage 和顺序兼容 | TTFT、tokens/s、上下文构建 |
| 工具链 | 1/4/16 工具；并行与顺序；工具失败后重试 | 参数校验、并发上限、取消传播、tool result 配对 | 排队/执行 p50/p99、并发峰值 |
| 长任务 | 30 分钟输出、1 GiB 输出、head/tail 截断 | 输出限额、replay、磁盘占用、RSS 不失控 | commit p50/p99、replay、RSS |
| 模型分级 | trivial→standard→reasoning→expert；按 token/延迟/成本约束路由 | 路由原因可解释；能力不满足时降级；失败重路由；state 可恢复 | 选中 tier、候选评分、路由耗时、降级次数 |
| 模型故障 | 429、5xx、超时、断流、上下文溢出 | 退避上限、重试次数、compaction 后重试、失败消息不重复计入上下文 | attempt、retry delay、error class |
| MCP stdio | 正常 server、慢 server、崩溃/忽略 shutdown、子进程树 | 握手版本协商、stderr、关闭时杀子进程、工具 schema | 启停时延、退出码、孤儿进程 |
| MCP HTTP/OAuth | 401、scope step-up、token 刷新、SSE/streamable HTTP 重连 | PKCE、回调限制、Last-Event-ID、重连退避、凭证不泄漏 | auth span、重连次数、事件序列 |
| MCP 资源 | list/read resources 与模板、分页、二进制内容 | URI/schema 保真、权限与超时 | payload 大小、解码错误 |
| Skill | user/project/path 来源、嵌套、同名冲突、非法 frontmatter、ignore 规则 | 发现顺序、诊断、禁用 model invocation、热重载 | 扫描时间、技能数、冲突数 |
| 会话树 | fork、branch、label、context edit、compaction、旧版本迁移 | 叶路径投影、模型/thinking 状态、重启后 ID/父子关系 | 投影耗时、JSONL 行数、迁移错误 |
| TUI | 150 行 transcript、2,500 Markdown、滚动、流式追加、resize、overlay/焦点 | ANSI/宽字符、差分输出、光标、焦点和终端恢复 | ms/帧、分配、写入字节、丢帧 |
| GUI/RPC | 多窗口/多会话、并发请求、断线重连、后台任务 | 与 TUI 共用事件模型；请求/响应 ID 唯一；状态可重放 | RPC p50/p95、队列深度、重连时间 |
| 崩溃恢复 | 任意 span 内强制终止进程，重启后恢复 | 内部 commit 去重；外部副作用以幂等 key 去重或显式标明需人工核对；崩溃日志关联 session | 恢复时间、重复副作用、未提交操作 |

## MCP、skill、模型路由的协议验收

1. **MCP**：逐协议版本运行官方 conformance；把 Pi `baseline.json` 作为差异清单，不自动豁免 Rust 的失败。当前本地 fixture 已覆盖 initialize、tools/list、tools/call、进度、取消、资源、OAuth PKCE、sampling、form elicitation、roots、tasks、completion 和 subscriptions；metadata discovery、动态注册、自动重连及官方 conformance 仍列为未完成。对外只宣称已通过 fixture 的明确能力。
2. **Skill**：fixture 中固定冲突优先级和诊断文本；同一目录多次扫描结果必须确定性排序。`disable-model-invocation` 必须阻止模型直接调用但仍允许显式用户触发（若产品策略如此定义）。
3. **模型分级**：为每次选择写入候选列表、能力缺口、评分项、预算、最终模型和 reason；同一输入与策略版本必须得到相同选择。路由器异常时使用声明的 fallback，并把异常作为 span event；不得把虚拟模型 ID发送到 provider。

“完整 MCP”不能仅指 tools/call：目标能力清单还需覆盖 prompts/list/get、completion、roots/list 与变化通知、resources subscribe/update、sampling、elicitation、协商版本允许的异步 task 生命周期，以及 logging level。已实现能力必须保留双向请求 fixture、取消/拒绝/权限场景；metadata discovery、动态注册、自动重连和官方 conformance 明确标为 planned。进度通知延长 request timeout 时仍需独立总 deadline，防止无限进度拖住任务。

## 故障恢复与一致性

- 所有异步操作接受取消信号；取消后停止派发新调用。已发往外部服务的副作用不能由取消信号撤回，必须标记执行状态和结果是否未知，已提交操作标记 committed；重启后禁止盲目重试非幂等工具。
- 重试采用指数退避 + 抖动 + 最大尝试次数；429/网络断开/5xx/上下文溢出分别计数。上下文溢出触发 compaction 后，重试请求不能重复旧 assistant 错误消息。
- durable 任务要验证进程重启、数据库损坏、半写 JSONL、schema migration、重复 submission。每次恢复生成 `recovery` span，记录 checkpoint 和 replay 范围。
- MCP server 崩溃必须关闭 stdin/stdout 读取、回收子进程树、解除等待中的 promise；HTTP 重连不能重复交付已确认事件。
- 日志和轨迹写满、权限不足或序列化失败时，核心任务仍可继续，但必须有计数器/health 状态；MCP 日志轮转和 crash log 保留上限应可配置。

## “万物皆 Object”与高度自定义验收

Object 化需要可验证的公共契约：每个 Agent/Task/Model/Tool/MCP server/Skill/Session/Artifact 具有稳定 ID、类型、schema 版本、能力描述和生命周期；任务链通过 ID 引用而不复制大型 payload。注册、查询、替换、销毁、snapshot/restore、未知类型和版本迁移均有契约测试。对 1/10k/100k Objects 测 registry 查询、枚举、事件传播和释放内存。

自定义验收包括：模型 tier 数量/评分权重/默认 reasoning、provider URL/headers、工具 timeout/并发、MCP 暴露 direct/deferred/codemode/hidden、skill 来源优先级、存储后端、日志 retention/redaction/exporter、TUI keybinding/theme、GUI 布局均可在配置中修改。测试配置优先级、错误诊断、热重载原子性、运行中任务继续使用旧策略 snapshot、新任务使用新策略，以及不合法配置回退。可扩展不是只允许任意 JSON；必须有 schema 校验和可解释的生效范围。

## TUI 与 GUI 验收细节

TUI 使用 pseudo-terminal fixture（Linux/macOS/Windows）；记录 ANSI 字节流并用终端模拟器重放截图/单元格矩阵。测试 CJK、组合字符、emoji、宽度变化、颜色、鼠标、SIGWINCH、Ctrl-C、overlay 焦点和退出后光标恢复。截图比较允许时间戳等动态字段白名单，其余像素/单元格差异必须为零。

GUI/RPC 应复用同一个事件协议：`agent_start/turn_start/message_update/tool_execution/turn_end/agent_end` 和 trace context 不能在 GUI 层重新推断。验收包括高延迟网络、乱序响应、重复响应、客户端重连、多个会话并发以及后台任务通知；界面关闭后任务仍可恢复，重新打开能从 durable 状态重建。

## 日志、调用轨迹与复盘验收

每次用户输入生成 root span；至少包含 `model_route`、`provider_request`、`llm_stream`、`tool_call`、`mcp_request`、`skill_load`、`session_projection`、`storage_commit`、`ui_render`、`retry`、`recovery` 子 span。单进程使用单调时钟和序列号，同时保留 wall clock；跨进程用 W3C traceparent 或等价字段传播，依父子关系/因果事件重建链路，不直接比较不同进程的单调时钟值。

复盘工具必须能按 session/turn/task 聚合：模型层级选择及候选、完整工具调用链、MCP 请求/响应摘要、token/cost、重试/取消、持久化 checkpoint、UI 帧异常。默认脱敏，提供 trace schema 版本和向后兼容迁移。验收用故障注入 fixture 逐项核对“发生了什么、为什么选择、何时重试、最终副作用”。

## 可移植性与发布门槛

目标平台为 Linux x86_64/aarch64、macOS arm64/x86_64、Windows x86_64；可选 WASI/浏览器仅承诺无 Node 专属功能的 core。CI 使用最小权限临时目录，禁止依赖 shell 特性、固定路径、系统时区或本地 locale。SQLite/JSONL、stdio 子进程、终端能力和文件锁需要平台适配测试。

发布候选必须满足：`cargo fmt --check`、`cargo clippy -- -D warnings`、单元/属性/协议/集成测试全绿；MCP conformance 与 skill fixture 无新增失败；上述性能门槛无回归；24 小时 soak、kill/restart 恢复和跨平台 TUI/RPC 烟测通过。所有失败保存原始 trace、日志、输入 fixture、二进制版本和硬件信息。

## 当前不确定项

以下结论目前只是推断，需在 Rust 原型和公平基准完成后更新：

- Rust 相对 TypeScript 的端到端加速比例；网络模型延迟可能掩盖运行时差异。
- `EventStream`/TUI 分配是否为真实瓶颈；必须用 CPU/heap profile 和每帧 trace 证明。
- SQLite、JSONL、Chord delta 在目标数据规模上的最佳后端；不能仅凭实现复杂度判断。
- 模型分级策略对质量、成本、延迟的实际 Pareto 前沿；需要固定任务集和盲评，而非只比较 token/s。
- GUI 框架和跨平台终端能力的最终选择；在 RPC 事件协议和故障语义冻结前不应绑定具体 toolkit。
