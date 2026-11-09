# rπ Web 工作台前端重构基准 — OpenCode × ZCode 分析与落地设计

> 依据：OpenCode（sst/opencode，`upstream/opencode`）与 ZCode v3.14.3（`upstream/zcode`）源码深度分析。
> 目标：对 rπ 的 Web 工作台做完整重构——对齐 ZCode 的会话管理与轨迹追踪、OpenCode 的消息/工具/思考渲染，
> 全局 UI 以 shadcn/ui 风格组件库落地（vanilla JS + CSS 变量，无框架、无构建，受 CSP `script-src 'self'; style-src 'self'` 约束）。

---

## 一、两个项目的前端设计结论

### 1. OpenCode（SolidJS + TypeScript，packages/session-ui / app / tui）

**数据模型**（`packages/schema/src/session-message.ts`）：
- 消息是 tagged union：`user | assistant | system | synthetic | compaction | idle | shell | …`。
- assistant 消息的 `content[]` 就是 part 流，part 只有三种：`text | reasoning | tool`。
- 工具 part 四态（流式渲染核心约定）：
  - `streaming`（input 是 JSON 字符串增量，UI 容错解码）
  - `running`（input 已解析 + metadata 可带流式进度）
  - `completed`（input + content 输出 + metadata）
  - `error`（input + error 对象独立字段）
- reasoning part：`{ type:"reasoning", text, time:{created, completed?} }`，completed 有值即"想完了"，据此算耗时。
- turn 结束由独立 `idle` 消息标记（outcome: succeeded/failed/interrupted）。

**工具调用 UI**（`session-ui/src/tools/tool-renderer.tsx` 2722 行）：
- 注册表模式：`Record<name, {name, render?}>`，未知工具落 `GenericTool` 兜底。
- 文案/图标全部前端按 `tool.name` 生成（i18n），服务端 metadata 只给结构化事实。
- 每工具 trigger 三段式：`title + subtitle（弱色文件名/目录）+ args（key=value 小徽章）`。
- 三态显示：running 用 TextShimmer + Spinner；completed 停 shimmer 出现副标题；error 换 `ToolErrorCard`（警告图标 + 与成功态一致的副标题 + 折叠显示首行错误摘要，展开显示全文 + 复制）。
- 折叠策略单一事实来源 `currentContentDefaultOpen`：error 折叠、shell 跟随用户设置、edit/write 跟随设置、其它默认收起。
- `BasicTool` 容器：pending 时禁止展开（防流式抖动）、locked 强制展开、重型内容 rAF 队列延迟挂载。
- 连续同类工具合并成组（"Used N Tools"、连续 read 合并成一行文件名列表）。
- 工具行高 36–40px，触发行字号 13px。

**思考过程**（`message/message-content.tsx` + TUI `context/thinking.ts`）：
- 默认折叠；流式时标题 shimmer "Thinking"，副标题 = 从思考文本提取的最后一个 markdown 标题（四条正则：HTML h1-6 / ATX `#` / setext / `**粗体**`，取位置最靠后者）。
- 完成后标题变 "Thought" + 耗时。正文降级排版：muted 色、13px、标题不放大不加粗——靠排版而非边框区分。
- TUI 交互：收起为一行 `Thought: <摘要> · 2m30s`，完成后左侧 1 字符竖线变引用块。

**会话管理**（`app/src/home/sessions/`）：
- 按本地日历日分三组：今天 / 昨天 / 更早（前两组为空时改标题"最近会话"）。
- parentID null 过滤掉所有子会话；子会话从父会话的工具卡跳转进入。
- 排序 updated 优先；hover 浮现操作按钮；删除/归档后相邻行 FLIP 动画回填。

**模型选择**（`app/src/providers/models/select-dialog.tsx`）：
- 命令面板式：搜索 + 键盘导航 + 分组头；无搜索时 Favorites/Recent 置顶，有搜索时只按 provider 字母序分组。
- 行内：模型名 + provider 名 + Free/Latest 徽章 + 固定宽度 variant 槽（hover 才显示，防抖动）+ 选中 check。

**Timeline 投影**（`timeline/projection.ts` 1128 行，纯函数零框架依赖）：
- `messages → rows[]（纯函数投影）→ 按 rowKey 复用 diff 渲染`。12 种 tagged row，各有稳定 key。
- user 消息开新 turn，turn 内 steering 归入同一 turn。
- part 合并规则：默认展开的工具才值得独立成行，默认收起的折进 context 组。
- 同 turn 内最后一个工具调用之前的 text 是过程评论（commentary），只有最后的 text 是答案。
- auto-scroll 单一事实源：pin/unpin；用户向上滚动或选中文本即 unpin。

**设计系统**（`ui/src/styles/tokens/*.css`）：
- 纯 CSS 变量双层 token：原始色板 → 语义层（`--v2-background-bg-base`、`--v2-text-text-muted`、`--v2-border-border-muted` = 半透明黑）。
- 边框全是半透明色（亮底黑 alpha、暗底白 alpha）；圆角 xs 0.125rem → xl 0.625rem；4px 网格。
- 组件样式用 `data-component`/`data-open`/`data-status` 属性选择器驱动，天然适配无框架重写。

### 2. ZCode（React + Tailwind v4 + shadcn 风格组件库，packages/ui / services / shared）

**轨迹追踪 ModelTrajectory**（本次重构的对齐核心）：
- 一次模型调用 = 一个 CallCard。面板：header（标题/搜索/按角色展开菜单/全部展开收起/刷新/关闭）→ 汇总行（N 次调用 · 总 token · 模型列表）→ 虚拟化时间线。
- CallCard sticky 摘要行：`01` 补零序号、来源分类（主会话/子智能体/压缩…）、finishReason 徽标、`IN x · OUT y · 3.2s · 14:08:31` 等宽元数据。
- section 只有两种：**输入**（本次请求新增消息，每条一行）+ **输出**（reasoning → 正文 → 逐个 tool call 固定顺序）+ 独立 error 块（红底 + 警告图标 + 可滚动 stack）。
- 消息行左侧角色标签列：`SYSTEM/USER/ASSISTANT/TOOL` 等宽大写按角色着色。
- **消息 delta 去重**（关键算法，~40 行纯函数）：每次请求的 messages 是全量上下文，直接渲染会重复。第一条展示全量；后续主会话只展示比上一条多出的尾部消息且过滤 assistant（assistant 由上一条的 Output 呈现）；消息数变少（压缩后）则整段展示；sidecar/compact 类请求有独立 prompt 不套用。
- **展开系统**（`ModelTrajectoryExpansion.ts` 仅 42 行）：6 个视觉角色（system/user/reasoning/assistant/tool-call/tool-result）各有 `{expanded, version}` 批量命令 + 单项 override Map（`Map<key, {open, commandVersion}>`），批量命令 version 大于 override 记录的 version 时命令生效——全局操作不被旧单项状态污染；搜索命中强制展开。
- **搜索**：空白归一化 + 小写匹配，`starts[]/ends[]` 把归一化偏移映射回源文本偏移；高亮用 CSS Custom Highlight API 不改 DOM。
- **大输出**：默认 `max-h-64` 钳制 + 底部 72px 渐隐 maskImage + ResizeObserver 测溢出 + 悬浮毛玻璃"展开/收起"按钮；错误载荷渲染 `bg-destructive/10 text-destructive` pre 块。
- 工具行折叠摘要：方向徽标（输入/输出，错误变红加警告图标）+ 工具名/callId 双 pill（等宽字体、max-w 截断）+ 单行预览 + hover 复制按钮（复制真实错误文本而非包装 JSON）。
- 内容 part 统一归一化为 6 种：`text | reasoning | tool-call | tool-result | image | unknown`；工具错误标准化 `{type:"error-text", value}`。

**会话/任务管理**（packages/ui）：
- 任务元数据：`taskId/title/createdAt/updatedAt/status(running|completed|error)/model/unreadAt/forkedFromTaskId/lastError`。
- **两层运行置顶稳定排序**（`taskListOrdering.ts`）：running 层整体置顶且按 createdAt + taskId 决胜（**禁用 updatedAt**，防止流式事件导致行跳动）；非运行层按用户偏好时间排序。
- 行内状态优先级：**error（红点）> unread（蓝点）> loading（spinner，需实时活动证明）> none**。
- 标题溢出用右侧渐隐 mask 而非省略号；元信息：变更摘要 `+N -N`、相对时间（刚刚/N 分钟前/N 小时前/N 天前）、fork 图标；hover 操作（归档两击确认：首次点击变"确认"destructive 按钮，鼠标离开不取消）。
- "新建任务"不立即创建：进入草稿态，发送首条消息才落库。
- 顶层分区：任务 / 置顶 / 归档（归档带独立搜索）；分组吸顶标题。

**模型选择**（`ModelConfigSelect.tsx` 663 行）：
- 两级结构：一级 provider（带选中勾），二级模型列表；无分组退化为单层列表；tier 用组标题徽标表达（Free/Team/Individual）。
- 行内：模型名 + badgeLabel + "视觉"能力 pill + 选中 check；锁定模型灰显 + tooltip 原因；底部 sticky"管理模型"入口。
- 快捷键 Ctrl+M 打开；关闭后焦点恢复到聊天输入框。

**实时消息 part 全集**（`zcode-protocol-legacy-types.ts`）：
`text | reasoning | file | tool | step-start | step-finish | snapshot | patch | compaction | timeline | subagent | agent | retry`；工具 part 判别联合四态 `pending/running/completed/error`；`parentToolUseId` 支撑子 agent 嵌套树。

**设计系统**（`ui/src/styles.css` 2053 行 + `DESIGN.md` 537 行）：
- 全部语义色从 neutral 调色板 `color-mix(in oklab, …, transparent)` 派生半透明色，切主题只换基色。
- 字号语义刻度：`--ui-font-size: 14px` 基准，xl=+4 / sm=−2 / xs=−4（10px 徽标）。
- 四主题面：`:root`（浅）/ `.dark` / `.theme-zai-light` / `.theme-zai-dark`，默认 zai-dark。
- 圆角层级规范：第一个容器 `xl`，嵌套逐级 `lg → md → sm`；菜单壳 `lg`、菜单项 hover `md`。
- 间距 4px 基数；图标 `size-3/3.5/4/5/6`；控件高 `h-6/7/8/9`；flex 文本必加 `min-w-0`，嵌套滚动必加 `min-h-0`。
- shadcn 风格组件 45 个（button/badge/dropdown-menu/select/dialog/collapsible/command/tooltip/switch/tabs/toast…），`cn()` 合并类名，data-state 变体驱动。

### 3. 直接采纳的移植清单

| # | 采纳项 | 来源 | 落地方式 |
|---|---|---|---|
| 1 | 工具四态 part 模型 + 容错解码 | OpenCode | 投影层纯函数 |
| 2 | 工具注册表 + title/subtitle/args 三段式 + shimmer 运行态 | OpenCode | `timeline/tools.js` |
| 3 | reasoningHeading 四条正则 + Thinking/Thought + 耗时 | OpenCode | `timeline/reasoning.js` |
| 4 | timeline 纯函数投影 + rowKey 复用 + turn 划分 + commentary | OpenCode | `timeline/projection.js` |
| 5 | 会话今天/昨天/更早分组 + 子会话过滤 | OpenCode | `sessions.js` |
| 6 | 模型选择 Favorites 置顶 + 分组折叠 + 选中勾 | OpenCode/ZCode | `model-select.js` |
| 7 | pin/unpin auto-scroll（向上滚动即 unpin + 回到底部按钮） | OpenCode | timeline 容器 |
| 8 | 轨迹 CallCard + sticky 摘要 + IN/OUT/时长/时钟 | ZCode | `trajectory/render.js` |
| 9 | 消息 delta 去重算法 | ZCode | `trajectory/projection.js` |
| 10 | 按角色批量展开 + version + override Map（42 行） | ZCode | `trajectory/expansion.js` |
| 11 | 大输出 max-h 钳制 + 渐隐 mask + 悬浮展开 + 复制真实文本 | ZCode | 展开内容组件 |
| 12 | 会话两层运行置顶稳定排序（running 禁用 updatedAt 决胜） | ZCode | `sessions.js` |
| 13 | 行内状态优先级 error > loading > none + 相对时间 | ZCode | `sessions.js` |
| 14 | 标题溢出渐隐 mask | ZCode | CSS |
| 15 | 归档两击确认 | ZCode | 会话行操作 |
| 16 | 新建任务草稿态（发送首条消息才落库） | ZCode | 已有行为，保留 |
| 17 | color-mix 派生语义 token + 四主题 + 字号刻度 + 圆角层级 | ZCode | `ui/tokens.css` |
| 18 | data-state 属性驱动组件 + shadcn 组件集 | ZCode | `ui/components.css` + `js/ui.js` |

---

## 二、rπ 落地设计

### 1. 技术约束与决策

- 后端 axum 用 `include_str!` 内嵌静态资源，CSP `script-src 'self'; style-src 'self'`：**无构建步骤、无 CDN、无内联样式/脚本**。
- 决策：ES Modules（`<script type="module" src="/js/main.js">`，同源多文件 import）+ 纯 CSS 变量 token + data-state 属性驱动组件。
- 动态样式一律走 JS CSSOM（`el.style.xxx =`），不走 HTML style 属性（CSP 禁止属性解析，但 CSSOM 不受限）。
- markdown 自渲染：先全量转义再生成白名单标签（标题/列表/代码块/行内代码/粗斜体/引用/链接/表格），杜绝 XSS。
- 服务端已有能力直接复用：SSE 双流（progress 快照 + trace 事件）、`/api/events/page` 分页、`/api/sessions/{id}` 完整消息、`TaskRequest.session_id` 续聊。

### 2. 后端改造（rpi-core + rpi-cli）

| 改动 | 位置 | 说明 |
|---|---|---|
| `Session.archived: bool`（serde default） | session.rs | Meta/SessionSummary 同步；老 journal 兼容（缺字段 default false） |
| `SessionStore::rename` | session.rs | 取出 → 改 title → commit（delta journal 天然支持元数据变更） |
| `SessionStore::set_archived` | session.rs | 同 rename |
| `SessionStore::delete_cascade` | session.rs | 收集目标会话 + parent_id 传递闭包后代；运行中拒绝；用 `backend.replace` 把保留会话重写为全量快照 delta 序列（复用 legacy 迁移机制），重建内存 |
| `PATCH /api/sessions/{id}` `{title?, archived?}` | server.rs | 校验 title 非空 ≤200 字符 |
| `DELETE /api/sessions/{id}` | server.rs | 级联 fork 后代，409 保护运行中会话 |
| 静态资源白名单 | server.rs | `/ui/tokens.css` `/ui/base.css` `/ui/components.css` `/app.css` `/js/*.js`（含 mime 表），保留 `/docs` `/plan` 资源 |
| 删除旧 `/style.css` `/app.js` `/chinese.css` 路由 | server.rs | index.html 不再引用 |

不改动执行循环与事件 schema（轨迹内容受 `log_content` 开关控制——关闭时轨迹 CallCard 显示元数据 + omitted 提示，主时间线始终用持久化会话消息渲染完整工具调用）。

### 3. 前端结构

```
web/
  index.html            重写：顶栏 + 侧栏 + 主时间线 + 右侧检查器（轨迹/对象/MCP/技能）
  ui/tokens.css         ZCode 风格语义 token（light/dark 双主题，color-mix 派生）
  ui/base.css           reset、排版刻度、滚动条
  ui/components.css     shadcn 组件集（button/badge/card/dialog/dropdown-menu/popover/
                        select/tooltip/tabs/collapsible/input/textarea/label/switch/
                        kbd/separator/scroll-area/spinner/toast/skeleton/command）
  app.css               工作台布局 + 时间线/轨迹/侧栏页面级样式
  js/
    main.js             入口：装配 + 快捷键 + 主题
    api.js              fetch + SSE 封装（自动重连、Last-Event-ID）
    store.js            全局状态 + 订阅
    ui.js               DOM helper（h/cn）+ 组件行为（dialog/dropdown/tooltip/toast/tabs/switch/command）
    markdown.js         安全 markdown 渲染器
    sessions.js         侧栏：分组/置顶/搜索/重命名/归档/删除/状态点
    composer.js         输入区 + 运行控制（steer/follow-up/cancel）+ 技能选择
    model-select.js     Command 风格模型选择器（tier 分组 + 徽章 + Ctrl+M）
    timeline/
      projection.js     messages → rows（turn 划分/part 合并/工具配对/commentary）
      tools.js          工具渲染注册表（read_file/write_file/edit_file/list_files/glob/grep/webfetch/todo_*/load_skill/MCP/generic）
      reasoning.js      思考块（标题提取/耗时/shimmer）
      render.js         行渲染 + key 复用 + live turn + auto-scroll pin/unpin
    trajectory/
      projection.js     trace events → CallCard 模型（delta 去重）
      expansion.js      按角色批量展开 + override + version
      search.js         归一化搜索 + 定位 + 高亮
      render.js         轨迹面板渲染
```

### 4. 关键渲染规格

**主时间线**
- User：浅底卡片 + "你"徽标；Assistant：markdown 正文。
- 思考：折叠块"思考中…（shimmer）/ 已思考 · Ns + 提取标题"；正文 muted 13px、标题不放大。
- 工具卡：图标 + title + subtitle + args 徽章；running=shimmer+spinner（禁止展开）；completed=可展开输入参数与输出（max-h 钳制 + 渐隐 + 悬浮展开 + 复制）；error=红色卡折叠显示首行摘要。
- role=tool 消息按 `tool_call_id` 配对进工具卡，不单独成行。
- turn 间分隔线；流式 live turn 由 progress 快照 + tool.start/result SSE 事件实时驱动。
- auto-scroll：向上滚动即 unpin 并显示"回到底部"；选中文本不自动滚动。

**轨迹面板**
- CallCard 摘要：序号补零 + 模型名 + finishReason 徽标 + `IN · OUT · 时长 · 时钟`。
- 输入 section：delta 消息（角色标签列 SYSTEM/USER/ASSISTANT/TOOL 着色）；输出 section：思考过程 → 正文 → tool calls；独立 error 块。
- 展开系统按 6 角色批量 + 单项 override；搜索归一化匹配 + 定位 + 命中展开。
- 汇总行：N 次调用 · 总 token · 模型列表；内容被 log_content 省略时显示提示而非裸 JSON。

**会话侧栏**
- 新建任务（草稿态）→ 搜索框 → 运行中置顶区（createdAt 排序）→ 今天/昨天/更早 → 归档折叠区。
- 行：状态点（error 红 > loading spinner）+ 标题渐隐 + 模型徽标 + 相对时间 + fork 徽标 + hover 菜单（重命名/归档/删除两击确认）。

**模型选择**
- Composer 左下按钮显示当前模型/层级 → Command 面板（搜索 + 自动路由置顶 + tier 分组 + 徽章 + 上下文窗口 + 选中勾），Ctrl+M，关闭焦点回输入框。

### 5. 测试计划

- **cargo**：session.rs 新增 rename/archived/delete_cascade 单元测试（含 fork 级联、运行中拒绝、journal 重放一致性）；server.rs 新路由契约测试（PATCH/DELETE/静态资源/CSP 下无 style 属性依赖）。
- **GUI（浏览器）**：`rpi serve` + 真实模型，逐项验收：新建/发送/续聊、工具卡三态、思考折叠、轨迹面板（CallCard/搜索/展开）、会话重命名/归档/删除、模型选择、主题切换、快捷键；截图存 `reports/frontend-refactor/`。
