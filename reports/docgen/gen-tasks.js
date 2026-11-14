// 《多任务运行报告与分析》— 图文 docx（数据全部来自演示实例真实日志）
const H = require("./helpers");
const S = (n) => "D:/ddcode/rpi/reports/shots/" + n;
const C = (n) => "D:/ddcode/rpi/reports/charts/" + n;

H.resetCounters();
const body = [];

// ── 一、摘要 ──
body.push(H.h1("一、摘要"));
body.push(H.p("本报告记录并分析 Samho 演示实例上的一次多任务并发运行。演示于 2026 年 11 月 14 日 12:58 至 13:29 进行，向同一个运行时先后提交 17 个中文编码任务，涵盖文档阅读、代码检索、网页生成、命令行执行与多步组合等类型。全部 17 个任务最终状态均为「已完成」，无失败、无取消——其中两次工具级失败（shell 命令）在任务内部被模型自我纠正，没有演变为任务级失败。"));
body.push(...H.tbl({
  caption: "关键指标总览",
  headers: ["指标", "数值", "说明"],
  widths: [30, 22, 48],
  rows: [
    ["提交任务数", "17", "全部完成，0 失败 0 取消"],
    ["会话数", "17", "每任务独立会话，其中 1 个为多轮续接会话"],
    ["调用轨迹事件", "325 条", "JSONL 落盘 + SSE 实时推送"],
    ["模型调用", "56 次", "17 次任务各含 1 次路由决策（model.route）"],
    ["工具调用", "68 次", "失败 2 次（shell），成功率 97.1%"],
    ["控制指令", "13 条", "steering 与 follow-up 排队/生效共 13 次事件"],
    ["累计消耗", "236,676 token / $0.0", "本地网关计费字段为 0"],
    ["单任务耗时", "10.3 – 239.1 秒", "最长者为多轮续接的四步组合任务"],
    ["全局并发上限", "4", "演示配置 max_concurrent_tasks = 4"],
  ],
}));

// ── 二、演示环境与方法 ──
body.push(H.h1("二、演示环境与方法"));
body.push(H.h2("2.1 演示环境"));
body.push(...H.tbl({
  caption: "演示实例配置（reports/demo-config.toml）",
  headers: ["配置项", "取值"],
  widths: [45, 55],
  rows: [
    ["服务地址", "127.0.0.1:7433（演示专用端口，不影响正式实例 7432）"],
    ["数据目录", "reports/demo-data（独立目录，文件锁独占）"],
    ["工作区", "D:/ddcode/rpi（仓库本身）"],
    ["模型", "wuyou/deepseek-v4.1-flash（经本机 OpenAI 兼容网关 127.0.0.1:3425/v1）"],
    ["层级绑定", "reasoning（推理）层级绑定上述单一模型"],
    ["max_turns / 并发", "40 轮 / 全局最多 4 个并发任务"],
    ["重试策略", "最多 3 次，指数退避 500ms 至 8s，失败允许降级路由（reroute）"],
    ["任务预算", "单任务 4,000,000 token 上限"],
  ],
  small: true,
}));
body.push(H.h2("2.2 触发与截图方法（如实披露）"));
body.push(...H.bullets([
  "任务触发：部分任务通过 Web 工作台界面手动提交与续接；并发批次通过页面内 fetch 调用本服务的 POST /api/run 接口触发（与界面提交走同一条运行时入口），以保证并发批次的时间对齐",
  "界面截图：使用浏览器自动化脚本驱动真实浏览器对运行中的服务操作并截屏；每次点击前均从实时 DOM 重新计算坐标",
  "两处自动化限制如实说明：浏览器面板与文档中心页面含 iframe，自动化合成截图中 iframe 内容无法成像（浏览器面板显示为空白画布，文档中心页面出现平铺伪影），因此这两张截图改用 Edge 无头浏览器对同一真实 URL 截取；终端面板为 PTY 渲染，自动化注入按键无法进入 xterm 隐藏输入框，故展示的是就绪状态的终端",
  "数据来源：任务/事件/预算数据分别来自 /api/sessions、reports/demo-data/events.jsonl 与 budget.jsonl，统计口径见附录",
]));

// ── 三、任务总览 ──
body.push(H.h1("三、任务总览"));
body.push(H.p("17 个任务以独立会话承载。下表按启动时间列出全部会话：时间为本地时钟，耗时为会话首末事件间隔，消息数包含用户消息、模型回复与工具卡片对应的消息条目。"));
body.push(...H.tbl({
  caption: "全部会话明细（17 个，均为已完成）",
  headers: ["会话", "启动", "消息", "工具调用", "耗时(s)", "任务主题"],
  widths: [12, 10, 8, 11, 10, 49],
  rows: [
    ["8f7ea2be", "12:58:56", "5", "1", "18.2", "阅读 README 开头并用中文总结"],
    ["e27b27f9", "13:00:39", "21", "10", "51.4", "在 examples 目录创建并迭代 hello-samho.html"],
    ["673e4b2b", "13:02:43", "24", "11", "79.3", "读 architecture.md 全文并自建验证脚本"],
    ["a0265403", "13:04:53", "9", "3", "34.1", "执行 PowerShell 命令（含一次失败自愈）"],
    ["9c39f362", "13:06:46", "9", "3", "13.9", "依次列出 crates 目录并读取文件"],
    ["3d5d9a7f", "13:08:03", "6", "2", "40.8", "两次 shell 命令验证环境"],
    ["c7abf1ae", "13:09:04", "23", "8", "239.1", "四步组合：列目录、读三份文档、写摘要"],
    ["a8e7a64c", "13:11:36", "7", "3", "22.7", "读 README 与两份文档写 MCP 摘要"],
    ["514cb0f0", "13:11:36", "9", "4", "14.7", "grep 搜索 fn run_with_task_id 并读文件"],
    ["4c02a575", "13:11:36", "8", "3", "11.9", "列出 samho-core 源码目录"],
    ["b5f7ae54", "13:27:06", "5", "1", "12.6", "读中文使用说明前 40 行列章节"],
    ["b77041ef", "13:27:20", "8", "3", "13.0", "grep 搜索组合任务（变体一）"],
    ["50462751", "13:27:20", "7", "3", "12.2", "读 README 与文档写 MCP 摘要（变体）"],
    ["8193d6e7", "13:27:20", "7", "3", "10.3", "列出 samho-core 源码目录（变体）"],
    ["115ffa89", "13:28:01", "9", "4", "15.3", "grep 搜索组合任务（变体二）"],
    ["0eeb6566", "13:28:01", "7", "3", "11.8", "依次读 README、LICENSE、示例配置"],
    ["3d8b20fe", "13:28:01", "7", "3", "11.6", "四步组合任务（精简变体）"],
  ],
  small: true,
}));
body.push(...H.figure(S("32-newui-tasks-running.png"), "并发运行中的任务列表面板：运行中 3 个（含停止按钮与轮次计数），已完成 4 个、失败 0、已取消 0"));
body.push(H.p("任务列表把运行中、已完成、失败、已取消四类分列展示。运行中的任务实时显示轮次、当前动作与「停止任务」按钮；每一项都带模型徽标（deepseek-v4.1-flash），方便确认路由结果。"));

// ── 四、并发执行分析 ──
body.push(H.h1("四、并发执行分析"));
body.push(...H.figure(C("task-gantt.png"), "17 个任务的执行时间窗（按会话，起点对齐第一个任务）"));
body.push(H.p("甘特图把演示期的三个阶段清晰分开。第一段（12:58 至 13:11）是界面手动提交阶段，七个任务大致串行，其中 c7abf1ae 是一个 239 秒的多轮任务（深色长条）。第二段在 13:11:36 前后，三个任务（a8e7a64c、514cb0f0、4c02a575）同时启动、并行完成，验证了多任务并发调度。第三段（13:27 至 13:29）通过 API 连发七个任务（b5f7ae54 先行，其余六个两批齐发），运行时按 max_concurrent_tasks = 4 的上限自动排队，所有任务依然全部按时完成。"));
body.push(H.p("值得注意的是并发并未显著拉长单任务耗时：第三段六个并行任务的耗时在 10.3 至 15.3 秒之间，与第一段串行时的同类任务基本一致。这是因为任务彼此独立、工具均为本地毫秒级操作，模型网关也有足够吞吐——并发瓶颈主要在 Provider 侧而非运行时侧。"));

// ── 五、代表性任务剖析 ──
body.push(H.h1("五、代表性任务剖析"));
body.push(H.h2("5.1 最简单的任务：README 摘要（8f7ea2be）"));
body.push(H.p("首个任务只要求读 README 开头并总结。轨迹显示一次路由决策、两轮模型调用、一次 read_file（12 毫秒）、任务 18.2 秒完成。这类任务展示了最小链路：路由器按确定性规则判定层级，模型一轮读文件、一轮作答，全程无冗余动作。"));
body.push(H.h2("5.2 网页生成与迭代：hello-samho.html（e27b27f9）"));
body.push(H.p("这个任务要求在 examples 目录创建一个介绍 Samho 的网页。模型的执行序列是：list_files 确认目录结构、glob 检查命名冲突、write_file 写出 3973 字节的 HTML，随后进行两次 edit_file 迭代润色，并用 shell（608ms、563ms）验证文件状态，最后 grep 与 list_files 复核。10 次工具调用覆盖 7 种工具，是全场工具种类最丰富的任务。"));
body.push(H.p("特别值得分析的是时间线里交错出现的三组控制事件（task.control.queued / applied）：任务运行期间，用户通过输入框追加了调整方向与追问。运行时没有打断正在执行的工具调用，而是把指令排队、在轮次边界生效——这正是 steering（下一轮生效）与 follow-up（任务后续接）两种机制的设计语义。最终生成的页面在浏览器中渲染如下。"));
body.push(...H.figure(S("24-preview-hello-samho.png"), "e27b27f9 任务的最终产物：examples/hello-samho.html 的浏览器渲染"));
body.push(H.h2("5.3 失败自愈：PowerShell 引号陷阱（a0265403）"));
body.push(H.p("任务要求用 shell 执行 PowerShell 命令。第一次 shell 调用返回错误（775ms，is_error = true）：命令字符串中的 ${} 被 shell 包装器的 PowerShell 展开规则提前求值，导致命令语法损坏。模型在下一轮识别了错误信息，改用单引号与转义规避展开，后续动作转为 read_file 两次核对目标文件，任务正常完成。这个案例说明任务级零失败的表象下仍有工具级失败，而失败信息进入上下文后模型具备自愈能力（详见第八章）。"));
body.push(H.h2("5.4 长会话多轮续接（c7abf1ae）"));
body.push(H.p("四步组合任务（列 docs 目录、读三份文档、写摘要）持续 239.1 秒、23 条消息、8 次工具调用，且期间叠加了 5 条追问排队（一批在轮次间连续生效）。会话中途模型把「继续读下一份」的追问合并进执行计划，而不是各自重启任务——journal 里同会话多轮 user 消息与单任务收尾共存，验证了同会话续接的完整语义。"));
body.push(H.h2("5.5 并发批次中的检索任务（115ffa89）"));
body.push(H.p("第三阶段的 grep 组合任务（搜索 fn run_with_task_id、读取所在文件、读取 workspace Cargo.toml）在 15.3 秒内完成 4 次工具调用：grep 10ms 定位、list_files 5ms 确认、read_file 两次读取。与串行阶段同类任务相比耗时持平，佐证第四章的并发结论。"));
body.push(...H.figure(S("43-session-result-full.png"), "任务完成后的会话视图：工具卡片（读取文件 · 完成）、思考摘要与编号答案"));

// ── 六、工具调用统计 ──
body.push(H.h1("六、工具调用统计"));
body.push(...H.figure(C("tool-dist.png"), "68 次工具调用按工具分布，橙色标注失败次数"));
body.push(...H.tbl({
  caption: "工具调用明细（次数 / 失败 / 延迟统计）",
  headers: ["工具", "次数", "失败", "p50 延迟", "最大延迟", "累计耗时"],
  widths: [22, 13, 13, 17, 17, 18],
  rows: [
    ["read_file", "37", "0", "5 ms", "14 ms", "196 ms"],
    ["list_files", "10", "0", "5 ms", "7 ms", "49 ms"],
    ["shell", "9", "2", "717 ms", "1112 ms", "6313 ms"],
    ["grep", "5", "0", "10 ms", "36 ms", "71 ms"],
    ["write_file", "3", "0", "8 ms", "9 ms", "23 ms"],
    ["glob", "2", "0", "8 ms", "8 ms", "16 ms"],
    ["edit_file", "2", "0", "9 ms", "9 ms", "17 ms"],
  ],
  small: true,
}));
body.push(...H.figure(C("tool-latency.png"), "工具延迟对比（对数刻度）：本地文件工具毫秒级，shell 受 PowerShell 进程启动拖慢"));
body.push(H.p("两个结构性结论。其一，文件类工具（read_file、list_files、glob、grep、write_file、edit_file）全部在毫秒级完成，最慢一次 grep 36ms，说明工具管线的开销主要在模型往返而非工具执行。其二，shell 平均耗时是文件工具的百倍以上——9 次 shell 共 6.3 秒，其中既有 PowerShell 进程冷启动的固有开销，也包含命令本身执行时间；两次失败同样发生在 shell，符合「外部进程交互是最脆弱环节」的普遍经验。"));

// ── 七、轨迹事件分析 ──
body.push(H.h1("七、轨迹事件分析"));
body.push(...H.figure(C("event-kinds.png"), "325 条轨迹事件的类型分布"));
body.push(...H.tbl({
  caption: "事件类型解读",
  headers: ["事件", "数量", "含义"],
  widths: [26, 12, 62],
  rows: [
    ["task.start / task.done", "17 / 17", "任务生命周期边界，与任务数一一对应"],
    ["model.route", "17", "每次任务提交时的层级判定，理由写入 payload（确定性规则分类器）"],
    ["model.start / model.response", "56 / 56", "模型往返：平均每个任务 3.3 轮，读写文件类任务 2 轮，长会话最多"],
    ["tool.start / tool.result", "68 / 68", "工具管线往返，result 记录延迟与 is_error"],
    ["task.control.queued / applied", "13 / 13", "steering 与 follow-up 指令的排队与生效，全部在轮次边界应用"],
  ],
  small: true,
}));
body.push(H.p("每个事件都带 sequence 序列号、trace_id / session_id / span_id / parent_span_id 关联字段与 object_id（如 tool:read_file、task:<id>）。演示实例按默认配置运行（log_content = false），正文内容被脱敏为占位符，只保留参数与结果的结构化摘要——这正是轨迹截图中「内容开关关闭」徽标的来源；开启后 journal 会保存完整正文，便于逐字复盘。"));

// ── 八、失败案例 ──
body.push(H.h1("八、失败案例深度分析"));
body.push(H.h2("8.1 两次 shell 失败的完整还原"));
body.push(H.p("全部 68 次工具调用中仅有的 2 次失败都来自 shell。共同模式是：提示词里包含 PowerShell 变量语法（如 $PSVersionTable 或 ${env:...}），命令经 shell 工具包装后传给 Windows PowerShell 时，双引号字符串里的 ${ 被展开求值，产生语法错误。"));
body.push(...H.bullets([
  "第一次（a0265403）：shell 返回 is_error = true（775ms）。模型在下一轮读取错误输出，改写命令为单引号形式规避展开，随后转为文件读取核对，任务完成",
  "第二次（673e4b2b）：同一模式的命令在长任务中再次失败（717ms）。模型改用输出重定向到临时文件、再 read_file 读回的方式绕过引号问题，任务继续推进并最终完成",
]));
body.push(H.h2("8.2 运行时层面的观察"));
body.push(H.p("两次失败均未触发配置的重试与降级路由（[retry] max_attempts = 3、reroute = true）——因为失败发生在工具层而非模型请求层，运行时把工具错误原样返回给模型自行处理，这是「工具失败留给模型自愈、模型请求失败交给重试器」的分层设计。任务级 0 失败的结果验证了这一层的健壮性，但也提示：对不可信命令字符串，PowerShell 包装器若能预先转义 ${ 模式，可以把这类失败从源头消除，这是一个值得列入后续迭代的改进点。"));

// ── 九、预算与成本 ──
body.push(H.h1("九、预算与成本"));
body.push(...H.figure(C("budget-curve.png"), "累计 token 消耗走势（budget.jsonl 全量快照；该文件无时间戳字段，横轴为逐条累计的预算记录序号）"));
body.push(H.p("budget.jsonl 在每次预算变动时写入累计快照，共 112 条。演示期累计消耗 236,676 token，折合成本字段 $0.0（本地网关未配置计费单价）。平均每个任务约 13,900 token；曲线斜率在三段并发期明显变陡，与并发批次对应。单任务预算上限设为 4,000,000 token，本次演示最高单任务消耗远低于上限，预算熔断未触发。"));

// ── 十、会话规模 ──
body.push(H.h1("十、会话规模与结论"));
body.push(...H.figure(C("session-scatter.png"), "会话规模分布：消息数与工具调用数基本线性相关"));
body.push(H.h2("10.1 观察结论"));
body.push(...H.bullets([
  "并发调度有效：17 任务 0 失败 0 取消；并发批次单任务耗时与串行持平，运行时调度不是瓶颈",
  "工具层毫秒级：文件工具 p50 不超过 10ms；shell 是唯一秒级工具，也是唯一出现失败的工具",
  "控制语义按设计工作：13 条 steering/follow-up 指令全部在轮次边界生效，不打断进行中的工具调用",
  "轨迹完整可复盘：325 条事件带完整关联字段落盘 JSONL，重放与导出可用",
  "失败自愈真实发生：2 次 shell 失败被模型在下一轮纠正，未升级为任务失败",
]));
body.push(H.h2("10.2 改进建议"));
body.push(...H.bullets([
  "shell 包装器对 PowerShell 变量语法预先转义或警告，可消除本演示中唯一的失败模式",
  "演示实例未开启 log_content，正文级复盘受限；生产使用建议在磁盘可控的前提下开启以获得完整轨迹",
  "模型角色表当前只绑定了 reasoning 层级；实际生产应补全六层级绑定，让自动分级真正分流不同复杂度的任务",
]));

// ── 附录 ──
body.push(H.h1("附录：统计口径与数据文件"));
body.push(...H.tbl({
  caption: "数据文件清单",
  headers: ["文件", "内容", "规模"],
  widths: [40, 42, 18],
  rows: [
    ["reports/demo-data/events.jsonl", "调用轨迹事件（含 sequence、trace/span 关联）", "325 行"],
    ["reports/demo-data/budget.jsonl", "预算累计快照（tokens/usd）", "112 行"],
    ["reports/demo-data/sessions.jsonl", "会话 journal（schema v2，SHA-256 链）", "165 行"],
    ["reports/stats.json", "本报告统计中间结果（脚本生成）", "—"],
    ["reports/charts/*.png", "六张统计图表（matplotlib）", "—"],
  ],
  small: true,
}));
body.push(H.p("耗时口径：会话首末事件时间差，含用户在轮次间输入追问的等待时间，因此略大于纯执行时间。任务状态口径：/api/tasks 返回的最终状态，工具失败不改变任务状态。"));
body.push(H.h2("复现本次运行"));
body.push(H.p("在仓库根目录按以下步骤可以复现一次与本报告同构的运行；演示数据目录与配置均未进入版本库，重跑不会污染主实例："));
body.push(...H.bullets([
  "准备模型网关：确保 OpenAI 兼容网关（演示用 http://127.0.0.1:3425/v1）可用，并在环境变量中设置 SAMHO_API_KEY",
  "启动演示实例：cargo run -p samho -- serve --config reports/demo-config.toml（独立 data_dir + 7433 端口，不占用主实例的 7432）",
  "打开工作台：浏览器访问 http://127.0.0.1:7433，右上角出现「运行时已连接」即服务就绪",
  "提交任务：在输入框逐条或并发提交第四章列出的任务；批量触发也可在页面控制台用 fetch 调 POST /api/run（本报告的并发批次即由此触发）",
  "运行中观察：任务列表面板查看状态徽标与轮次计数；检查区图标切换对象注册表、轨迹与预算面板",
  "事后复盘：读取 reports/demo-data/ 下的 events.jsonl、budget.jsonl 与 sessions.jsonl，按附录口径重算统计即可对上本报告的数字",
]));
body.push(H.p("需要注意的两点：其一，events.jsonl 的字段以 kind 区分事件类型，工具名取自 payload.name；其二，重跑会生成新的会话 ID 与新的 journal 文件，本报告中的会话 ID 表仅对当次演示有效。报告生成的全部脚本（collect_stats.py、make_charts.py 与 docgen/ 目录）已随本仓库提交入库（reports/ 在 .gitignore 中采用白名单制：脚本、截图、图表与三份成品 docx 入库，tools/ 本机工具、node_modules 与渲染中间页等大体积产物除外），数字与图表均可一键重算。"));

const doc = H.buildDoc({
  coverConfig: {
    title: "多任务运行报告与分析",
    subtitle: "17 个中文编码任务的并发执行 · 结果与失败案例复盘",
    englishLabel: "MULTI-TASK RUN REPORT",
    metaLines: [
      "演示日期：2026 年 11 月 14 日 12:58 – 13:29",
      "演示实例：http://127.0.0.1:7433（独立数据目录 reports/demo-data）",
      "数据来源：events.jsonl · budget.jsonl · /api/sessions（真实日志）",
      "结果总览：17 任务全部完成 · 325 事件 · 68 次工具调用 · 2 次 shell 失败自愈",
    ],
    footerLeft: "Samho · Rust Agent Framework",
    footerRight: "2026-11",
  },
  headerTitle: "多任务运行报告与分析",
  frontExtra: [
    H.h1("摘要"),
    H.p("本报告是对 Samho 演示实例一次多任务并发运行的完整复盘。报告的真实性原则贯穿全文：所有数字来自运行时落盘的 JSONL 日志与 API 返回，所有界面截图来自对运行中服务的浏览器自动化操作，触发方式（界面提交、页面内 API 调用、无头浏览器截屏）在第二章如实披露，包括自动化手段本身的局限。分析部分回答五个问题：并发调度是否有效、工具层表现如何、失败如何发生与自愈、控制指令是否按设计生效、预算消耗处于什么量级。"),
  ],
  bodyChildren: body,
});

H.writeDocx(doc, "D:/ddcode/rpi/reports/out/Samho-多任务运行报告与分析.docx");
