# Pi（commit `2db5e359bf84c1c0be51d2c5c5c5c7cf27072c2b`）架构分析

## 范围、版本与证据方法

本文针对 `upstream/pi`（远端 `earendil-works/pi`，工作树 `main`，提交时间 2026-10-07）做静态代码与文档审计。分析覆盖 `ai`、`agent`、`durable`、`chord`、`telemetry`、`coding-agent`、`mcp`、`codemode`、`tui`、`protocol`、`client` 和 `server`。完整读取了 Agent/loop、MCP client、virtual-model、skill loader、telemetry reference adapter、Chord public API 等关键文件；大型 `AgentSession`/TUI/provider 实现按关键路径定位，未声称逐行审完仓库全部代码。引用路径均以 `upstream/pi` 为根（分析期本地 clone 已清理，commit 见 `docs/upstream-lock.json`）。检查时上游没有 `node_modules`，本报告没有把构建或模型请求结果冒充实测结果。

静态规模（PowerShell `Get-ChildItem src -Recurse -File` / `Get-Content | Measure-Object -Line` 统计口径，含源码目录中的资源）：`ai` 197 文件/24,339 行，`coding-agent` 311/84,543，`chord` 30/8,284，`durable` 67/18,965，`tui` 45/17,146，`mcp` 18/2,907，`agent` 6/2,288，`telemetry` 6/826。根 `package.json` 要求 Node `>=22.19.0`，构建顺序为 chord → tui → telemetry → codemode → mcp → ai → durable → env → agent → protocol → client → server → coding-agent。

“静态”表示从源码、类型、README、测试与基准文件推导；“实测”只表示本文实际运行的命令。本文实际只运行了仓库状态/文件统计、`git` 元数据和 Node/npm 版本检查，没有安装依赖、没有调用真实模型、没有运行 `npm test`。

## 分层总览

```text
providers/API/auth/catalog (pi-ai)
             |
      Agent + event loop (pi-agent-core)
             |
 SessionManager/AgentSession/ResourceLoader (coding-agent)
       /             |                    \
 JSONL session    MCP + skills + extensions   TUI/CLI/JSON/RPC/SDK

 durable Harness/Session/Docs/Tasks  ->  Chord JSON delta + replicated state
 Chord facets/services              ->  composition + reload + remote binding
 protocol/client/server  ->  opaque routed Chord service envelopes
 telemetry          ->  explicit span context (adapter supplied)
 codemode           ->  QuickJS/WASM capability sandbox
```

`chord` 是独立通用运行时，不依赖其它 Pi workspace 包。`durable` 在 Chord 之上提供持久会话与任务。普通 `coding-agent` 仍主要使用 `SessionManager` 的 JSONL 树；实验性 durable CLI 是另一条运行时路径，不能把两者的恢复语义混为一谈。

## `pi-ai`：模型、协议适配与身份验证

证据：`packages/ai/src/types.ts`、`models.ts`、`models-store.ts`、`providers/*.ts`、`api/*.ts`、`auth/*`、`README.md`。

- `Provider` 是拥有 `id/name/auth`、模型目录、刷新、过滤、chat streaming、可选 image/classifier/deferred 操作的运行时单元。`Models` 负责 provider 注册、凭据解析、可用性过滤、刷新竞态代数、请求委派和错误归一化；provider 才拥有 HTTP/API 细节。
- 模型已按能力拆成 `chat`、`image`、`classifier` 三种。chat `Model` 含 `reasoning`、`thinkingLevelMap`、`contextWindow`、`maxTokens`、价格分层、输入图像限制、prompt-cache 生命周期和 API compatibility flags；image 只接受 `generateImages`；classifier 对 JSON 状态回答选择/布尔/评分问题。
- 统一事件流是 `AssistantMessageEventStream`。`api/*` 将 Anthropic、OpenAI Responses/Completions、Bedrock、Google、Mistral、Cloudflare、OpenRouter 等不同协议归一化为 text/thinking/tool-call/done/error 事件，并处理工具 schema、推理预算、缓存和重试。`durationMs` 从单调时钟计算。
- auth 支持 API key、环境变量/命令插值、OAuth 凭据存储、刷新和 provider 自定义 login；请求层明确区分“未知/未配置 auth”“OAuth 刷新失败”“请求失败”。动态模型刷新使用 `publish` 和 generation check，失败保留旧目录。
- thinking 目前是 `off/minimal/low/medium/high/xhigh/max` 七级，模型通过 `thinkingLevelMap` 映射到 provider 具体 effort/budget；`thinkingBudgets` 只给 minimal～high 自定义 token 上限。它是推理强度/预算，不是完整任务等级策略。
- Pi 对兼容模型暴露大量可调字段（采样、缓存 retention、session-affinity、vLLM priority、strict schema、mid-conversation tool changes 等），扩展可 `registerProvider`、提供自定义 stream/images/classifiers/OAuth 和动态目录。

### 关键限制与迁移启示

`pi-ai` 提供能力与 provider 路由，但没有任务分类器、预算守卫、全局优先级、并发配额或租户隔离的统一策略。可作为 Rust 的底层 provider trait；任务等级需要独立的 policy/router object，而不应把 `thinkingLevel` 误当作全部分级。

## `pi-agent-core`：Agent 状态机与工具调用

证据：`packages/agent/src/agent.ts`、`agent-loop.ts`、`types.ts`、`stream-fn.ts`、`README.md`。

一次 `prompt` 的核心轨迹：`agent_start` → `turn_start` → 输入 `message_start/end` → provider 流式 assistant `message_start/update/end` →（有工具时）`tool_execution_start/update/end` 与 `toolResult message_start/end` → `turn_end`；队列或工具结果需要时回到下一 `turn_start`，否则 `agent_end`。`Agent` 等待异步 listener（包括 `agent_end`）后才转 idle。

每次 provider request 的准备顺序是 pending input → `prepareRequest` → `transformContext` → `convertToLlm` → API key 解析 → `streamFn`。`AgentMessage` 可通过 declaration merging 扩展，LLM 边界只接受 system/user/assistant/toolResult；因此 UI/业务对象可以留在 transcript 而不进入模型。

工具参数先 `prepareArguments`（兼容修复）再 TypeBox schema 校验，然后 `beforeToolCall` 可阻断，执行中 `onUpdate` 发增量，最后 `afterToolCall` 可逐字段覆盖 content/details/isError/usage/terminate。未知工具、校验失败、异常、abort 都转成模型可见的错误结果，低层循环原则上不因工具失败 reject。

多工具默认并行：preflight/校验按 assistant 源顺序，允许的执行并发，`tool_execution_end` 按完成顺序；持久化的 tool-result message 仍按源顺序。任一工具声明 `executionMode: "sequential"` 会令整个批次串行。只有批次中所有 finalized result 都 `terminate: true` 才跳过自动 follow-up。

steering 队列在工具轮结束后注入，follow-up 队列在自然停止后注入；两者都支持 `all` 或 `one-at-a-time`。`finishTurn`、`prepareNextTurn`、`prepareRequest`、`beforeToolCall`、`afterToolCall` 和 context transform 是主要扩展点。`Agent` 只保存当前内存 transcript；持久性由上层 SessionManager/Harness 完成。

### 可靠性/性能评价（静态）

- 优点：工具参数在执行前验证；截断的 `length` assistant 中的所有工具调用被拒绝执行；abort signal 贯穿 provider、工具和 listener；事件顺序及并发语义写入 README。
- 风险：并行批次使用 `Promise.all`，单个工具的高内存/高 CPU 会占用整个进程；没有通用的每工具 deadline、资源配额或 sandbox。`AgentTool` 的类型仍留有 `any` 兼容泛型，Rust API 应用严格 `serde` 对象和能力 token。
- 轨迹可观察到事件和 `durationMs`，但默认没有 trace/span ID、父子关系、请求 attempt ID 或持久化调用树。

## `coding-agent`：产品会话、资源、MCP 与模型分级

证据：`packages/coding-agent/src/core/{agent-session.ts,session-manager.ts,model-runtime.ts,resource-loader.ts,skills.ts,mcp-servers.ts,extensions/*,compaction/*}`，文档 `docs/how-pi-works.md`、`docs/rpc.md`、`docs/json.md`、`docs/virtual-models.md`、`docs/mcp.md`、`docs/skills.md`。

### 会话与上下文

`SessionManager` 默认把会话写成 JSONL 树（`CURRENT_SESSION_VERSION = 3`），每条 entry 有 `id/parentId`；active leaf 决定当前分支，fork/tree/clone 保留历史。模型上下文由 active branch 重建；compaction 写入 summary entry，旧 entry 保留。写入采用同步 append/临时重写；这是简单、可移植的会话日志，但不是事务型 task store。

`AgentSession` 组合 Agent、SessionManager、SettingsManager、ModelRuntime、ResourceLoader、扩展 runner、compaction/retry/cache warming。`AgentSessionRuntime` 在新会话、恢复、fork 时先 abort/持久化旧会话，再替换服务，订阅者必须重新绑定。CLI、print、JSON、RPC 和 SDK 共用这套 AgentSession；RPC 使用严格 JSONL、命令 ID 对应 response，`agent_end` 不是最终 idle 标志，`agent_settled` 才表示自动重试/压缩/队列全部结束。

### Virtual models 与分级路由

`core/virtual-models.ts` 定义“可选择的虚拟模型”与物理模型的分离：选择值记录在 `model_change`/`thinking_level_change`，每次 assistant message 记录实际 provider/api/model；route 接收 `reason`（`user`/`continuation`/`retry`/`direct`）、`previous`、失败消息、分支 state、完整 messages 和 abort signal，返回物理模型+thinkingLevel+可 JSON 持久化 state。`ModelRuntime` 保证不能路由到另一个 virtual model，检查 credentials 并在请求失败/overflow retry 时重新路由。

这已经支持按任务状态、成本或“先强后快”的两阶段 router，也支持扩展调用 classifier（Jev/Clef/Decisions）作前置判断；但内置没有固定的“简单/中等/困难/关键” taxonomy、SLO、成本上限或 fallback 预算。Rust 重写可以保留 virtual model 兼容层，再增加可组合 `TaskClass -> Policy -> RouteDecision` 对象，并把原因、置信度、预算和实际 dispatch 写入 trace。

### 扩展、Skills 与配置

扩展 API 能注册 lifecycle handler（project trust/session/context/agent/turn/message/tool/compaction/provider/request 等）、工具、命令、快捷键、CLI flag、消息/entry/tool renderer、provider、MCP server、virtual model，以及 TUI widget/footer/header/overlay。资源来自用户目录、项目 `.pi`、显式路径、npm/git Pi package；SettingsManager 对全局与项目设置深合并。

Skills loader (`core/skills.ts`) 递归发现 `SKILL.md`，遵循 Agent Skills frontmatter。name 只允许小写字母/数字/连字符，长度≤64；description 必填且≤1024；`.gitignore/.ignore/.fdignore` 生效；同名冲突保留首先发现项并生成 diagnostic；`disable-model-invocation` 技能不进入 system prompt 但可用 `/skill:name` 显式调用。启动只注入 name/description/path，模型按需通过 read/bash 读取完整文件，技能相对路径按 skill 目录解释。

### MCP

`packages/mcp` 是不依赖官方 SDK 的独立客户端。支持 MCP `2025-11-25`，兼容 `2025-06-18/2025-03-26/2024-11-05`；stdio 和 Streamable HTTP；JSON-RPC request correlation、超时续期的 progress、cancel、server ping/roots/list、tools/list 分页（最多 1000 页）、tools/call 结构化结果、resources/list/templates/read、HTTP response stream 重连和 `Last-Event-ID` 恢复、OAuth PKCE/动态注册/refresh/step-up scope。内置拒绝 legacy SSE；MCP README 明确不实现 batch、sampling、elicitation、tasks、servers。

coding-agent 的 MCP 管理层还提供 server 级 `codemode/deferred/direct/hidden` exposure 与 per-tool pattern override。`mcp__server__tool` 命名冲突加 hash；非 direct 工具通过 QuickJS codemode 或 `tool_search` 发现，MCP 结果大于 20KB 中间裁剪且完整结果留临时文件；resource 工具最宽 exposure，资源链接可继续 read。

MCP conformance baseline (`test/mcp-conformance/baseline.json`) 记录已知失败：elicitation 默认项、basic CIMD、scope-retry-limit（设计上失败），且 2026-07-28 stateless 协议未覆盖。迁移时应把“支持 MCP”拆成 transport/工具/资源/OAuth/采样/elicitation/tasks 的能力位，不应只写一个布尔开关。

## `durable` 与 Chord：持久对象/任务运行时

证据：`packages/durable/README.md`、`src/session/session.ts`、`src/harness/{harness,generation,tool,scheduler,task-graph,view,events}.ts`、`src/storage/{sqlite,jsonl,memory}`；`packages/chord/README.md`、`src/{api,types,delta,services,facets}`（各自包内的 `src` 路径）。

### Durable Harness

Harness 把 Conversation、immutable entries、typed JSON documents、Task、Submission 统一放进一个原子 commit 线。`SessionImpl` 的 `#enqueue` 保证所有 commit/read 排队；回调、storage settle、内存 adoption、publication 在同一 mutation line 上完成，观察者之后才收到变更。Storage 有 Memory、SQLite（WAL，`synchronous=NORMAL`）和 JSONL（可选每次 commit `fsync`），也有 Cloudflare Durable Object adapter。

生成任务 (`pi.generation`) 每个 checkpoint 持久化 provider turn、tool calls、compaction、retry；工具是独立 `pi.tool` task。工具在执行前先 commit `phase: execute` intent 和 `replay: safe|unsafe`。崩溃恢复时只有 storage 与当前 tool 都标记 safe 才重跑，否则写入 interrupted error，保留已提交的 partial output。task 可以拥有 child tasks，以 `waiting`+`allSettled/failFast` 恢复，foreground owner 随父 abort，`background` anchor 跨父流程存活。

Conversation view 从 `pi.live/pi.inbox/pi.agent/pi.provider/pi.usage` 文档派生；`watch` 提供 exact commit ops，`watchEvents` 由 commit 推导 coding-agent 风格事件，并在积压超过 100 batch 时发送 snapshot。部分答案/工具输出默认每 100ms commit 一次；可配置 progress interval。已有 compaction、submission requestId 幂等、fork、handoff、busy steer/follow-up、usage aggregation、task graph。

### Chord 的 Object/扩展基础

Chord 的实际抽象接近“万物皆 Object”，但类型单位叫 `Facet`、`Service`、`ReplicatedState`、`Context`。Plugin/facet 在 setup 阶段通过 `env.provide/use/observe` 形成依赖图；host 校验完整图，按 provider→consumer 激活，反向释放；`FacetHost.reload()` 可在不中断稳定 service facade 的情况下原子替换 provider。Service token 可 singleton 或 keyed，可本地或远程；remote wire 只定义严格 JSON 的 call/catalogue/snapshot/update，外层 transport 由应用决定。

`delta` tracker 对严格 JSON 记录字符串 append/trim、数组 splice/permutation、set/delete；每个远程订阅独立 path codec，缓冲最多 100 update，溢出转完整 reset snapshot。Replicated state 的 immutable ownership 要求调用方在转移后不再修改 root；draft 只在 change callback 有效。这是 Rust Object graph 的好参考，但需在 Rust 中把 ownership、lifetime、capability 和 remote schema 变成编译期/serde 边界。

### Durable/Chord 局限

`durable` README 将 API 标成 Experimental；单 storage 由一个进程拥有，没有跨进程锁。Chord 的 remote boundary 不负责认证；protocol/server/client README 也明确 peer authentication 和 lifecycle 在应用策略中。默认 durable 事件/文档没有自动 telemetry trace ID，需宿主自行关联。

## Telemetry、日志与事后复盘能力

证据：`packages/telemetry/README.md`、`src/index.ts`、`src/memory.ts`、`packages/ai/src/types.ts`，`packages/coding-agent/src/core/{timings,telemetry}.ts`。

`pi-telemetry` 提供显式 callback API：`TelemetryContext.startSpan(options, callback)`、`TelemetrySpan.startSpan/addEvent/setAttributes/setStatus`。adapter 必须保持同步 admission、父子 span、promise settle、last-write-wins 状态和 recording non-throwing；`InMemoryTelemetryContext` 只在内存按 start 顺序记录数字 ID、parentId、属性、事件、状态和 end sequence；没有 exporter、global current span、时间戳或后端依赖。`NOOP_TELEMETRY_CONTEXT` 是共享惰性实现。

`pi-ai` 的 `ProviderRequestOptions` 只接受并透传 `telemetryContext`；本提交中 `agent/src` 没有 `AI_TELEMETRY_SCHEMA`、`HARNESS_TELEMETRY_SCHEMA` 或 `startAiSpan/startHarnessSpan` 实现，coding-agent 的 telemetry 文件只负责安装遥测开关，`timings.ts` 只在 `PI_TIMING=1` 下输出启动阶段毫秒。因此 README 中所称“agent 导出的 typed schema”与实际导出存在漂移，不能据此声称已有默认全链路 tracing。

Pi 仍有可复盘的原始材料：JSONL session entries、JSON/RPC 的 agent/turn/message/tool/compaction/retry events、tool `durationMs`、provider response/stream hooks、MCP stderr log、TUI debug/crash logs、durable commit/task graph。但它们缺少统一 trace/span/run/attempt IDs、跨进程 correlation、结构化敏感字段策略、事件 schema version 和独立 append-only audit sink。Rust 版本应把这些作为 Object 生命周期的默认 envelope，支持 redaction、sampling、backpressure 和可插拔 OTLP/JSONL exporter。

## TUI、codemode 与现有界面边界

`pi-tui` 是终端组件库，不是桌面 GUI。`Component.render(width)->string[]`，`TuiBase` 以 16ms 最小渲染间隔调度；main/alt screen 保存上一帧 lines，仅计算首末改变行并输出 ANSI diff，CSI 2026 synchronized output 避免闪烁；宽高变化、viewport 外改变或 Kitty image 区域异常时 full redraw。组件包括 editor、markdown、select/settings list、scroll/overlay、mouse/focus、Kitty/iTerm2 image、OKLCH/OKHSL 颜色。

`codemode` 在 QuickJS-WASM 中运行模型写的 async JavaScript，唯一能力是注入 tools/globals；无 fetch/process/require/timers/modules，`timeoutMs` 和 `memoryLimitBytes` 限制，store 只返回 JSON writes。MCP codemode 调用不会把内部嵌套 tool calls 注入 LLM context，最终只返回脚本输出。

CLI 还提供 print/JSON/RPC、Node/Bun SDK 以及实验性的 local `protocol/client/server`：固定长度+CBOR frame、opaque strict-JSON payload、server/session/attachment route，Chord 负责实际 service grammar。它可以承载未来 Web/IDE presentation，但仓库没有 Zed/Zcode 风格桌面 GUI、浏览器前端或跨平台窗口层；用户要求的 GUI 是新增产品面。

## 性能、并发与安全评估

### 已有强项（静态证据）

- provider stream 归一化、缓存 affinity、动态 auth/model refresh 和 retry 已分层，避免每个 UI 重复实现。
- agent 并行工具、每文件 mutation queue、bounded tool output、MCP progress timeout renewal、TUI differential rendering 和 Chord delta replication 减少重复工作/输出。
- durable 的单写入线、SQLite WAL/JSONL fsync 选项、checkpoint/replay policy、requestId 幂等和 task ownership 形成明确 crash semantics。
- 所有主要入口均可自定义：provider、virtual router、tool/hook、resource loader、storage/env、facet/service、telemetry adapter、TUI component、RPC transport。

### 主要风险/缺口

- Agent 与普通 coding-agent 默认仍是进程内循环+JSONL append；provider 请求、工具、副作用和 UI 渲染没有统一预算/优先级/公平队列。并行工具无全局并发上限，durable 默认 progress commit 仍可能产生高频 storage 写入。
- Chord remote service 没有内建认证/授权，MCP 项目配置/扩展在 trust 后可执行进程权限；Pi README 明确没有 filesystem/process/network permission system，生产部署必须 sandbox/container。
- trace 记录不是默认行为，日志可能包含 prompt、response、文件内容和凭据相关数据（docs/debug/export 均有提示）；缺少统一 redaction policy。
- 模型分级只有 thinking 七级和可选 virtual router，缺乏可审计的任务难度 taxonomy、预算预估、fallback 资源配额、deadline/SLO 和结果质量反馈闭环。
- MCP core 明确不实现 sampling/elicitation/tasks；GUI、远程鉴权、跨进程生命周期也处于实验切片或宿主责任。

## 功能 parity 矩阵（面向 Rust 重写）

| 能力 | Pi 现状与证据 | Rust parity 目标 | 状态/备注 |
|---|---|---|---|
| 多 provider chat | `ai` Provider/Models + `api/*` | Provider trait、统一 stream/error/usage | 必须保留 |
| image/classifier | `ImageModel`/`ClassifierModel`、codemode APIs | Object capability + typed calls | 必须保留并纳入分级 router |
| thinking 等级 | 七档 `thinkingLevelMap` | 兼容七档，扩展 task class/预算/质量策略 | 需扩展 |
| virtual model | route reason/previous/failed/state | Policy object、实际 dispatch 与 trace | 必须保留并增强 |
| Agent loop | `agent-loop.ts` 工具/队列/事件 | async state machine、取消、并行/串行策略 | 必须保留 |
| tools/schema | TypeBox 校验、before/after hooks | serde schema、capability/permission、deadline | 必须保留并强化 |
| 普通会话 | JSONL tree/session v3、branch/compaction | versioned append log + atomic index | 必须保留 |
| durable 恢复 | Harness/Tasks/SQLite/JSONL/checkpoint/replay | Rust durable object/task store | 必须保留；实验 API 可重设计 |
| 子任务 | durable owned child/background task graph | Object ownership、join/failFast、budget inheritance | 必须保留 |
| Chord facets/services | typed tokens、reload、replicated delta | DSH Object registry/facet/service/replica | 直接借鉴并改为 Rust ownership |
| Telemetry | explicit context + in-memory adapter；无默认全链路 | 每次 run/attempt/tool/provider 自动 span、OTLP/JSONL exporter | 必须增强 |
| MCP stdio/HTTP | protocol 2025-11-25、OAuth、tools/resources | transport/object adapter；增加 sampling/tasks/elicitation 可选能力 | 核心保留，能力位化 |
| Skills | Agent Skills `SKILL.md`，按需 read，collision diagnostics | 同规范 + Object discovery/签名/版本/权限 | 必须保留并可审计 |
| Extensions/plugins | TS module、hooks、provider/tools/UI；Chord facet bundler | WASM/native plugin ABI，声明依赖与热 reload | 必须保留，ABI 设计关键 |
| Codemode | QuickJS-WASM、timeout/memory、tool-only | Rust sandbox/WASM，严格 capability | 必须保留；安全边界默认化 |
| TUI | diff renderer、alt/main、mouse/overlay/images | ratatui/crossterm 或自研 diff，事件与 Object state 绑定 | 必须保留 |
| JSON/print/RPC/SDK | JSONL events、RPC commands、Node SDK | versioned JSONL + binary/CBOR RPC，语言无关 client | 必须保留 |
| Zcode 类 GUI | 仅实验 client/server service boundary，无桌面 GUI | 新增 Web/desktop presentation facet，复用 replicated state/RPC | 当前缺失 |
| 权限/sandbox | Pi 无内置权限系统，依赖容器/宿主 | capability grants、filesystem/process/network policy、approval | 必须增强 |
| 全面测试 | Vitest/node:test、MCP conformance baseline、evals、storage/telemetry conformance、bench | Rust unit/property/conformance/e2e/stress/fault injection | 需建立等价矩阵 |

## 建议的 Rust/DSH Object 重写切分

1. 先实现 `object-core`：严格 JSON/CBOR value、Object ID/version、Context cancellation/deadline、capability、typed event envelope（trace_id/run_id/attempt_id/parent_id）。
2. 以 `model-runtime` 兼容 `pi-ai`：Provider、ModelCatalog、CredentialStore、stream/image/classifier/deferred；把七档 thinking 映射成可配置 `TaskPolicy`，增加 complexity、latency、cost、quality、risk 五维约束。
3. 以 `agent-runtime` 重建低层 loop：保持事件顺序、steering/follow-up、并行工具的源序列持久化；所有 tool/provider/route 都自动创建 span，并把权限、预算和 cancellation 作为 Object context。
4. 以 `durable-object` 重建 Harness：commit log、typed docs、task checkpoints、replay policy、owned child graph；SQLite/JSONL 先做 conformance，再接远程 replica。
5. 以 `object-registry` 对应 Chord facets/services：声明 provide/use/observe，校验依赖图，singleton/keyed/remote service，generation-safe hot reload，replicated state delta/reset。
6. 在其上接 `mcp-adapter`、`skills-loader`、WASM codemode 和 provider/plugin ABI；保留 MCP exposure 分级并将采样/elicitation/tasks 作为 capability flags。
7. 最后接 TUI 和 GUI：TUI 直接订阅 Object replicated state；GUI 通过 versioned RPC/CBOR + WebSocket/Unix transport。Presentation 不应拥有 agent truth，只消费 session view/task graph/trace streams。

## 验证与测试基线

上游已提供可复用测试意图：`packages/agent` loop/tool/queue 测试，`coding-agent/test/suite` faux provider 回归，`test/virtual-models.test.ts` 与 `jev-router-example.test.ts`，MCP OAuth/command/conformance baseline，`durable/test/*recovery*`、storage/env conformance、storage benchmarks，`chord/test/delta*.bench.ts`，telemetry adapter conformance，TUI virtual-terminal 测试，evals 的 Docker 隔离/文档 lift。Rust 版本应先用 faux provider 做确定性多场景测试，再做 property-based delta、crash/restart、并发工具、MCP transport/OAuth、技能冲突、router budget、trace completeness 和 TUI golden frame；真实 provider 与付费模型只进入显式 smoke/eval 阶段。

## 结论

Pi 已是高扩展性的 TypeScript agent harness，且最新提交已经包含 durable、Chord、telemetry、virtual models 和较完整 MCP；重写不应从“最小聊天循环”开始，也不应把缺失的 GUI/全面 tracing/权限策略误写成已有能力。最有价值的 Rust 差异化是：以 DSH Object 统一运行时实体、持久化与远程复制；把模型分级做成可审计的策略对象；默认生成跨 provider/tool/task/UI 的调用轨迹；用 capability + durable checkpoint 把安全、恢复、资源预算和插件隔离变成一等语义。
