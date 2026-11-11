# DSH 与 ZCode 源码参考分析

分析日期：2026-10-07。本文记录固定上游版本的设计事实、适合 Rust 重写的迁移原则，以及尚未验证的边界；不把阅读源码等同于运行测试或性能测量。

下载核验：分析期曾对 `upstream/dsh` 与 `upstream/zcode` 做完整 Git clone 并 checkout 到下表 SHA；本地副本已随清理移除，可按 `docs/upstream-lock.json` 的 commit 重新 clone 检查。

| 项目 | 官方仓库 | 分析基准 | 许可证 |
| --- | --- | --- | --- |
| DeepSeek Harness / DSH | `deepseek-ai/deepseek-harness` | `5badb15009ae1756c3afe0ae0cef1faafc290ccc`，`master` | MIT；另有第三方声明 |
| ZCode | `zai-org/ZCode` | `29628c9acdb81b703bbd4080c207a0e7ce5e276e`，v3.14.3 | Apache-2.0；另有第三方声明与组件清单 |

## 1. 先校准“万物皆 Object”的含义

DSH 官方采用 **Everything is a Plugin**，不是把所有数据装进一个无类型 `Object` 容器。README 明确其由 Cordis 驱动；`docs/cordis-primer.md` 的五个核心概念是：插件为实现 Service 的对象，Context 为服务仓库，inject 为依赖声明，typed events 为通信边界，reversible effects 为注册与释放机制。`vendor/cordis/src/service.ts` 中 Service 构造器调用 `ctx.reflect.provide()`，使服务随所属 fiber 卸载；`vendor/cordis/src/context.ts` 中 Context 通过服务解析代理暴露能力，`extend()`、`isolate()`、`intercept()` 创建子上下文而不修改父上下文。

这对本项目的准确迁移方式是：用统一 Object 身份、元数据、版本、作用域、能力描述与生命周期管理 Agent、Tool、ModelProvider、MCPServer、SkillProvider、Session、Storage、UIAdapter 等对象；实际操作仍使用专用 Rust trait 与显式类型。可序列化的描述和运行时 `Arc<dyn Trait>` 应分离，避免跨异步边界传递不受约束的 `Any`，也避免将 JSON registry 误称为完整的可替换执行框架。

## 2. DSH：可组合执行框架

### 2.1 配置、插件与依赖

`docs/architecture.md` 与 `packages/boot/app-boot/README.md` 将应用定义为有序插件树：profile 选 bundle，bundle 提供 config rows，随后依次应用 profile patch、home patch、CLI `--patch`。模型适配器、工具、会话存储与 agent loop 都是可替换插件。每一层都可定位配置行；对象实现不是写死在 UI 中。需要区分“可发现”“可替换配置”“可卸载实现”三个层次。

Cordis 事件分为 emit、waterfall、parallel、serial、bail。waterfall 是 around middleware，监听者必须调用 next 才继续；policy 可以直接短路。服务依赖通过 inject 激活，不靠手工启动顺序。effects 负责取消监听、注销工具与关闭资源。Rust 版本需要对依赖图做缺失与环检测，提供注册 lease、明确关闭与等待 drain，不能仅依赖同步 Drop 来关闭子进程或网络连接。

`vendor/README.md` 记录上游 Loader 的 eager、非事务式配置变更，部分激活失败可能留下部分应用的树；因此不能假设 DSH 的所有 HMR 都具有原子回滚。Rust 版本可将配置校验、候选 generation 准备、健康检查、发布与旧 generation 释放分成明确阶段。运行中的调用应持有原 generation，避免热替换时悬空。

### 2.2 Object 作用域与生命周期

`packages/core/scope/src/index.ts` 将 ScopeKey 定义为不透明对象身份；默认 loop 使用 live Agent 本身作为 key，但 scope primitive 不检查对象内容。注册上下文负责可见性与所有权；子作用域继承祖先 registry，事件向祖先监听者传播。`Scoped<T>` 是路由 carrier，不是实际业务对象。`bindScopeParent()` 检查环；`createScope()` 提供精确 disposer 和共享 quiescence boundary。

`packages/core/agent-loop/README.md` 的创建事务先构建私有 session、agent、scope，完成 setup 后发布 registry，再 await serial `agent/created`，最后释放输入。失败回滚；卸载顺序是停止并 drain driver、释放 scope、关闭 session 写句柄、注销 agent、注销 session。`packages/core/agent/src/index.ts` 的 detach 捕获精确 entry 身份，旧 disposer 不能删掉后来同 id 的对象。

Rust 建议用 `{ObjectId, Generation}` 标识对象实例，区分持久化身份和运行期 owner；scope graph 明确 parent；注销必须验证 generation。取消结束与资源关闭应幂等，多调用者等待同一完成状态。Session 与 Agent 的持久化父子关系，不应代替运行时资源所有权。

### 2.3 Agent turn、step 与工具调度

`docs/architecture.md` 和 `packages/core/agent-loop/src/agent.ts` 的 turn 是多个 step，step 是一个模型请求及其工具组。默认路径为输入 claim → prompt/schema assembly → pre-step → request route → prepared call → durable 用户输入与 request metadata → stream → tool calls → tool results → 下一 step。模型请求绑定实际 prepareCall 路由，retry 不重复用户 admission；取消中保留已经向用户展示的流式文本。

`packages/core/agent-loop/src/tool-calls.ts` 在 parallel-safe 工具上使用有界池，exclusive 工具为排序屏障。默认 `maxParallelToolCalls=10` 可配置。Rust 版本应声明工具 effect class、resource key、幂等性及读写边界，按“可并行且不冲突”调度，不应让每个工具无限 tokio::spawn。模型请求、MCP、子 agent、后台作业都需要独立配额、队列与超时。

### 2.4 Policy 与工具 pipeline

`docs/tool-execution-pipeline.md` 和 `packages/core/tools/src/index.ts` 的顺序是：durable tool/call → pre-execute → 单调 guards → approval → around execute → tool body → content projection → post-execute → 归一化 → finalize content → frozen final outcome → durable tool/result。monotonic guard 不能被普通 hook 重排绕过；缺失或不可回答的 approval 会拒绝。调用身份不可被 middleware 替换；工具必须将取消传到其拥有的异步工作，并等待 quiescence。框架不能硬杀同进程插件。

`packages/sandbox/sandbox-policy/README.md` 统一 bash、filesystem、terminal 的文件影响策略；模式切换是一条 `sandbox/mode` session event，重放可恢复，cwd 为 immutable workspace root。其模式只约束文件影响，不含网络和进程政策。`SAFETY.md` 明确 developer preview、未完成安全审计；插件组合或 scope 不能当操作系统隔离。

Rust 版应让 policy 独立于工具实现与 UI，工具在 Agent/MCP/手工调用路径经过同一 guard；默认权限可配置，同时保留不可被低优先级 hook 覆盖的约束。Process plugin/MCP 的隔离边界与进程内 native plugin 分开设计。

## 3. DSH：日志、复盘与模型路由

`docs/architecture.md` 的关键原则为 **Model-visible means logged**。session append-only event log 是模型上下文来源，deriveMessages 从日志投影，不从临时 UI 状态拼装。`packages/core/session/README.md` 记录 request/header 为 canonical 请求配置、adapter defaults、tool schemas；request/context 记录实际 provider、model、contextWindow 和 prompt update capability。系统提示词是日志 surface node。失败、重试、取消和 stream-error attempt 可作为 assistant/attempt 保存但不加入模型历史；成功 assistant/message 带 compact timed stream。

因此完整复盘至少要保留：配置与插件 generation、任务需求与路由原因、实际模型和 reasoning effort、prompt/工具 schema 版本、请求与 attempt ID、流式节奏、tokens/usage/cost、工具前后事件、拒绝/审批、重试与取消原因、对象父子关系及最终输出。只写 timestamp+文本不是可重放 trace。运行日志和可重放 session event 是不同的数据面：前者用于诊断，后者用于状态与上下文恢复。

DSH 的路由扩展点 `agent/request` 与可替换 `ctx.llm` 适合模型分级插件。本项目可将模型 intelligence tier 与 provider-specific reasoning effort 分开：tier 表达任务需要的能力，effort 表达同一模型的计算配置。需要以能力约束筛选，再依据任务类型、复杂度、风险、预算、延迟目标、上下文与工具/图像能力评分；失败后升级并记录理由。模型名称不能当智力评分，关键词规则只能作为可替换初版 policy。不能仅改变日志中的 model 字段而仍向同一个实现发送请求。

物理存储不宜直接照搬 DSH 所有版本机制。DSH JSONL 持久化通过 versioned generations 与 adjacent migration 保留源文件；本项目第一版可以 JSONL 加序号、schema_version、校验/恢复尾部，再逐步加入 SQLite 索引、压缩与明确写所有权。必须说明硬崩溃前未持久化的 stream 数据仍可能丢失。日志脱敏策略、正文保存策略、保留期、大小上限与导出访问应该可配置；脱敏后的导出不再等于原始精确重放。

## 4. DSH：MCP 与 Skills 的实际边界

### 4.1 MCP

`docs/subsystems/mcp.md` 与 `packages/mcp/mcp-client/README.md` 将每个 server 当连接插件，而不是一个共享 `ctx.mcp` 大对象。client 使用官方 SDK，支持 stdio、Streamable HTTP、tool discovery/call、取消、超时、重连、原子 tool generation、resources list/templates/read、server instructions 进入 logged system prompt。serverName 为本地配置身份，raw tool name 仅用于 wire 调用；公开工具名有命名空间，避免 remote serverInfo 改名影响 session history。stdio 环境剔除 ambient secrets，显式 env 再合并。

同一 scope 内 serverName 唯一，不同 Agent scope 可以复用。发现失败保留旧 generation，注册冲突回滚整个新 generation；连接关闭后工具注销，释放等待队列与在途操作。resources 是 demand-driven，连接不等于自动读取资源。

**DSH 当前文档明确未支持 MCP prompt templates、human-input elicitation、task-based execution、resource subscriptions。** 不能用“参考 DSH”推导“完整 MCP 已完成”。Rust 版本应以官方 rmcp 对实际协议版本做 negotiation，先形成能力矩阵，再逐项测试 prompts、resources、tools、sampling/elicitation/tasks 等 SDK 与规范允许的特性。不支持时必须显式报错，不能静默丢弃 rich content 或 isError。

### 4.2 Skills

`docs/subsystems/skills.md` 将 skill 拆为 service definition `ctx.skills`、filesystem/packaged provider 和 model-facing consumer。provider list/get 分离：摘要发现，完整 body 按需加载；不完整 discovery 不覆盖 last-good catalog。scope 层级与 provider rank 管理重名，modelInvocable 与 userInvocable 独立。filesystem provider 支持 `<name>/SKILL.md` 与 `<name>.md`，不递归扫描任意嵌套树，资源相对 provider resourceBase 解析。

`packages/skill/tool-skill/README.md` 在第一请求前记录摘要 catalog；变更追加完整 replacement，包括空 catalog 以退休旧名字。model 用 skill tool 加载正文，用户 `/name` gesture 可以注入同一正文渲染；摘要不能替代真实 instructions。Rust 版应记录加载的正文 digest、来源、版本、relative resources、使用路径和可调用性，并允许可配置来源与 precedence。Skill 是指令与资源，不应因发现 SKILL.md 就自动执行脚本。完整支持还需 frontmatter 容错、冲突诊断、缺失资源、更新/卸载与调用策略测试。

## 5. ZCode：GUI 的真实公开范围

当前固定 HEAD **公开了客户端、后端、shared UI、Agent CLI 与 runtime 源码**，并非只提供安装包。`README.en.md` 对此有明确说明，`apps/zcode-cli/` 是普通目录，不需要另拉 submodule。可实际读取 `packages/ui`、`packages/web`、`packages/desktop`、`packages/server`、`packages/zcode-server-cli`、`packages/rpc`、`packages/services` 与 Agent CLI/core。本文没有安装并启动该上游，因此只确认代码可读与架构，不声称整个 ZCode 功能已经实测。

| 参考点 | 源码位置 | 可迁移原则 |
| --- | --- | --- |
| Web 与 Desktop 共用 UI | `packages/web/src/main.tsx`、`packages/ui/package.json`、`packages/desktop` | 业务状态与展示分开；本地 Web GUI 不需要强行引入 Electron |
| 服务与 WebSocket channel | `packages/server/src/http.ts` | 统一服务端权限、生命周期与事件；GUI 不直接持有 Agent loop |
| 传输无关 RPC | `packages/rpc/src/protocol.ts` | send/onMessage 窄接口，上层不依赖 WebSocket/IPC 的具体实现 |
| RPC 调用日志 | `packages/rpc/src/logging-middleware.ts` | 通过 middleware 记录 register/call/listen、耗时与失败；Rust 需要再加 trace/span/causal ID |
| 多 session 共享数据层 | `packages/ui/src/v4/sessionDataLayer.ts` | 每连接一个 data layer，同 session 多 pane 共用投影；引用计数和 keep-warm；关 pane 不停止 session |
| 工作区与任务导航 | `packages/ui/src/WorkspaceSidebar.tsx`、`WorkspaceFileTree.tsx`、`WorkspaceGroupedTasksSection.tsx` | workspace、task、session 分层，不能只做单聊天窗口 |
| 工作流时间线 | `packages/ui/src/components/workflow-timeline/WorkflowTimeline.tsx` | 执行过程投影成阶段、agent、状态和依赖，点击钻取 transcript |
| 视觉设计系统 | `DESIGN.md` | 面向长会话、键盘操作和高信息密度，semantic color/font/spacing token，亮暗主题一致 |
| 架构校验 | `architecture-policy.yaml` | module roots、public entrypoint、禁止环/深层引用；多数旧模块仍 `managed: false`，不应宣称全仓严格分层 |

ZCode README 的本地 Web 模式默认监听 loopback，非本机监听默认生成 token。Rust 服务也应明确本机/远程部署的认证边界，并让 TUI 与 Web 复用同一个 application API、事件序列和 replay，而不是维护两套 Agent 状态机。GUI 初期建议工作区/会话列表、流式 transcript、工具参数结果、模型路由与 usage、对象与插件管理、MCP/skill 状态、可过滤 trace；复杂 IDE、SSH/WSL、支付/账号/分享功能不属于第一版必需项。

## 6. 引用、扩展和性能评估限制

DSH MIT 与 ZCode Apache-2.0 均允许修改与再分发，但复制代码或资源时要保留对应版权/许可证和需要的 notices；第三方依赖、字体、图标与 native/runtime 组件还需按自己的许可证处理。ZCode 的 `NOTICE.md`、`THIRD-PARTY-NOTICES.md`、`third-party/inventory.json`、`third-party/copied-components.json`、`third-party/embedded-components.json` 应作为使用其实际资源时的追溯入口。Apache-2.0 不授予项目商标使用权；新的 Rust 项目采用独立名称与独立视觉资产更清晰。

此次分析未给出 Node→Rust 的倍率，也未测上游 p95。语言切换能降低宿主开销、提高类型与并发资源管理能力，但 LLM 网络等待、模型推理、MCP server、shell 与 context token 通常仍主导端到端延迟。插件 scope、事件日志、rich rendering 和重放均有成本，应分别测量冷启动、idle RSS、object lookup、scope resolution、日志 append/flush/replay、大 session、并发工具、UI 长列表、MCP 重连和取消 drain。先固定功能与工作负载，再比较宿主性能；同时报告模型质量、成本、任务成功率和工具正确性，避免用微基准替代 Agent 能力。

本项目第一阶段应将“可运行核心”“参考设计映射”“已实现并测试的能力”和“后续 parity 工作”清楚分开。仅注册对象目录不等于 Cordis 等价插件系统；仅 HTTP chat 不等于 Pi 全部 provider/streaming/session 功能；仅 SDK initialize/list tools 不等于完整 MCP；仅页面 shell 不等于 ZCode GUI parity。
