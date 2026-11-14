// 《Samho 系统软件完整说明报告》— 17 章 + 附录，图文 docx
const H = require("./helpers");
const S = (n) => "D:/ddcode/rpi/reports/shots/" + n;

H.resetCounters();
const body = [];

// ══ 第 1 章 产品概述 ══
body.push(H.h1("第一章 产品概述"));
body.push(H.h2("1.1 定位"));
body.push(H.p("Samho 是 Pi coding agent 的 Rust 重写工程。它把一个编码智能体所需的全部基础设施——运行时核心、模型接入、工具管线、会话持久化、调用轨迹、终端界面与图形工作台——用 Rust 从零重建，并通过本地 HTTP/SSE 服务把同一份运行时状态同时提供给 Ratatui TUI 与 Web 工作台两种界面。项目遵循「增量重写」策略：每交付一块能力都保持整仓可运行、测试全绿，未完成项按实现状态文档保留边界，绝不以文档形式许诺未实现的行为。"));
body.push(H.p("与直接使用上游 Pi 相比，Samho 的差异点在于：第一，核心是编译型 Rust 库（samho-core），可以被嵌入其他 Rust 程序；第二，所有对象（运行时、模型、工具、技能、MCP 服务、会话、任务）都实现统一的 Object 契约，由注册表按依赖拓扑管理生命周期；第三，模型接入完全由使用者配置，不内置任何供应商偏好，默认 Provider 是一个明确标注 [DEMO simulation] 的离线模拟器，保证「装好即可跑通全流程，接上真实网关即可真实推理」。"));
body.push(H.h2("1.2 设计原则"));
body.push(...H.bullets([
  "证据优先：README、架构文档与实现状态文档中的每一条能力声明都要有测试或验收脚本支撑；无法验收的能力明确写成边界",
  "单一事实源：TUI 与 Web 工作台共用同一 Rust 服务与同一数据目录，不存在两套状态",
  "配置即契约：模型、层级绑定、工具开关、预算、重试策略全部落在 runtime.toml，运行时启动时严格校验，配置错误直接拒绝启动而不是静默降级",
  "轨迹可复盘：每一次模型往返与工具调用都写入 JSONL 轨迹，带序列号与层级关联，支持导出与 SSE 实时查看",
  "本地优先：服务面向单机或可信局域网，数据全部落在本地数据目录，不依赖云服务",
]));
body.push(H.h2("1.3 读者与术语"));
body.push(H.p("本报告面向三类读者：希望部署使用的使用者（第一、九、十、十三、十五章）、希望理解内部机制的二次开发者（第二至八章、十一、十二章）与维护者（第十四至十七章）。术语约定：任务（Task）指一次异步执行的智能体作业；会话（Session）指承载任务的对话容器；轨迹（Trace）指落盘的调用事件流；层级（Tier）指模型分级路由的六个档位；工作区（Workspace）指服务进程绑定的文件系统根目录。"));

// ══ 第 2 章 总体架构 ══
body.push(H.h1("第二章 总体架构"));
body.push(H.h2("2.1 Crate 结构"));
body.push(...H.tbl({
  caption: "Cargo workspace 组成",
  headers: ["成员", "形态", "职责"],
  widths: [26, 14, 60],
  rows: [
    ["crates/samho-core", "库", "运行时核心：配置校验、数据目录与文件锁、技能发现、MCP 连接、对象注册表、模型路由、Agent 循环、会话与轨迹存储"],
    ["crates/samho-cli", "二进制 samho", "入口适配：serve / tui / run / inspect / replay / config 六个子命令，Axum HTTP + SSE 服务"],
  ],
}));
body.push(...H.tbl({
  caption: "samho 子命令",
  headers: ["子命令", "用途"],
  widths: [24, 76],
  rows: [
    ["serve", "启动本地 HTTP/SSE 服务，Web 工作台与 TUI 共用；默认绑定 0.0.0.0:7432"],
    ["tui", "连接一个已运行的 serve 实例（--url），以 Ratatui 终端界面操作同一运行时"],
    ["run", "以独立 CLI 形式执行一条任务（可指定复杂度 --complexity 与 --json 输出）"],
    ["inspect", "打开数据目录打印运行时状态"],
    ["replay", "把调用轨迹导出为 trace.jsonl（事件查看与导出，不重执行副作用）"],
    ["config", "config init 生成可编辑的完整配置模板"],
  ],
  small: true,
}));
body.push(H.p("run、inspect 与 replay 会以独占方式打开本地数据目录；服务运行期间应改用 TUI/Web 访问同一运行时，或为 CLI 指定另一数据目录——目录文件锁会拒绝两个进程同时写同一数据目录。"));
body.push(H.h2("2.2 三界面一运行时"));
body.push(H.p("serve 进程是整个系统唯一的执行体。Web 工作台（浏览器）、Ratatui TUI（终端）与 CLI 工具（run/inspect/replay）都是这个进程的客户端。这带来两个直接好处：一是界面切换不丢状态——网页上提交的任务在 TUI 里同步可见；二是调试口径统一——所有界面看到的行为都由同一份日志解释。"));
body.push(...H.figure(S("30-workbench-home-new.png"), "Web 工作台与 TUI 共用同一运行时：图为工作台首页三栏布局"));
body.push(H.h2("2.3 内嵌前端与静态资源"));
body.push(H.p("Web 工作台源码在 web/app（React 19 + TypeScript + Vite 8 + Tailwind v4 + shadcn/ui），构建产物输出到 web/dist 并以 include_str! 方式内嵌进二进制——发布形态就是一个 samho.exe，没有任何外部静态文件依赖。静态资源表 STATIC_ASSETS 共 6 项（含 SPA 入口、文档中心 docs.html、确认计划 plan.html）。服务端启用严格 CSP（style-src 与 script-src 均为 self），前端不得内联脚本或样式；修改前端后必须重新构建并提交 web/dist，再重新编译 Rust，旧 web 目录删除与新前端切换必须在同一提交里完成（CI 有产物同步门禁）。"));
body.push(H.h2("2.4 一次任务的完整时序"));
body.push(H.p("以演示实例中的一个真实任务（8f7ea2be，读取 README 并总结）为例，把从提交到落盘的每一步按时间顺序走一遍。这个任务全场最快，但链路与最复杂的任务完全一致。"));
body.push(...H.numbered([
  "提交：用户在工作台输入目标，前端 POST /api/run；运行时返回 202 与 task_id，任务进入排队状态。任务列表页立即出现一个「运行中」条目",
  "路由：运行时为任务做一次层级判定（model.route 事件）。自动分级按确定性规则计算层级，并把选择理由写进事件 payload；手动指定时同样记录一条路由事件以保持轨迹完整",
  "首轮模型调用（model.start）：上下文投影器从会话 journal 拼装上下文（含系统提示、历史与任务目标），发往绑定层级的 Provider。流式开启时输出逐 token 经 SSE 推送到前端",
  "工具调用：模型请求读取 README，返回 tool_use。管线做准入检查（工具存在、开关允许、路径合法）后执行 read_file，结果 12 毫秒返回，完整结果进入模型上下文，摘要进入轨迹（tool.start / tool.result 事件对）",
  "次轮模型调用：模型带着文件内容再次响应，给出最终中文摘要；运行时把答案写进会话 journal 并标记任务完成（task.done）",
  "落盘与推送：以上每一步都同步写入 events.jsonl（325 条之一），预算快照追加到 budget.jsonl；前端任务条目转为「已完成」，会话时间线渲染出工具卡片与答案",
], "seq-task"));

// ══ 第 3 章 运行时核心 ══
body.push(H.h1("第三章 运行时核心 samho-core"));
body.push(H.h2("3.1 AgentRuntime"));
body.push(H.p("AgentRuntime 是 samho-core 的门面类型，装配顺序为：读取并校验配置 → 打开或创建数据目录并获取文件锁 → 发现技能（SKILL.md）→ 建立 MCP 连接 → 注册内置对象（运行时、模型、工具、会话等）→ 初始化模型路由与会话/轨迹存储。任何一步失败都会中止启动并给出明确错误，不会带病运行。"));
body.push(H.h2("3.2 Object 契约与注册表"));
body.push(H.p("所有核心概念都实现统一的 Object 契约。对象信封（ObjectEnvelope）携带稳定 ID、类型、版本、依赖、能力与标签；注册表按依赖拓扑激活对象，拒绝缺失依赖与循环依赖；初始化失败时自动回滚已激活对象，销毁时按依赖逆序释放。Runtime、Model、Tool、Skill、MCP server、Session 与 Task 都纳入同一套生命周期。在此之上，运行时还提供 scope（自定义作用域）、middleware（中间件）、lease（租约）与 generation（热替换）等运行语义；远程 Facet 与生产级跨进程隔离仍按实现状态文档列为后续验收项。"));
body.push(...H.figure(S("33-objects-panel.png"), "对象注册表面板：运行时对象按类型分列（截图时 30 个，随会话与终端数量增长）"));
body.push(H.h2("3.3 数据目录"));
body.push(H.p("数据目录（data_dir）是运行时唯一的持久化位置，默认随配置指定；演示实例使用 reports/demo-data，正式实例默认为 .samho。目录内文件与用途如下。"));
body.push(...H.tbl({
  caption: "数据目录结构",
  headers: ["文件", "内容", "写入时机"],
  widths: [30, 48, 22],
  rows: [
    ["sessions.jsonl", "会话 journal：消息流（schema v2，SHA-256 链式摘要）", "每轮追加"],
    ["events.jsonl", "调用轨迹事件（325 行对应 17 任务）", "每事件追加"],
    ["budget.jsonl", "预算累计快照（tokens、usd）", "每次预算变动"],
    ["projects.json", "项目列表与 workspace 目录关联", "项目操作时"],
    ["attachments.json", "会话附属终端/浏览器标签（重启恢复用）", "附属项变化时"],
    ["runtime.lock", "数据目录文件锁", "进程持锁期间"],
  ],
  small: true,
}));
body.push(H.p("首次以新版本启动旧数据目录时，会话 journal 会自动迁移到当前 schema：迁移以字节级摘要校验保证逐行无损，失败即中止而不是写入半迁移状态。历史上曾因 JSON 反序列化丢失键序导致迁移摘要不一致，已通过显式保序修复并加了回归测试。"));

// ══ 第 4 章 Agent 循环 ══
body.push(H.h1("第四章 Agent 循环与任务语义"));
body.push(H.h2("4.1 任务生命周期"));
body.push(H.p("任务由 POST /api/run（或界面提交）创建，先进入排队状态；运行时按会话串行（每会话同一时刻只有一个活动任务）、按全局并发上限（max_concurrent_tasks）并行调度。运行中的任务经历若干轮次：每轮把当前上下文发给模型，模型返回文本则可能是中间步骤或最终答案，返回工具调用则执行后把结果回填继续下一轮。任务以完成、失败或取消三种终态结束；轮次上限（max_turns，演示为 40）与任务预算（max_task_tokens）任一触顶都会终止任务并标记失败原因。"));
body.push(...H.figure(S("32-newui-tasks-running.png"), "任务列表面板：运行中 3、已完成 4、失败 0、已取消 0；每项显示模型徽标与轮次"));
body.push(H.h2("4.2 工具管线"));
body.push(H.p("模型返回的工具调用经过四步管线：准入检查（工具是否存在、会话上下文的开关与 Skill allowed-tools 限制是否允许）→ 参数解析与校验 → 执行（带超时，shell 由 tools.shell_timeout_ms 控制）→ 结果摘要与落盘（完整结果进入模型上下文，摘要进入轨迹）。工具失败不抛出任务异常，而是作为错误结果返回给模型，让模型在下一轮自行纠正；只有模型请求层的失败才触发配置的重试与降级路由。"));
body.push(H.h2("4.3 控制指令：steering 与 follow-up"));
body.push(H.p("任务运行期间，用户在会话输入框追加的内容按语义分为两类：调整方向（steering）进入队列，在下一轮开始时生效；追问（follow-up）在当前任务结束后自动续接为新任务，共用同一会话。两类指令都记录为 task.control.queued 与 task.control.applied 轨迹事件——排队与生效各一条，方便复盘「指令何时下达、何时起作用」。运行时保证进行中的工具调用不会被控制指令打断，生效点永远在轮次边界。"));
body.push(...H.figure(S("43-session-result-full.png"), "会话时间线：用户消息、工具卡片（读取文件 · 完成）、思考摘要与最终答案依序呈现；右侧设备面板中浏览器标签的 iframe 内容在自动化截图下不参与合成，显示为空白（已知边界，见第十六章）"));
body.push(H.h2("4.4 取消、失败与重试"));
body.push(H.p("取消可以从任务列表的「停止任务」按钮或快捷键发起：运行时在最近的安全点终止轮次并写入取消事件，已完成轮次的产出保留在会话里。模型请求失败按 [retry] 配置执行指数退避重试（max_attempts、initial_backoff_ms、max_backoff_ms），重试耗尽且 reroute = true 时允许把请求降级路由到其他可用模型；这些行为都写入轨迹的路由理由字段。"));

// ══ 第 5 章 模型体系 ══
body.push(H.h1("第五章 模型体系与分级路由"));
body.push(H.h2("5.1 Provider"));
body.push(H.p("Provider 抽象目前有两类实现：Chat Completions 兼容 Provider（provider = chat_completions）覆盖绝大多数 OpenAI 兼容网关与自建代理；显式模拟 Provider 是内置的离线模拟器，输出明确标注 [DEMO simulation]，用于无模型环境下的全流程验证与测试 fixture。Provider 配置以 [[models]] 数组表达，每个条目是一个可路由的物理模型。"));
body.push(...H.tbl({
  caption: "模型条目（[[models]]）字段",
  headers: ["字段", "含义"],
  widths: [34, 66],
  rows: [
    ["id", "路由与展示用的逻辑模型 ID（如 deepseek-v4.1-flash）"],
    ["model", "请求体里的物理模型名（如 wuyou/deepseek-v4.1-flash）"],
    ["provider", "Provider 类型：chat_completions 或内置模拟器"],
    ["tier", "该模型可承载的层级"],
    ["base_url / api_key_env", "端点与读取 API key 的环境变量名（key 不写入配置文件）"],
    ["context_window / max_output_tokens", "上下文窗口与单次最大输出"],
    ["supports_tools / vision / reasoning", "能力开关，路由时参与约束过滤"],
    ["streaming", "是否走流式（SSE 逐 token 推送）"],
    ["output_token_parameter", "输出上限参数名（max_tokens 或 max_completion_tokens）"],
    ["input/output_usd_per million、latency_ms", "计费单价与延迟提示，用于展示与约束（不代表实测）"],
  ],
  small: true,
}));
body.push(...H.callout("已知坑：thinking token 计入 max_tokens", "部分网关把推理（thinking）产生的 token 计入 max_tokens 总额。若 max_output_tokens 配小了，推理模型会在「思考」阶段耗尽配额、答不出正文。对接这类网关时务必把 max_output_tokens 配大（演示配置为 32768），必要时用 output_token_parameter 指定正确的参数名。"));
body.push(H.h2("5.2 六级层级与角色路由"));
body.push(...H.tbl({
  caption: "六级模型层级",
  headers: ["层级", "界面名称", "典型用途", "约束倾向"],
  widths: [20, 16, 40, 24],
  rows: [
    ["nano", "轻量", "轻量问答、格式化、简单抽取", "小上下文、低预算"],
    ["fast", "快速", "快速任务、单文件小改动", "低延迟优先"],
    ["balanced", "均衡", "常规对话与规划", "默认档位"],
    ["reasoning", "推理", "编码专精、多步工具链", "中高上下文、工具能力"],
    ["frontier", "前沿", "深度推理、难问题攻坚", "大上下文、强推理"],
    ["specialist", "专家", "复杂架构、超大仓库任务", "最大预算与窗口"],
  ],
}));
body.push(H.p("提交任务时可以三选一：交给自动分级（运行时按确定性规则判定层级，理由写入 model.route 事件）、手动锁层级、或手动指定具体模型。层级与物理模型的映射由模型角色表（model_roles.json）维护；某个层级没有绑定模型时，界面会在层级列表明确标注「暂无已配置模型」而不是静默失败。分级标签是运维方提供的语义标注，不代表已测模型性能——路由 payload 里也带着这句免责声明。"));
body.push(...H.figure(S("44-model-picker.png"), "模型选择器：自动路由（当前选中）与六层级列表，未配置层级明确标注；右侧设备面板浏览器标签的 iframe 内容在自动化截图下不参与合成，显示为空白（已知边界，见第十六章）"));
body.push(...H.figure(S("36-models-panel.png"), "模型面板：演示实例把 reasoning 层级绑定到 deepseek-v4.1-flash"));
body.push(H.h2("5.3 上下文投影与流式"));
body.push(H.p("每轮发送给模型的上下文由会话 journal 投影生成：按 lineage（消息血缘）裁剪 fork 出的分支，按层级上下文窗口约束截断，并注入当前任务的工具结果。流式开启（streaming = true）时，模型输出经 SSE 实时推送到工作台，前端逐 token 渲染；轨迹仍按完整轮次记录。Provider streaming 已由 15 项专项测试覆盖（含真实 HTTP fixture 验证模型→工具→模型循环）。"));
body.push(H.h2("5.4 路由决策示例"));
body.push(H.p("下面是演示实例 17 个任务中真实记录的一条 model.route 事件 payload（节选自 events.jsonl）。分类器是可配置的确定性规则集，不涉及任何模型推理；payload 里还带着一句免责声明——层级标签由运维方提供，不构成对模型能力的度量。"));
body.push(...H.codeBlock([
  "{",
  "  \"kind\": \"model.route\",",
  "  \"object_id\": \"router\",",
  "  \"payload\": {",
  "    \"classifier\": \"configurable deterministic rules;",
  "                 tier labels are operator supplied,",
  "                 not intelligence measurements\",",
  "    \"selected_tier\": \"reasoning\",",
  "    \"model\": \"deepseek-v4.1-flash\"",
  "  }",
  "}",
]));
body.push(H.p("字段含义：classifier 标识判定器类型；selected_tier 是判定的目标层级；model 是层级绑定后的物理模型。手动指定层级或模型时，payload 会改记手动来源，保证复盘时能区分「自动判定」与「人工指定」。"));

// ══ 第 6 章 工具体系 ══
body.push(H.h1("第六章 工具体系"));
body.push(H.h2("6.1 内置工具"));
body.push(...H.tbl({
  caption: "九个内置工具",
  headers: ["工具", "用途", "默认", "备注"],
  widths: [18, 44, 12, 26],
  rows: [
    ["read_file", "读取文件（可指定行区间）", "开", "拒绝符号链接与越界路径"],
    ["write_file", "写文件", "开", "限制在 workspace 内"],
    ["edit_file", "精确替换编辑", "开", "要求唯一匹配"],
    ["list_files", "列目录", "开", "—"],
    ["glob", "模式匹配文件", "开", "—"],
    ["grep", "内容搜索", "开", "—"],
    ["shell", "执行命令（Windows 走 PowerShell）", "开", "非沙箱，见第十五章"],
    ["webfetch", "抓取网页转文本", "开", "—"],
    ["todo", "任务清单维护", "开", "—"],
  ],
  small: true,
}));
body.push(H.p("write 与 shell 默认开启是有意的设计取舍：编码智能体的核心价值在于真实改文件与跑命令。代价是安全责任转移给使用者——处理不可信内容（外部仓库、网页抓取结果、MCP 数据）时，应在配置 [tools] 段显式关闭 shell，必要时同时关闭 write。"));
body.push(H.h2("6.2 准入控制与扩展"));
body.push(H.p("工具准入有三道闸：一是会话级开关（输入框下方的工具菜单）；二是 Skill 的 allowed-tools 声明——当任务命中某技能时，技能未放行的工具在技能作用范围内不可用；三是路径边界——文件类工具一律拒绝 workspace 之外的路径与符号链接目标。扩展方面，除 MCP 服务器带来的外部工具外，还支持注册自定义 Rust Tool（实现工具契约后注册进运行时，与内置工具同权参与路由与轨迹）。"));

// ══ 第 7 章 会话与持久化 ══
body.push(H.h1("第七章 会话与持久化"));
body.push(H.h2("7.1 会话 journal"));
body.push(H.p("每个会话对应 sessions.jsonl 中的一段 JSONL journal，当前 schema 为 v2：每条消息记录角色、内容与结构化元数据，行间以 SHA-256 链式摘要衔接，任何一行被篡改或丢失都会在恢复时被发现。会话支持 fork（从任意轮次分支出新会话，lineage 记录血缘）与重命名、归档、删除等管理操作。"));
body.push(H.h2("7.2 项目制工作区"));
body.push(H.p("会话按「项目」组织：一个项目可关联多个 workspace 目录（entry 为主目录，新会话的终端与文件面板默认从这里起步），项目下展开多个会话。存量会话首次启动时自动归入「默认项目」（幂等迁移）。每个会话可挂附属终端与浏览器标签；浏览器标签持久化到 attachments.json，服务重启后按会话恢复。"));
body.push(H.h2("7.3 重启恢复"));
body.push(H.p("serve 重启后从数据目录完整重建状态：会话与消息从 journal 恢复（校验摘要链）、预算从 budget.jsonl 恢复、附属终端/浏览器从 attachments.json 恢复、项目从 projects.json 恢复。恢复过程全部幂等——重复启动不产生重复对象；恢复失败（如摘要校验不过）会明确报错并拒绝继续，防止在损坏数据上继续写入。"));
body.push(H.h2("7.4 会话操作与时间线元素"));
body.push(H.p("会话条目菜单提供六种管理操作：打开、移动到项目、重命名、分支（fork，从当前对话分支出新会话并记录血缘）、归档与删除。归档会话从默认列表隐藏但数据保留；fork 出的会话在轨迹与界面上都携带 lineage 信息，续接行为只影响新分支。"));
body.push(...H.tbl({
  caption: "会话时间线元素",
  headers: ["元素", "含义", "交互"],
  widths: [28, 44, 28],
  rows: [
    ["用户消息", "提交的任务目标与追加的控制指令", "—"],
    ["工具卡片", "一次工具调用：名称、参数摘要、延迟与完成/失败标记", "展开查看参数与结果摘要"],
    ["思考摘要", "模型的推理过程摘要（可折叠）", "展开查看"],
    ["流式答案", "模型最终回复，Markdown 全量渲染（表格/清单/高亮/公式）", "渲染视图"],
    ["内容开关徽标", "log_content = false 时显示「内容开关关闭」", "提示轨迹已脱敏"],
    ["轮次分隔", "第 N 轮标记，区分任务的各轮次", "—"],
  ],
  small: true,
}));

// ══ 第 8 章 HTTP API 与 SSE ══
body.push(H.h1("第八章 HTTP API 与 SSE"));
body.push(H.h2("8.1 核心端点"));
body.push(...H.tbl({
  caption: "HTTP API（本表为报告编写期间实测验证过的端点）",
  headers: ["方法与路径", "用途"],
  widths: [42, 58],
  rows: [
    ["POST /api/run", "提交任务，返回 202 与 task_id；请求体含 prompt 与可选模型/层级参数"],
    ["GET /api/tasks", "任务列表（含状态、结果摘要、错误信息）"],
    ["GET /api/sessions", "会话列表（标题、状态、消息数、模型、时间戳）"],
    ["GET /api/models", "已配置模型清单"],
    ["GET /api/model-roles", "层级绑定关系（model_roles.json 视图）"],
    ["GET /api/events", "轨迹事件查询（分页）"],
    ["POST /api/preview", "提交 HTML 到沙箱预览存储"],
    ["GET /api/preview/fs/{path}", "以沙箱 CSP 提供 workspace 文件；目录自动 307 到 index.html；单文件上限 8 MiB"],
    ["GET /api/proxy?url=", "代理抓取外部网页并注入 base 标签（浏览器面板用）"],
    ["POST /api/term/{id}/input", "向指定 PTY 终端写入输入（id 为完整终端 ID）"],
    ["GET /api/file/raw", "原始文件读取（供文件面板下载）"],
  ],
  small: true,
}));
body.push(H.p("此外运行时还暴露任务取消、控制指令（steering/follow-up）、对象/技能/MCP 状态查询、会话管理与静态资源等端点，由工作台对应按钮调用；完整清单以 docs/中文使用说明.md 与源码 server.rs 为准。服务拒绝跨源请求（CORS），仅供同源页面与本机工具访问。"));
body.push(H.h2("8.2 SSE 事件流"));
body.push(H.p("工作台通过 SSE 订阅运行时事件：任务状态变化、模型流式输出、工具调用起止与轨迹事件实时推送。事件与 events.jsonl 落盘内容同源，因此「界面看到什么、日志里就有什么」。SSE 断线后前端自动重连并按序列号补齐缺口，不依赖刷新页面。"));
body.push(H.h2("8.3 事件示例与分页"));
body.push(H.p("GET /api/events 支持按序列号分页拉取轨迹事件。事件条目带 sequence（全服务单调递增）、时间戳、trace/session/span 关联、kind 与 payload。一条工具结果事件的完整结构如下（演示实例 log_content = false，正文被脱敏为占位符）。"));
body.push(...H.codeBlock([
  "{",
  "  \"sequence\": 201,",
  "  \"timestamp_ms\": 1794633099831,",
  "  \"trace_id\": \"812cdcb8-33e9-...\",",
  "  \"session_id\": \"a8e7a64c-5316-...\",",
  "  \"span_id\": \"fd3e9877-...:tool:call_02_ET_...\",",
  "  \"parent_span_id\": \"fd3e9877-...\",",
  "  \"kind\": \"tool.result\",",
  "  \"object_id\": \"tool:read_file\",",
  "  \"payload\": {",
  "    \"call_id\": \"call_02_ET_L25RT0fFz7yYvz1N5oku3360\",",
  "    \"is_error\": false,",
  "    \"latency_ms\": 6,",
  "    \"name\": \"read_file\",",
  "    \"result\": \"[content omitted; enable log_content ...]\"",
  "  }",
  "}",
]));
body.push(H.p("前端 SSE 订阅的正是同一批事件的实时流：kind 与落盘事件一一对应，sequence 用于断线重连后补齐缺口。需要完整正文复盘时，在配置开启日志内容开关后重新运行任务即可。"));
body.push(H.h2("8.4 调用示例"));
body.push(...H.codeBlock([
  "# 提交任务（请求体需 UTF-8 编码；Windows 控制台建议 --data-binary @file.json）",
  "curl -s -X POST http://127.0.0.1:7433/api/run \\",
  "     -H \"Content-Type: application/json\" \\",
  "     --data-binary @payload.json",
  "# => {\"task_id\":\"...\",\"session_id\":\"...\"} (HTTP 202)",
  "",
  "curl -s http://127.0.0.1:7433/api/tasks   # 查询任务状态",
  "curl -s http://127.0.0.1:7433/api/events  # 分页拉取轨迹",
]));

// ══ 第 9 章 Web 工作台 ══
body.push(H.h1("第九章 Web 工作台"));
body.push(H.h2("9.1 布局与操作"));
body.push(H.p("工作台为三栏布局：左栏是项目与会话管理（项目分组、搜索、Alt+N 新建任务）；中栏是当前会话的时间线（用户消息、工具卡片、思考摘要、流式答案）与输入框；右栏是可折叠设备面板。输入框下方有三个开关：自动分级、技能、自动路由，分别控制层级判定方式、技能发现与模型选择策略。"));
body.push(H.h2("9.2 会话管理"));
body.push(H.p("每个会话条目的菜单提供打开、移动到项目、重命名、分支（fork）、归档与删除操作。搜索框按标题过滤会话；项目条目可展开为会话列表。会话时间线里的工具卡片可展开查看参数与结果摘要，错误工具卡片以醒目样式标记。"));
body.push(...H.figure(S("37-files-readme.png"), "文件面板：目录树 + Markdown 渲染视图，右上角提供源码编辑与保存"));
body.push(...H.figure(S("38-git-panel.png"), "Git 面板：分支切换、暂存与变更列表（有超时与输出上限保护）"));
body.push(...H.figure(S("39-terminal-panel.png"), "终端面板：多 PTY 标签，默认从项目 entry 目录启动"));
body.push(...H.figure(S("40-browser-panel.png"), "浏览器面板：经服务端代理（/api/proxy）加载网页，支持多标签；图中为未导航时的空态（自动化截图下 iframe 内容不参与合成，见第十六章已知边界）"));
body.push(H.h2("9.3 检查面板"));
body.push(...H.figure(S("33-objects-panel.png"), "对象注册表：运行时对象清单"));
body.push(...H.figure(S("34-skills-panel.png"), "技能面板：SKILL.md 校验与发现结果"));
body.push(...H.figure(S("35-mcp-panel.png"), "MCP 面板：服务器连接与工具状态"));
body.push(...H.figure(S("36-models-panel.png"), "模型面板：层级绑定与模型条目"));
body.push(H.h2("9.4 设置与主题"));
body.push(...H.figure(S("41-settings-dialog.png"), "界面与快捷键设置（保存到当前工作区 ui.json）"));
body.push(H.p("设置对话框管理主题（深色/亮色/跟随系统）与三个快捷键：发送任务（默认 Ctrl+Enter）、新建会话（Alt+N）、取消任务（Ctrl+K）。顶栏的太阳/月亮按钮一键切换主题，全部组件同步换肤。"));
body.push(...H.figure(S("42-light-theme.png"), "亮色主题下的工作台全貌"));
body.push(H.h2("9.5 沙箱预览"));
body.push(H.p("对话中的 HTML 代码块与 workspace 网页可在隔离沙箱运行：/api/preview 存储提交的 HTML，/api/preview/fs/{path} 以严格 CSP 提供 workspace 文件，预览页运行在受限源（Origin 为 null），无法访问服务 API 与用户凭据。目录请求自动 307 到 index.html，单文件上限 8 MiB。"));
body.push(...H.figure(S("24-preview-hello-samho.png"), "沙箱预览示例：Agent 生成的 hello-samho.html"));
body.push(H.h2("9.6 文档中心与确认计划"));
body.push(...H.figure(S("45-docs-center.png"), "文档中心：12 份中文文档，支持标题/正文/配置项搜索"));
body.push(...H.figure(S("46-plan-page.png"), "确认计划页：勾选生成计划文本，不自动执行"));
body.push(H.p("确认计划页把「下一阶段做什么」变成显式契约：用户勾选目标、填写项目目标与执行边界，页面生成 UTF-8 文本文件；把该文本回传到对话后，Agent 才按确认范围实现。网页本身不调用 Agent 接口、不提交模型请求。"));
body.push(H.h2("9.7 键盘快捷键总表"));
body.push(...H.tbl({
  caption: "工作台快捷键（默认值，可在界面设置中修改）",
  headers: ["快捷键", "动作", "生效范围"],
  widths: [28, 40, 32],
  rows: [
    ["Alt+N", "新建任务（新会话草稿）", "全局"],
    ["Ctrl+Enter", "发送任务", "输入框"],
    ["Shift+Enter", "输入框换行", "输入框"],
    ["Ctrl+K", "取消当前任务", "运行中会话"],
    ["Ctrl+M", "模型选择器（自动路由/层级/模型）", "全局"],
    ["Esc", "关闭弹层（模型选择器、设置对话框等）", "弹层"],
  ],
}));

// ══ 第 10 章 TUI ══
body.push(H.h1("第十章 Ratatui TUI"));
body.push(H.p("TUI 是工作台的终端形态：cargo run -p samho -- tui --url http://127.0.0.1:7432 连接到运行中的 serve 实例，展示会话列表、任务时间线与轨迹面板，支持提交任务、追加控制指令与取消。因为共用同一服务与数据目录，网页上发生的一切在 TUI 中同步可见，反之亦然。TUI 适合 SSH 远程场景或不想开浏览器的快速操作；键盘交互细节以 docs/中文使用说明.md 为准。"));
body.push(H.h2("10.1 面板构成"));
body.push(H.p("TUI 界面由三个信息面板与一个输入区组成，各自对应运行时的一个数据源："));
body.push(...H.tbl({
  caption: "TUI 面板与数据来源",
  headers: ["面板", "展示内容", "数据来源"],
  widths: [22, 44, 34],
  rows: [
    ["会话列表", "当前 data_dir 中的全部会话，含标题与状态", "GET /api/sessions 同源数据"],
    ["任务时间线", "选中会话的任务及其进行状态", "任务队列与 task.* 事件"],
    ["轨迹面板", "选中任务的模型调用与工具调用事件流", "events.jsonl 轨迹记录"],
    ["输入区", "提交新任务、对运行中任务追加控制指令或取消", "POST /api/run 与控制接口"],
  ],
  small: true,
}));
body.push(H.h2("10.2 与 Web 工作台的分工"));
body.push(H.p("两个前端共享同一个 serve 实例，因此不存在“TUI 会话”与“网页会话”的分裂：任一侧创建的会话与任务在另一侧立即可见，控制指令（追加要求、取消）同样互通。分工按场景选择——Web 工作台提供文件树、Git、终端、浏览器等设备面板与富文本渲染，适合本机交互式开发；TUI 无需浏览器、占用低，适合 SSH 远程维护或在服务器上顺带查看任务进度。键盘快捷键与面板焦点切换细节以中文使用说明为准，本文不逐键罗列。"));

// ══ 第 11 章 MCP 支持 ══
body.push(H.h1("第十一章 MCP 支持"));
body.push(H.h2("11.1 通道与能力"));
body.push(H.p("MCP 集成基于官方 Rust SDK rmcp 3.5.1，支持 stdio 与 Streamable HTTP 两种通道：stdio 模式由运行时拉起子进程并管理其生命周期，HTTP 模式直连远程端点。编解码、JSON-RPC 请求 ID、初始化协商与取消通知均由 SDK 完成。"));
body.push(...H.tbl({
  caption: "MCP 能力矩阵",
  headers: ["能力", "状态", "说明"],
  widths: [30, 14, 56],
  rows: [
    ["工具（tools）", "已验证", "列表、调用、超时与取消；工具进入统一管线并纳入准入控制"],
    ["资源（resources）", "已验证", "资源读取与列表"],
    ["模板（resource templates）", "已验证", "参数化资源模板"],
    ["prompts", "已验证", "提示词模板获取与使用"],
    ["分页（pagination）", "已验证", "游标分页列表"],
    ["超时", "已验证", "可配置超时，超时视为工具失败返回模型"],
    ["取消", "已验证", "进行中的请求可取消"],
    ["子进程回收", "已验证", "stdio 子进程在服务器注销/退出时回收"],
    ["完整官方 conformance", "未验收", "列于实现状态文档的边界，不宣称兼容"],
  ],
  small: true,
}));
body.push(H.p("MCP 集成由 5 项核心测试与 4 项 optional fixture 测试覆盖（真实子进程 fixture，无需网络）；浏览器验收阶段未配置实际 MCP 服务器，协议行为以 Rust 集成测试为准。"));
body.push(H.h2("11.2 配置与运行方式"));
body.push(H.p("MCP 服务器在配置文件中声明，运行时启动时逐个建立连接。stdio 通道给出启动命令（运行时负责拉起子进程、建立 JSON-RPC 通道并在退出时回收）；Streamable HTTP 通道给出端点 URL。握手成功后，服务器声明的工具注入统一工具管线，与内置工具同权参与准入控制与轨迹记录；服务器注销或进程退出时，其工具随之失效并从注册表移除。"));
body.push(...H.bullets([
  "连接失败不阻断启动：某个 MCP 服务器不可达时，运行时记录错误并继续装配其余对象，界面 MCP 面板显示该服务器为未连接状态",
  "超时与取消可配置：慢服务器不会拖垮任务轮次；超时的调用按工具失败返回给模型自行处理",
  "能力按需启用：工具、资源、模板与 prompts 分别列出，界面按类型展示，调用记录全部进入轨迹",
]));

// ══ 第 12 章 Skill 系统 ══
body.push(H.h1("第十二章 Skill 系统"));
body.push(H.h2("12.1 声明与发现"));
body.push(H.p("技能以 SKILL.md 文件声明：frontmatter 携带名称、描述与 allowed-tools 等字段，运行时按确定性顺序扫描发现（同名冲突会给出诊断而不是随机选择）。frontmatter 校验失败的技能被拒绝加载并报告原因。正文（Markdown 指令）按需加载：任务命中技能时才读取全文注入上下文，避免无关技能占用窗口。"));
body.push(H.h2("12.2 调用与准入"));
body.push(H.p("技能支持两种触发：自动发现（任务描述与技能描述匹配）与显式调用（用户以技能名直接调用）。技能的 allowed-tools 参与工具准入：在技能作用范围内，未放行的工具不可用，防止技能越权调用危险工具。技能策略由 5 项专项测试覆盖（校验、发现、冲突、按需加载与准入）。"));
body.push(H.h2("12.3 SKILL.md 声明字段"));
body.push(...H.tbl({
  caption: "SKILL.md frontmatter 字段",
  headers: ["字段", "必填", "作用"],
  widths: [26, 12, 62],
  rows: [
    ["name", "是", "技能标识；发现与显式调用均按此名匹配，同名冲突会在诊断中报告"],
    ["description", "是", "技能描述；自动发现依据这段文字与任务描述匹配"],
    ["allowed-tools", "否", "技能作用范围内放行的工具清单；未声明放行的工具不可用"],
    ["正文（Markdown）", "—", "技能指令正文，命中技能时才按需加载，避免无关技能占用上下文窗口"],
  ],
}));
body.push(H.p("frontmatter 校验失败（缺必填字段、类型不符）的技能会被拒绝加载并在技能面板标注原因；发现顺序是确定性的，不依赖文件系统遍历的随机顺序，同名技能的冲突有明确诊断而不是静默选择。"));

// ══ 第 13 章 配置参考 ══
body.push(H.h1("第十三章 配置参考"));
body.push(H.h2("13.1 runtime.toml 主字段"));
body.push(...H.tbl({
  caption: "runtime.toml 常用字段（演示实例取值见括号）",
  headers: ["字段", "含义", "演示取值"],
  widths: [32, 46, 22],
  rows: [
    ["display_name", "界面显示名", "Samho"],
    ["workspace", "服务绑定的文件系统根目录", "D:/ddcode/rpi"],
    ["data_dir", "持久化数据目录", "reports/demo-data"],
    ["http_bind", "HTTP 监听地址", "127.0.0.1:7433"],
    ["max_turns", "单任务轮次上限", "40"],
    ["max_concurrent_tasks", "全局并发任务上限", "4"],
    ["request_timeout_ms", "单次模型请求超时", "600000"],
    ["streaming", "模型输出流式开关", "true"],
    ["[tools] shell_timeout_ms", "shell 工具超时", "120000"],
    ["[retry] max_attempts / initial_backoff_ms / max_backoff_ms", "重试次数与指数退避区间", "3 / 500 / 8000"],
    ["[retry] reroute", "重试耗尽后是否允许降级路由", "true"],
    ["[budget] max_task_tokens", "单任务 token 预算上限", "4000000"],
  ],
  small: true,
}));
body.push(H.h2("13.2 生成与切换配置"));
body.push(...H.codeBlock([
  "cargo run -p samho -- config init samho.toml   # 生成可编辑完整模板",
  "cargo run -p samho -- --config samho.toml serve",
]));
body.push(H.p("config init 默认接入本机 OpenAI 兼容网关 http://127.0.0.1:3425/v1（模型 wuyou/deepseek-v4.1-flash），需要先设置环境变量 SAMHO_API_KEY。API key 一律通过 api_key_env 指定的环境变量间接读取，不落盘、不进日志（轨迹脱敏会遮蔽敏感串）。"));
body.push(H.h2("13.3 环境变量"));
body.push(...H.tbl({
  caption: "环境变量",
  headers: ["变量", "用途"],
  widths: [34, 66],
  rows: [
    ["SAMHO_API_KEY", "演示网关与按 api_key_env 引用的模型 key（示例部署取值 magpie）"],
    ["自定义任意变量", "任何 [[models]].api_key_env 指向的变量名均可，服务启动时读取"],
  ],
}));

// ══ 第 14 章 测试与 CI ══
body.push(H.h1("第十四章 测试、验收与 CI"));
body.push(H.h2("14.1 测试分布"));
body.push(...H.tbl({
  caption: "Rust workspace 测试分布（共 162 项，另有 0 个 doc test）",
  headers: ["测试组", "数量", "覆盖内容"],
  widths: [30, 12, 58],
  rows: [
    ["CLI", "50", "子命令、参数解析、退出码"],
    ["core 单元", "34", "运行时核心模块单元测试"],
    ["Agent", "6", "循环语义（轮次、取消、失败）"],
    ["契约测试", "15", "API/存储契约"],
    ["Facet", "6", "Facet 握手"],
    ["MCP", "5", "stdio/HTTP 核心（真实子进程 fixture）"],
    ["MCP optional", "4", "optional fixture 场景"],
    ["OAuth", "3", "PKCE 流程"],
    ["Provider streaming", "15", "真实 HTTP fixture 的流式与工具循环"],
    ["runtime semantics", "10", "scope/lease/generation 语义"],
    ["Skill policy", "5", "校验、发现、冲突、准入"],
    ["storage lifecycle", "9", "journal 生命周期与迁移"],
  ],
  small: true,
}));
body.push(H.p("Node 侧 21 项测试覆盖中文页面（test-chinese-pages.mjs）与工作台契约（test-workbench-contracts.mjs），另有 check-web-esm.mjs 校验前端 ESM 规范。测试全部使用本地 fixture，无需供应商凭证。"));
body.push(H.h2("14.2 验收脚本"));
body.push(...H.codeBlock([
  "powershell -NoProfile -File scripts/smoke-web.ps1 -Force        # 浏览器验收",
  "powershell -NoProfile -File scripts/benchmark-runtime.ps1 -Force # 本地基准",
  "python scripts/smoke_runtime.py --force                          # 运行时冒烟",
]));
body.push(H.p("浏览器验收覆盖自动分级、同会话续接、手动 reasoning 层级、固定模型、重启恢复与 trace/Object 展示，并检查桌面与 560×900 窄屏无水平溢出、控制台无 error/warning。验收脚本默认拒绝覆盖既有报告，重跑需显式 -Force / --force。本地基准（benchmark-runtime.ps1）报告的任务调度 p50 约 131 毫秒（2026-11 记录）；该数字只用于本机回归对比，不可跨机、跨版本直接比较。"));
body.push(H.h2("14.3 CI 门禁"));
body.push(...H.bullets([
  "cargo fmt --check / cargo test --workspace / cargo clippy -D warnings 三道静态与行为门禁",
  "Rust 1.88 MSRV 检查（低于 MSRV 的依赖变更会被拦截）",
  "RustSec 依赖公告扫描（cargo audit）",
  "第三方 GitHub Action 固定 commit SHA，由 Dependabot 维护升级",
  "web 产物同步门禁：修改 web/app 后未同步构建 web/dist 的提交会被 CI 拒绝",
]));
body.push(H.h2("14.4 运行测试子集"));
body.push(H.p("全量测试约数分钟；日常迭代可按名称过滤运行单个子集。常用方式如下（均在仓库根目录执行）。"));
body.push(...H.codeBlock([
  "cargo test -p samho-core            # 只跑核心库测试",
  "cargo test -p samho                 # 只跑 CLI 测试",
  "cargo test --workspace mcp          # 按关键字过滤测试名",
  "cargo test --workspace -- --nocapture  # 显示测试输出",
  "npm --prefix web/app run build      # 前端改动后先构建",
  "node --test scripts/test-chinese-pages.mjs",
]));
body.push(H.p("约定：新增 Rust 代码需要同步补充对应测试组的用例；修改前端必须重新构建 web/dist 并一并提交（CI 门禁检查产物一致性）；测试一律使用本地 fixture，不允许依赖外部网络与供应商凭证。"));

// ══ 第 15 章 安全边界 ══
body.push(H.h1("第十五章 安全边界"));
body.push(H.h2("15.1 网络与访问"));
body.push(...H.callout("无身份认证，不是沙箱", "serve 默认绑定 0.0.0.0:7432 并拒绝跨源请求，但没有身份认证，也不是 OS 沙箱。它只应运行在可信局域网；只在本机使用时用 --bind 127.0.0.1:7432 收回。启动日志与页面顶栏会显示可用的局域网地址，方便按需收紧。"));
body.push(H.h2("15.2 shell 与文件系统"));
body.push(...H.bullets([
  "默认工具策略包含 shell 与 write：Agent 可以以服务进程用户的权限执行任意命令、写 workspace 内文件",
  "shell 不是沙箱：命令可访问 workspace 之外的系统资源；处理不可信内容时在 [tools] 显式关闭",
  "文件面板与内置文件工具拒绝符号链接目标与越界路径",
  "Git 面板操作有超时与输出上限，防止大仓库操作拖垮界面",
]));
body.push(H.h2("15.3 数据与脱敏"));
body.push(...H.bullets([
  "API key 只经环境变量读取，配置文件与日志不落明文；轨迹脱敏遮蔽敏感串",
  "log_content = false 仅省略轨迹正文；会话恢复仍保存经脱敏的完整消息",
  "沙箱预览运行在受限源（CSP sandbox、Origin null），与服务源隔离",
]));
body.push(H.h2("15.4 最小安全基线清单"));
body.push(H.p("部署到真实环境前，建议至少完成以下清单。全部条目均为配置或使用习惯层面的动作，无需修改代码。"));
body.push(...H.numbered([
  "只在可信局域网暴露服务；单机使用时一律 --bind 127.0.0.1:7432",
  "处理不可信内容前在 [tools] 关闭 shell（必要时同时关闭 write）",
  "API key 只放环境变量，不写入任何会进入版本库的文件",
  "为正式实例与演示/实验使用不同的 data_dir 与端口",
  "开启 log_content 前确认磁盘与备份策略；关闭状态下轨迹仍可用于结构与行为复盘",
  "定期验收：升级后先跑 smoke_runtime.py 与 cargo test --workspace，再切流量",
], "seq-security"));

// ══ 第 16 章 已知问题与边界 ══
body.push(H.h1("第十六章 已知问题与边界"));
body.push(H.p("以下事项按实现状态文档如实列出，构成本报告与产品宣传之间的防火墙。"));
body.push(...H.bullets([
  "完整 MCP 官方 conformance 尚未验收：当前能力以第十一章矩阵为准",
  "真实供应商模型质量未验收：演示与测试使用本地网关与模拟器，模型输出质量不能由本地结果推断",
  "跨平台实机、24 小时 soak 测试未完成：Linux/macOS 长时运行稳定性待验收",
  "Pi/Rust 同机性能对照未完成：不宣称相对上游的速度倍率",
  "远程 Facet 与生产级跨进程隔离列为后续验收项",
  "trace replay 是事件查看/导出，不会重执行外部副作用",
  "分级标签与延迟提示是运维方标注，不代表已测性能",
]));
body.push(H.p("已知工程坑位（修复后留档）：旧 journal 迁移需字节级摘要校验（键序问题已修复）；PowerShell 环境下 shell 包装器对 ${} 的展开可能导致命令失败（模型可自愈，包装器转义待改进）；修改 web/app 后必须与 web/dist 构建同步提交，否则 CI 门禁拒绝。"));

// ══ 第 17 章 性能 ══
body.push(H.h1("第十七章 性能与基准"));
body.push(H.p("本地基准脚本 scripts/benchmark-runtime.ps1 输出任务调度与工具执行的分位数指标：2026 年 11 月记录的调度 p50 约 131 毫秒。同机演示（另见《多任务运行报告与分析》）显示：文件类工具 p50 不超过 10 毫秒、shell 受进程启动影响在数百毫秒级、并发 4 任务对单任务耗时无显著影响。"));
body.push(...H.bullets([
  "性能对比必须同 commit、同构建条件、同工作负载；不同版本二进制体积不可直接互比",
  "性能目标必须以同机、同输入、同语义的 Pi 对照实验校准，当前该实验未完成",
  "基准数字只用于本机回归，不用于对外宣称",
]));

// ══ 第 18 章 故障排查 FAQ ══
body.push(H.h1("第十八章 故障排查 FAQ"));
body.push(H.p("以下问答覆盖部署与日常使用中最常见的十类问题，全部来自本工程实际踩过的坑或验收记录，处理步骤均经过验证。"));
body.push(...H.tbl({
  caption: "常见问题与处理",
  headers: ["现象", "原因", "处理"],
  widths: [30, 32, 38],
  rows: [
    ["serve 启动即报错退出", "配置校验失败（字段缺失、层级未绑定、路径不存在）", "按报错字段修正 runtime.toml；config init 生成模板对照"],
    ["提示数据目录被占用", "文件锁：另一进程（如 run/inspect/replay）正持锁", "先停 CLI 侧进程或另配 data_dir；同一目录只允许一个写入者"],
    ["模型调用 401/403", "api_key_env 指向的环境变量未设置", "先设置对应环境变量再启动；key 不写进配置文件"],
    ["推理模型答不出正文", "thinking token 计入 max_tokens，输出配额被思考耗尽", "调大 max_output_tokens；核对 output_token_parameter 参数名"],
    ["任务一直排队不跑", "已达 max_concurrent_tasks 上限，或同会话已有活动任务", "等待或调大并发上限；同会话先取消/等待当前任务"],
    ["界面样式/脚本空白", "严格 CSP 下出现内联代码", "前端不得内联 script/style；走 web/dist 构建产物"],
    ["文件工具报越界", "目标在 workspace 之外或为符号链接", "把目标放进 workspace；不使用符号链接目标"],
    ["预览页拒绝执行", "沙箱 CSP 拦截（受限源、Origin 为 null）", "预览只作隔离展示；完整页面测试放本地静态服务器"],
    ["重启后会话缺失/校验失败", "journal 摘要链损坏", "恢复备份；不要手改 sessions.jsonl"],
    ["重跑验收脚本被拒", "脚本默认拒绝覆盖既有报告", "加 -Force（PowerShell）或 --force（Python）"],
  ],
  small: true,
}));
body.push(H.p("补充两条工程经验。其一，做界面演示或截图前，先核对 target/release 的构建时间与 git HEAD 的提交时间：二进制落后于前端提交时会拍到旧界面、新接口返回 404，这是演示环境最隐蔽的坑。其二，演示与验证使用独立 data_dir 与独立端口（如 7433），与正式实例隔离，避免污染真实数据。"));

// ══ 附录 ══
body.push(H.h1("附录 A 术语表"));
body.push(...H.tbl({
  caption: "术语表",
  headers: ["术语", "定义"],
  widths: [26, 74],
  rows: [
    ["Object / ObjectEnvelope", "统一对象契约与信封：稳定 ID、类型、版本、依赖、能力、标签"],
    ["Agent 循环", "模型往返与工具执行交替进行的任务执行过程"],
    ["层级（Tier）", "六级模型分档：nano/fast/balanced/reasoning/frontier/specialist"],
    ["RoleRouter", "层级到物理模型的绑定与路由器（model_roles.json）"],
    ["steering", "任务运行中追加的调整方向指令，下一轮生效"],
    ["follow-up", "当前任务结束后自动续接的追问指令"],
    ["journal", "会话消息的 JSONL 持久化（schema v2，SHA-256 链）"],
    ["trace / span", "调用轨迹与其层级单元（trace/session/span/parent 关联）"],
    ["Facet", "对象的远程访问面（当前为本地实现，远程列后续）"],
    ["lease / generation", "对象租约与热替换代次"],
    ["SSE", "Server-Sent Events，工作台实时事件通道"],
    ["沙箱预览", "受限源（CSP sandbox）中的 HTML/网页预览"],
    ["DEMO simulation", "内置离线模拟 Provider 的输出标注"],
    ["数据目录", "journal/轨迹/预算/项目/附属项的唯一持久化位置"],
    ["文件锁", "数据目录的进程级互斥锁"],
  ],
  small: true,
}));
body.push(H.h1("附录 B 仓库文件索引"));
body.push(...H.tbl({
  caption: "关键文件与目录",
  headers: ["路径", "说明"],
  widths: [40, 60],
  rows: [
    ["agent.md", "权威工程参考（本报告的事实源）"],
    ["README.md", "使用说明与验证门禁"],
    ["docs/architecture.md", "当前架构"],
    ["docs/customization.md", "自定义配置与扩展"],
    ["docs/implementation-status.md", "实现状态与边界"],
    ["docs/mcp-support.md", "MCP 能力矩阵"],
    ["docs/中文使用说明.md", "中文启动/操作/复盘指南"],
    ["docs/中文确认计划说明.md", "/plan 的使用规则"],
    ["docs/upstream-lock.json", "上游参考固定提交"],
    ["web/app / web/dist", "前端源码与内嵌构建产物"],
    ["scripts/", "验收与基准脚本"],
    ["examples/samho.deepseek.toml", "真实 Provider 配置完整示例"],
  ],
  small: true,
}));
body.push(H.h1("附录 C 演示运行数据图表集"));
body.push(H.p("以下六张图表出自 2026 年 11 月 14 日的演示实例（17 任务、325 事件、68 次工具调用），统计口径与逐条分析见配套的《多任务运行报告与分析》。此处集中收录，供阅读本报告时对照工具层与运行时行为参考。"));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/tool-dist.png", "工具调用分布（68 次，橙色为失败）", { maxH: 460 }));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/tool-latency.png", "工具延迟对比（对数刻度）", { maxH: 460 }));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/task-gantt.png", "17 个任务执行时间窗（甘特图）", { maxH: 500 }));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/event-kinds.png", "轨迹事件类型分布（325 条）", { maxH: 460 }));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/budget-curve.png", "预算累计走势（236,676 token）", { maxH: 440 }));
body.push(...H.figure("D:/ddcode/rpi/reports/charts/session-scatter.png", "会话规模分布", { maxH: 460 }));
body.push(H.h1("附录 D HTTP 状态与错误对照"));
body.push(...H.tbl({
  caption: "常见 HTTP 状态与错误来源",
  headers: ["状态/表现", "来源场景", "处理方向"],
  widths: [26, 40, 34],
  rows: [
    ["202", "POST /api/run 受理成功，任务排队中", "用返回的 task_id 查询进度"],
    ["400", "请求体非法（如 JSON 编码不是 UTF-8）", "核对请求体；Windows 控制台用 --data-binary @file"],
    ["404", "任务/终端/文件不存在；或用短 ID 调终端接口", "使用完整 ID；文件类接口只收 workspace 内路径"],
    ["403/401", "Provider 侧鉴权失败（透传）", "检查 api_key_env 与网关配置"],
    ["跨源请求被拒", "CORS：非同源页面访问 API", "同源使用或本机工具直连"],
    ["任务列表无此任务", "任务已结束且超出活动列表窗口", "改查 /api/sessions 与 /api/events"],
    ["终端输入 404", "终端 ID 使用了界面缩略显示", "使用完整终端 ID"],
  ],
  small: true,
}));

body.push(H.h1("附录 E 演示实例完整配置样例"));
body.push(H.p("以下为本报告写作期间使用的演示实例完整配置（reports/demo-config.toml）：独立 data_dir 与 7433 端口，与主实例隔离；模型条目指向本机 OpenAI 兼容网关。它同时覆盖了第十三章中绝大多数配置项的实际写法，可直接作为新部署的起手模板——按需修改 workspace、data_dir、http_bind 与模型条目即可。"));
body.push(...H.codeBlock(`# 演示专用配置：独立 data_dir + 7433 端口，避免占用主实例
display_name = "Samho"
workspace = "D:/ddcode/rpi"
data_dir = "D:/ddcode/rpi/reports/demo-data"
http_bind = "127.0.0.1:7433"
max_turns = 40
max_concurrent_tasks = 4
request_timeout_ms = 600000
streaming = true

[tools]
shell_timeout_ms = 120000

[retry]
max_attempts = 3
initial_backoff_ms = 500
max_backoff_ms = 8000
reroute = true

[budget]
max_task_tokens = 4000000

[[models]]
id = "deepseek-v4.1-flash"
model = "wuyou/deepseek-v4.1-flash"
provider = "chat_completions"
tier = "reasoning"
base_url = "http://127.0.0.1:3425/v1"
api_key_env = "SAMHO_API_KEY"
context_window = 272000
max_output_tokens = 32768
supports_tools = true
supports_vision = true
supports_reasoning = true
streaming = true
output_token_parameter = "max_tokens"
input_usd_per_million = 0.0
output_usd_per_million = 0.0
latency_ms = 3000`.split("\n")));
body.push(H.p("配套的环境变量只有一个：SAMHO_API_KEY，保存网关的鉴权密钥，通过 [models] 的 api_key_env 字段按名字引用。启动命令为 cargo run -p samho -- serve --config reports/demo-config.toml；确认服务就绪后浏览器打开 http://127.0.0.1:7433 即可进入工作台。若需要从局域网其他设备访问，将 http_bind 改为 0.0.0.0:7432 并开放对应端口（默认配置即为此形态）。"));

// ── 装配 ──
const doc = H.buildDoc({
  numbering: [{
    reference: "seq-task",
    levels: [{
      level: 0, format: H.LevelFormat.DECIMAL, text: "%1.",
      alignment: H.AlignmentType.LEFT,
      style: { paragraph: { indent: { left: 720, hanging: 360 } } },
    }],
  }, {
    reference: "seq-security",
    levels: [{
      level: 0, format: H.LevelFormat.DECIMAL, text: "%1.",
      alignment: H.AlignmentType.LEFT,
      style: { paragraph: { indent: { left: 720, hanging: 360 } } },
    }],
  }],
  coverConfig: {
    title: "Samho 系统软件完整说明报告",
    titleLinesOverride: ["Samho 系统软件", "完整说明报告"],
    subtitle: "架构 · 运行语义 · 接口 · 安全边界 · 测试与性能",
    englishLabel: "SYSTEM SOFTWARE REFERENCE",
    metaLines: [
      "文档版本：V1.0（2026 年 11 月 14 日）",
      "事实基准：agent.md（工程权威参考）+ README + 源码实测",
      "配套文档：《Samho 项目完整介绍》《多任务运行报告与分析》",
      "写作原则：未验收能力一律标注为边界，不做超出证据的声明",
    ],
    footerLeft: "Samho · Rust Agent Framework",
    footerRight: "2026-11",
  },
  headerTitle: "Samho 系统软件完整说明报告",
  frontExtra: [
    H.h1("导读"),
    H.p("本报告是 Samho 系统软件的完整说明，共十七章加五个附录。第二至三章建立架构与运行时核心的全貌；第四至七章深入智能体循环、模型体系、工具体系与持久化；第八至十章给出 HTTP/SSE 接口、Web 工作台与 TUI 的完整说明；第十一至十二章覆盖 MCP 与 Skill 两大扩展机制；第十三章是配置速查；第十四至十七章如实交代测试、安全、边界与性能；附录 A 至 E 提供术语、文件索引、图表集、状态对照与可直接套用的完整配置样例。全部界面截图由浏览器自动化对运行中的真实服务操作截取；文中引用的端点为编写期间实测验证过的子集，其余以源码与中文使用说明为准。"),
  ],
  bodyChildren: body,
});

H.writeDocx(doc, "D:/ddcode/rpi/reports/out/Samho-系统软件完整说明报告.docx");
