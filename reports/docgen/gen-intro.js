// 《Samho 项目完整介绍》— 图文 docx
const H = require("./helpers");
const fs = require("fs");
const S = (n) => "D:/ddcode/rpi/reports/shots/" + n;
const C = (n) => "D:/ddcode/rpi/reports/charts/" + n;

H.resetCounters();

const body = [];

// ── 1 项目概述 ──
body.push(H.h1("一、项目概述"));
body.push(H.p("Samho 是 Pi coding agent 的 Rust 重写工程。项目在完整下载并分析上游仓库（Pi、DSH、ZCode 等参考实现）之后，采取「增量重写」的交付策略：不是一次性翻译上游全部功能，而是把已经验证过的核心链路逐块用 Rust 重建，每一步都保持可运行、可测试、可复盘。当前版本已经交付四大件：Rust 运行时核心（samho-core）、共享本地服务（HTTP + SSE）、Ratatui 终端界面（TUI）和本地 Web 图形工作台（React 工作台）。尚未完成的能力仍按实现状态文档保留边界，不做超出证据的宣称。"));
body.push(H.p("把这套系统说清楚，可以概括成一句话：它把「会话、任务、模型、工具、技能、MCP 服务」全部纳入统一的 Object 注册表管理，用异步 Agent 循环驱动任务执行，用六级模型策略按任务复杂度自动分配模型，用 JSONL 日志把每一次调用轨迹完整落盘，再通过 Web 工作台与 TUI 两种界面共用同一个 Rust 服务对外呈现。"));
body.push(H.p("项目仓库位于 D:/ddcode/rpi，构建产物与演示环境彼此隔离：正式服务默认绑定 0.0.0.0:7432，本报告中的截图与运行数据来自独立数据目录与 127.0.0.1:7433 的演示实例，不污染主实例数据。"));
body.push(...H.figure(S("30-workbench-home-new.png"), "Samho Web 工作台首页：左侧项目与会话列表，中间对话区，右侧设备面板"));

// ── 2 核心能力 ──
body.push(H.h1("二、核心能力总览"));
body.push(H.p("下表汇总当前已实现的八类核心能力。每一项都有对应的自动化测试与验收脚本支撑，详细行为在《系统软件完整说明报告》中逐章展开。"));
body.push(...H.tbl({
  caption: "Samho 核心能力一览",
  headers: ["能力域", "要点", "关键实现位置"],
  widths: [22, 50, 28],
  rows: [
    ["项目制工作区", "项目关联多个 workspace 目录，项目下展开多会话；会话可挂附属终端与浏览器，浏览器标签持久化并随重启恢复", "runtime + Web 工作台"],
    ["Object 契约", "统一对象注册表：依赖拓扑校验、失败回滚、逆序销毁；运行时/Model/Tool/Skill/MCP 全部纳入", "samho-core runtime"],
    ["异步 Agent 循环", "任务并发限制、取消、失败状态、会话持久化与重启恢复；支持自定义 Rust Tool", "agent 模块"],
    ["六级模型策略", "nano/fast/balanced/reasoning/frontier/specialist，可配置分类规则与能力约束；支持手动指定层级或模型", "model routing"],
    ["Provider 体系", "Chat Completions 兼容 Provider + 显式模拟 Provider；真实 HTTP fixture 验证模型→工具→模型循环", "provider 模块"],
    ["调用轨迹", "JSONL trace、序列号、trace/session/span/parent 关联、路由理由、脱敏、导出与 SSE 实时推送", "trace/journal"],
    ["MCP 支持", "stdio 与 Streamable HTTP 双通道；工具、资源、模板、prompts、分页、超时与子进程回收（rmcp 3.5.1）", "mcp 集成"],
    ["Web 工作台", "React + TypeScript + Tailwind v4 + shadcn/ui，构建为单文件产物内嵌二进制，严格 CSP，Markdown 全量渲染与沙箱预览", "web/app → web/dist"],
  ],
}));
body.push(...H.figure(S("36-models-panel.png"), "模型面板：演示实例将 deepseek-v4.1-flash 绑定到推理（reasoning）层级的角色配置"));

// ── 3 系统架构 ──
body.push(H.h1("三、系统架构"));
body.push(H.h2("3.1 分层结构"));
body.push(H.p("Samho 采用两层 crate 结构。samho-core 是纯库形态的运行时核心，负责配置校验、数据目录管理、技能发现、MCP 连接、对象注册、模型路由、会话与轨迹存储；samho-cli 是入口适配层，提供 serve、tui、run、inspect、replay、config 六个子命令。Web 工作台与 Ratatui TUI 都通过同一组 HTTP/SSE 接口访问同一个运行时，保证两种界面看到的状态完全一致。"));
body.push(...H.tbl({
  caption: "仓库主要组成",
  headers: ["路径", "内容"],
  widths: [34, 66],
  rows: [
    ["crates/samho-core", "运行时核心库：runtime、agent 循环、provider、模型路由、Object 注册表、MCP、Skill、存储"],
    ["crates/samho-cli", "命令行入口：serve（HTTP/SSE 服务）、tui（终端界面）、run/inspect/replay（CLI 运行与复盘）、config"],
    ["web/app", "React + TypeScript 工作台源码（Vite 8 构建）"],
    ["web/dist", "构建产物，经 include_str! 内嵌进二进制（共 6 个静态资源）"],
    ["docs/", "架构、自定义配置、实现状态、上游分析等 17 份中英文文档"],
    ["scripts/", "验收与基准脚本：smoke_runtime.py、smoke-web.ps1、benchmark-runtime.ps1 等"],
    ["examples/", "真实 Provider 配置模板（samho.deepseek.toml、samho.chat.toml）"],
  ],
}));
body.push(H.h2("3.2 数据流"));
body.push(H.p("一次典型任务的执行链路是：用户在 Web 工作台填写目标并提交，POST /api/run 返回任务 id（HTTP 202）；运行时按会话排队执行 Agent 循环，每一轮把对话上下文发送给模型路由选中的 Provider，模型返回工具调用则进入工具管线执行，结果回填后再进入下一轮，直到模型给出最终答案或触发轮次/预算上限；全过程的事件按 trace/session/span 层级写入 JSONL 日志，同时经 SSE 推送到工作台实时渲染；任务结束后会话 journal 与预算快照落盘，重启后可完整恢复。"));
body.push(...H.figure(S("43-session-result-full.png"), "会话详情：左侧 11 条历史会话，中部为一次已完成任务的工具卡片、思考摘要与编号答案"));

// ── 4 运行语义 ──
body.push(H.h1("四、智能体运行语义"));
body.push(H.h2("4.1 任务状态机"));
body.push(H.p("任务从提交到结束经历排队、运行、完成/失败/取消五种状态。每个会话同一时刻只允许一个活动任务；全局并发上限由配置项 max_concurrent_tasks 控制（演示实例设为 4）。任务执行中用户可以随时取消，也可以在不打断当前任务的情况下追加「调整方向」（steering，下一轮生效）或「追问」（follow-up，当前任务结束后自动续接）两类控制指令。"));
body.push(H.h2("4.2 分级路由"));
body.push(H.p("提交任务时可以不指定模型——运行时按任务复杂度自动选择层级，决策理由会写进轨迹日志（model.route 事件）；也可以手动锁定层级或具体模型。六个层级对应不同上下文窗口、预算与延迟约束，界面中文分别显示为轻量、快速、均衡、推理、前沿、专家。层级与物理模型的绑定关系由 model_roles.json 维护，演示实例把推理层级绑定到本地网关的 deepseek-v4.1-flash。"));
body.push(...H.figure(S("44-model-picker.png"), "Ctrl+M 模型选择器：自动路由 + 六层级列表，未配置的层级会明确标注"));
body.push(H.h2("4.3 工具管线"));
body.push(H.p("内置九个工具：read_file、write_file、edit_file、list_files、glob、grep、shell、webfetch、todo。write 与 shell 默认开启（shell 以服务进程的 OS 用户权限执行，不是沙箱）；处理不可信内容时建议在配置的 [tools] 段显式关闭 shell。Skill 系统通过 SKILL.md 的 allowed-tools 声明参与工具准入控制；MCP 服务器提供的工具在握手后同样进入统一管线。所有工具调用在轨迹里记录参数摘要、结果摘要、延迟与错误标记。"));

// ── 5 Web 工作台 ──
body.push(H.h1("五、Web 工作台巡览"));
body.push(H.h2("5.1 三栏布局与设备面板"));
body.push(H.p("工作台分为三栏：左侧是项目与会话管理（新建任务快捷键 Alt+N、搜索、项目分组），中间是当前会话的对话与任务时间线，右侧是可折叠的设备面板。设备面板包含文件、Git、终端、浏览器四个标签页，全部指向当前 workspace。"));
body.push(...H.figure(S("37-files-readme.png"), "文件面板：点击 README.md 后右侧以渲染视图展示 Markdown 内容，支持源码编辑"));
body.push(...H.figure(S("38-git-panel.png"), "Git 面板：分支切换、暂存区管理；图中 examples/hello-samho.html 为 Agent 新建的文件"));
body.push(...H.figure(S("39-terminal-panel.png"), "终端面板：每个会话可挂多个 PTY 终端标签，PowerShell 提示符就绪"));
body.push(...H.figure(S("40-browser-panel.png"), "浏览器面板：地址栏经服务端代理（/api/proxy）加载网页，支持多标签；图中为未导航时的空态，Agent 生成页面的实际渲染效果见下文 hello-samho 预览图"));
body.push(H.h2("5.2 扩展状态面板"));
body.push(H.p("检查区一排图标进入对象注册表、技能、MCP 与模型四个状态面板，随时查看运行时内部状态。"));
body.push(...H.figure(S("33-objects-panel.png"), "对象注册表：运行时对象按类型列出，可逐个查看能力与状态（截图时 30 个，随会话与终端数量增长）"));
body.push(...H.figure(S("34-skills-panel.png"), "技能面板：SKILL.md 校验结果与发现路径"));
body.push(...H.figure(S("35-mcp-panel.png"), "MCP 面板：服务器连接状态与工具清单"));
body.push(H.h2("5.3 界面设置与主题"));
body.push(...H.figure(S("41-settings-dialog.png"), "界面与快捷键设置：主题、发送任务（Ctrl+Enter）、新建会话（Alt+N）、取消任务（Ctrl+K）"));
body.push(...H.figure(S("42-light-theme.png"), "一键切换亮色主题，全部组件同步换肤"));
body.push(H.h2("5.4 沙箱预览"));
body.push(H.p("对话中出现的 HTML 代码块与 workspace 内的网页可以在隔离沙箱中直接运行预览。预览页面运行在独立的受限源（CSP sandbox、Origin 为 null），与服务源隔离，无法携带用户凭据；文件面板与预览均拒绝符号链接与越界路径。下图为演示中 Agent 按任务生成的 examples/hello-samho.html 实际渲染效果。"));
body.push(...H.figure(S("24-preview-hello-samho.png"), "Agent 生成的 hello-samho.html 在浏览器中的真实渲染效果"));

// ── 6 快速上手 ──
body.push(H.h1("六、快速上手"));
body.push(H.h2("6.1 环境要求"));
body.push(...H.bullets([
  "Rust 1.88 及以上工具链（CI 按 MSRV 1.88 校验）",
  "运行 Web 工作台无需 Node：web/dist 构建产物已随仓库提交；修改前端才需要 Node 22",
  "MCP stdio 集成测试需要 Python 3",
  "真实模型推理需要一个 OpenAI 兼容网关或自配 Provider；默认 Provider 是明确标注 [DEMO simulation] 的离线模拟器",
]));
body.push(H.h2("6.2 启动服务与界面"));
body.push(...H.codeBlock([
  "cargo run -p samho -- serve --bind 0.0.0.0:7432",
  "# 本机打开 http://127.0.0.1:7432；局域网地址见启动日志与页面顶栏",
  "",
  "cargo run -p samho -- tui --url http://127.0.0.1:7432",
  "# 另开终端连接同一个服务，TUI 与网页看到同一运行时",
]));
body.push(H.h2("6.3 配置真实模型"));
body.push(...H.codeBlock([
  "cargo run -p samho -- config init samho.toml",
  "# config init 默认接入本机网关 http://127.0.0.1:3425/v1",
  "# 模型 wuyou/deepseek-v4.1-flash，先设置 SAMHO_API_KEY",
  "",
  "cargo run -p samho -- --config samho.toml serve",
]));
body.push(H.p("等价的完整配置示例见 examples/samho.deepseek.toml：模型条目里显式声明端点、物理模型 ID、能力开关（工具/视觉/推理）、上下文窗口、最大输出、费用与延迟提示。所有模型配置均由使用者维护，分级标签不代表已测性能。"));
body.push(H.h2("6.4 验证与复盘"));
body.push(...H.codeBlock([
  "cargo fmt --all -- --check",
  "cargo test --workspace          # 201 项 Rust 测试",
  "cargo clippy --workspace --all-targets -- -D warnings",
  "node --test scripts/test-workbench-contracts.mjs",
  "",
  "powershell -NoProfile -File scripts/smoke-web.ps1 -Force",
  "python scripts/smoke_runtime.py --force",
]));
body.push(H.p("CLI 侧的独立验证三件套：run 以指定复杂度执行一条任务并输出 JSON；inspect 打开数据目录打印运行时状态；replay 把轨迹导出为 trace.jsonl。注意这三个命令会打开本地数据目录，服务运行时应改用 TUI/Web 访问同一运行时，或为 CLI 指定另一数据目录——文件锁会拒绝两个进程同时写同一目录。"));

// ── 7 安全边界 ──
body.push(H.h1("七、安全边界与当前限制"));
body.push(...H.callout("安全须知", "serve 默认绑定 0.0.0.0:7432 并拒绝跨源请求，但没有身份认证，也不是 OS 沙箱——只应在可信局域网使用，需要时用 --bind 127.0.0.1:7432 收回本机。默认工具策略包含 shell 与 write：Agent 可以以当前用户权限执行任意命令、写 workspace 内文件。处理不可信内容（外部仓库、网页、MCP 数据）时，请在配置的 [tools] 段关闭 shell。"));
body.push(...H.bullets([
  "文件面板与内置文件工具都会拒绝符号链接目标与越界路径；Git 面板操作有超时与输出上限",
  "log_content = false 仅省略轨迹正文；会话恢复仍保存经脱敏的完整消息",
  "trace replay 是事件查看/导出，不会重执行外部副作用",
  "完整 MCP 官方 conformance、真实供应商质量、跨平台实机、24 小时 soak 与 Pi/Rust 同机性能对照尚未验收；这些限制不能由本地模拟器结果推断",
  "性能比较必须同 commit、同构建条件、同工作负载；本地基准脚本为 scripts/benchmark-runtime.ps1",
]));

// ── 附录 ──
body.push(H.h1("附录：上游参考与文档索引"));
body.push(H.p("上游参考仓库（pi/dsh/opencode/zcode）是分析期临时 clone，未入库，固定提交记录保留在 docs/upstream-lock.json。源码证据与重写设计文档如下。"));
body.push(...H.tbl({
  caption: "上游参考固定提交",
  headers: ["项目", "固定提交"],
  widths: [45, 55],
  rows: [
    ["Pi（earendil-works/pi）", "2db5e359bf84c1c0be51d2c5c5c5c7cf27072c2b"],
    ["DSH（deepseek-ai/deepseek-harness）", "5badb15009ae1756c3afe0ae0cef1faafc290ccc"],
    ["ZCode（zai-org/ZCode）", "29628c9acdb81b703bbd4080c207a0e7ce5e276e"],
  ],
  small: true,
}));
body.push(...H.tbl({
  caption: "中文文档索引",
  headers: ["文档", "内容"],
  widths: [40, 60],
  rows: [
    ["中文使用说明", "启动、网页/TUI 操作、任务执行、复盘与配置入口"],
    ["当前架构 architecture.md", "分层设计、Object 契约、数据流"],
    ["自定义配置 customization.md", "runtime.toml 全字段与扩展方式"],
    ["实现状态 implementation-status.md", "当前能力边界与后续阶段"],
    ["MCP 支持矩阵 mcp-support.md", "stdio/HTTP 双通道能力清单"],
    ["多场景验证计划 validation-plan.md", "M01-M10 评测计划"],
    ["Pi 全面分析 pi-analysis.md", "上游模型/Agent/工具/持久化/性能热路径分析"],
    ["DSH/ZCode 参考分析 reference-analysis.md", "上游插件与 Object 设计对比"],
  ],
  small: true,
}));

// ── 文档装配 ──
const doc = H.buildDoc({
  coverConfig: {
    title: "Samho 项目完整介绍",
    subtitle: "Pi coding agent 的 Rust 增量重写工程 · 图文版",
    englishLabel: "SAMHO PROJECT OVERVIEW",
    metaLines: [
      "文档日期：2026 年 11 月 14 日",
      "演示实例：http://127.0.0.1:7433（独立数据目录）",
      "演示模型：deepseek-v4.1-flash（本地 OpenAI 兼容网关）",
      "配图说明：全部界面截图由浏览器自动化对运行中的真实服务操作截取",
    ],
    footerLeft: "Samho · Rust Agent Framework",
    footerRight: "2026-11",
  },
  headerTitle: "Samho 项目完整介绍",
  frontExtra: [
    H.h1("摘要"),
    H.p("本报告面向希望快速了解 Samho 的读者，用图文并茂的方式介绍项目的定位、核心能力、系统架构、运行语义与 Web 工作台的全部主要界面。报告共七章加一个附录：第一、二章给出概述与能力总览；第三章拆解系统架构与数据流；第四章解释任务状态机、分级路由与工具管线的运行语义；第五章逐一巡览 Web 工作台界面，全部 16 张界面截图均来自对运行中真实服务的浏览器自动化操作；第六章给出可直接复制的上手命令；第七章如实声明安全边界与当前限制。"),
    H.p("需要强调的一点：本报告中的所有截图与运行数据都来自演示实例（独立数据目录、127.0.0.1:7433），配置文件与数据未进入正式仓库；文中不包含任何超出实现状态文档的说法，未验收能力一律明确标注。"),
  ],
  bodyChildren: body,
});

H.writeDocx(doc, "D:/ddcode/rpi/reports/out/Samho-项目完整介绍.docx");
