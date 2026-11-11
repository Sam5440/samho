# Samho 全面代码审查与性能优化报告

**审查日期：** 2026-11-10  
**审查提交：** `3a680cea961fdbd976935e65da34ea73d7b52d73` (`master`)  
**审查结论：** 项目总体结构清楚，Rust 核心与 Axum 本地服务边界明确，Web/TUI 共用运行时，测试规模和协议/恢复覆盖较完整。当前最需要先处理的不是盲目压缩依赖或拆分模块，而是默认工具权限与文件边界、轨迹流高频全量加载、并发提交/工作区切换竞态及动态工作区持久化/关停问题。没有发现足以判定为 P0 的已验证问题；但 P1 中包含可导致越权写文件、UI 注入、默认 shell 执行风险和任务状态不一致的项目，应优先排期。

性能方面，当前 Windows release Demo 新进程任务基线为 **p50 254.154 ms、p95 271.439 ms、峰值工作集 p50 12,060,672 bytes**。该指标包含进程启动、runtime 初始化、模拟任务、持久化和关闭，不测量真实 Provider 延迟或浏览器页面性能。release 构建本次耗时约 **7 分 46 秒**。项目已有旧版 Demo 基准 p50 88.946 ms、峰值工作集 p50 9,777,152 bytes，但对应旧代码/二进制，不能据此推断当前发生了回归。

## 优先级总览

| 顺序 | 优先级 | 主题 | 核心影响 | 建议动作 |
|---|---|---|---|---|
| 1 | P1 | 默认 shell/write 权限及本机信任边界 | 模型/提示注入可触发当前用户权限下的系统命令；文档与默认值矛盾 | 立即明确授权策略，补安全提示及默认配置测试 |
| 2 | P1 | workspace 文件写入符号链接边界 | 文件面板写入可跟随 workspace 内指向外部的符号链接 | 所有写路径使用抗链接/竞态的安全打开方式 |
| 3 | P1 | 轨迹搜索 HTML 插入 | 将 trace 中不可信文本带入 `innerHTML` | 改用文本节点和 mark 节点，并做恶意样本回归 |
| 4 | P1 | 流式轨迹重复分页和整表重绘 | 进度频繁时放大 API 请求、DOM 工作和主线程占用 | 加载去重/增量分页、事件批量更新和增量渲染 |
| 5 | P1 | 会话锁顺序反转 | 并发 checkpoint 与级联删除可能死锁 | 统一锁顺序并用并发压力测试验证 |
| 6 | P1 | 动态工作区注册表被覆盖 | 创建第二个动态工作区会令先前条目在重启后丢失注册 | 写入全量快照并做多工作区重启测试 |
| 7 | P1 | 工作区切换和任务提交竞态 | 旧响应污染新工作区；提交等待期间可启动多个任务 | 用代际令牌/提交锁及延迟响应测试收口 |
| 8 | P1 | 手机侧栏导航不可达 | 375px 等窄屏下无法切换会话和工作区 | 增加可访问的移动抽屉入口 |
| 9 | P2 | 文件、Git、代理等资源限制不完整 | 大文件/大目录/大 Git 输出/并发代理可能造成高内存占用 | 流式限额、分页、超时和并发限流 |
| 10 | P2 | 动态工作区关停和 PTY 生命周期 | 活动 SSE 可能阻止服务优雅退出；移除 workspace 可能遗留 PTY | 统一 shutdown token，移除时先关闭终端并取消任务 |
| 11 | P2 | 持久化热路径 | durable 模式每个事件 sync；长会话 checkpoint 反复复制历史 | 先做阶段 profiling，再批量同步/增量提交 |
| 12 | P2 | 测试脚本契约漂移 | 现有 smoke 与 benchmark 因 Demo 文案/UTF-8 处理失败 | 统一用结构化字段验证，不依赖完整输出字符串 |
| 13 | P2 | CI 工具链与供应链验证 | 未验证声明的 MSRV，Actions 未固定 SHA，未做依赖公告扫描 | 增 MSRV job、固定 SHA、定期 advisory 扫描 |
| 14 | P2 | 前端无障碍细节 | 流式全文被重复 live announce；对话框焦点/命名不足 | 提供状态摘要播报、dialog 标题关联和焦点恢复 |
| 15 | P3 | 持久化失败后的状态一致性 | 项目写盘失败后内存已变更；启动 I/O 错误被静默当作空数据 | 失败回滚、区分 not-found 与其他错误 |
| 16 | P3 | 常驻任务快照缺少字节上限 | 任务数有上限但完整结果体积无上限 | 设总字节/结果长度预算和明确过期策略 |
| 17 | P3 | 文档基线和基准可比性 | README 测试数落后；旧/新 benchmark 不同代码版本 | 自动生成测试摘要并记录 commit、环境和工作负载 |

## 评估范围与架构

- **核心运行时：** `crates/samho-core` 管理配置、模型路由、Agent 循环、工具、MCP/Skill、会话/事件存储和预算。
- **入口与服务：** `crates/samho-cli` 提供 `run`、`serve`、TUI、会话操作、面板 API 和动态工作区。`serve` 限制在 loopback 地址，Web 与 TUI 共用 API 和运行时。
- **前端：** `web/index.html` 加载原生 ESM 和拆分 CSS；工作台、文档页、计划页均为本地静态资源。无 npm 依赖/打包链；xterm 与 canvas addon 从终端面板按需加载，文档数据不进入工作台主入口。
- **存储与运行方式：** JSONL/SQLite 事件和会话 journal、项目清单及动态 workspace 注册表均在本地数据目录；`durable_logs=true` 默认逐次同步落盘。服务默认绑定 `127.0.0.1:7432`。
- **信任边界：** loopback 与 Host/Origin 检查可降低远程网页滥用风险，但不是身份认证、同机进程隔离或 OS 沙箱。工具和 PTY 都以运行服务的 OS 用户权限运行。

## 逐项发现与修改建议

### P1-01 默认工具权限没有显式用户授权，且 shell 不是沙箱

- **问题描述：** 默认 `ToolPolicy` 同时开启 `write`、`edit`、`shell` 等工具。Shell 将模型传入命令交给 `powershell.exe -Command` 或 `sh -c`，工作目录限制不等同于文件/网络/进程沙箱。提示注入或被污染的仓库内容可能诱导 Agent 在当前用户权限下执行命令。与此同时 README 称写入和 shell“可选开启”，与实际默认值冲突。
- **代码位置：** [`config.rs:224`](../crates/samho-core/src/config.rs#L224)、[`tools.rs:634`](../crates/samho-core/src/tools.rs#L634)、[`README.md:48`](../README.md#L48)、服务本机边界 [`server.rs:475`](../crates/samho-cli/src/server.rs#L475) 与 [`server.rs:581`](../crates/samho-cli/src/server.rs#L581)。
- **判断依据：** 静态代码确认默认值为 true，shell 使用系统命令解释器；HTTP 服务只强制 loopback。未做破坏性命令测试。
- **影响范围：** 安全、用户数据、凭据和本机系统。风险主要来自不可信任务、外部仓库内容或本机其他程序，不代表本服务可从任意网络直接访问。
- **修改建议：** 近期先让 shell 由明确配置/启动确认启用，并在首次启动和工作台中清楚展示“本地执行、使用当前 OS 权限”；write 与 shell 权限分开控制；为 MCP 和提示注入场景补充高风险工具确认/策略接口。若要执行不可信代码，采用 OS 级隔离（受限账户/容器/Job Object/沙箱），不要把 `current_dir` 描述成沙箱。同步修正 README 的默认行为说明。
- **预期收益：** 已实测收益：无，属于安全收益。估算收益：默认关闭/显式授权可消除未获用户明确授权的 shell 调用路径；不能消除用户主动开启后的命令风险。定性上降低误用和提示注入的爆炸半径。
- **实施成本：** 2–4 人日；默认值变更需兼容迁移/CLI 配置提示及 UI 行为确认。
- **风险与验证：** coding-agent 默认体验会增加一步授权，可能影响自动化；用默认配置、显式开启配置、MCP 工具和既有 Agent 集成用例覆盖。验收标准：默认状态下 shell 不注册或被策略拒绝；显式启用时保留现有能力；文档与 `config init` 输出一致。
- **排序理由：** 默认开启、高权限、影响面跨整个 OS，且配置/文档不一致，安全影响高于其他工作。

### P1-02 文件面板写入可跟随 workspace 内的外部符号链接

- **问题描述：** 对 `must_exist=false` 的路径，`confined()` 只 canonicalize 父目录再拼回 basename，不核验目标本身是否为符号链接。`file_write()` 随后直接 `tokio::fs::write()`，会跟随目标链接。核心工具 `within_workspace()` 对已存在链接有检查，但悬空链接时 `Path::exists()` 返回 false，转而仅校验父目录，也存在跟随写入风险。
- **代码位置：** [`panels.rs:45`](../crates/samho-cli/src/panels.rs#L45)、[`panels.rs:57`](../crates/samho-cli/src/panels.rs#L57)、[`panels.rs:219`](../crates/samho-cli/src/panels.rs#L219)、[`tools.rs:381`](../crates/samho-core/src/tools.rs#L381)、[`tools.rs:395`](../crates/samho-core/src/tools.rs#L395)、[`tools.rs:526`](../crates/samho-core/src/tools.rs#L526)。
- **判断依据：** 静态追踪路径检查后到写入的调用链。尝试在当前 Windows 测试容器创建符号链接时，系统返回 `WinError 1314`（缺少特权），因此没有动态改写外部哨兵文件。
- **影响范围：** 文件系统完整性和 workspace 隔离；需要攻击者/仓库内容能在 workspace 中放置链接，并让面板或 Agent 对该相对路径写入。
- **修改建议：** 写入时采用平台支持的“不跟随最终链接”打开标志/句柄 API；检查最终组件和父目录，并避免“检查后再打开”的 TOCTOU。将面板与 core 工具集中到同一套安全路径 API。对 rename/mkdir 等也统一审查。
- **预期收益：** 已实测收益：未动态确认。估算/定性收益：使写入的实际目标遵守 workspace 边界，不应承诺抵御所有本机并发目录替换，除非采用句柄相对路径 API。
- **实施成本：** 1–3 人日，需跨平台 Windows/Unix 实现和测试。
- **风险与验证：** 符号链接/重解析点语义在 Windows 与 Unix 不同。验收需覆盖普通文件、正常新文件、目录链接、目标链接、悬空链接及链接替换竞态；外部哨兵内容必须保持不变。
- **排序理由：** 属于明确的路径隔离绕过，可能改写 workspace 之外的数据，优先于普通健壮性优化。

### P1-03 轨迹搜索把不可信文本作为 HTML 插入

- **问题描述：** 搜索高亮只转义命中的片段，命中前后的原文未经转义拼入 HTML，再写入 `innerHTML`。trace 中的模型输出、工具结果或用户输入可能包含 HTML；当搜索词命中其附近内容时，浏览器会解析该标记而非按文本显示。全站 CSP 限制了脚本执行，但这仍是 HTML 注入面，可能生成伪造界面或外部资源请求。
- **代码位置：** [`render.js:27`](../web/js/trajectory/render.js#L27)、[`render.js:48`](../web/js/trajectory/render.js#L48)、[`render.js:57`](../web/js/trajectory/render.js#L57)。
- **判断依据：** 代码直接将 `parts.join('')` 赋给 `pre.innerHTML`，而 `parts` 的前缀/后缀是 `raw.slice()`；现有中文文档页 DOM 安全测试没有覆盖工作台轨迹渲染。
- **影响范围：** 查看/搜索包含用户或模型控制文本的轨迹时，影响前端 DOM 完整性和本机工作台用户。
- **修改建议：** 构造文本节点：无命中部分用 `createTextNode()`，命中部分创建 `<mark>` 并设置 `textContent`，完全移除该处 `innerHTML`。增加轨迹搜索恶意字符串测试。
- **预期收益：** 已实测收益：未单独执行 XSS fixture。估算收益：消除此渲染路径的 HTML 解析，不依赖 CSP 继续兜底。
- **实施成本：** 0.5–1 人日。
- **风险与验证：** 需保持空白归一、Unicode 搜索和多命中映射行为；测试 `<img onerror>`、`<svg>`、`</mark>`、`&`、多字节文本只作为字面文本呈现且不产生元素。
- **排序理由：** 修复范围小、验证容易，且接触不可信模型/工具输出，适合快速关闭。

### P1-04 流式进度会重复分页拉取完整轨迹并重建列表

- **问题描述：** 每个携带 `trace_id` 的进度快照都调用 `loadTrajectory()`；每次从游标 0 顺序拉取最多 60 页、每页 500 条。没有按 trace 去重或复用中的 Promise。每条 trace SSE 事件又调用 `setEvents()`，轨迹面板 `render()` 清空并重建所有调用卡片。长任务/高频流会将请求数和 DOM 工作放大。
- **代码位置：** [`main.js:135`](../web/js/main.js#L135)、[`main.js:140`](../web/js/main.js#L140)、[`main.js:226`](../web/js/main.js#L226)、[`main.js:240`](../web/js/main.js#L240)、[`main.js:426`](../web/js/main.js#L426)、[`render.js:239`](../web/js/trajectory/render.js#L239)。
- **判断依据：** 代码路径可由每个 progress SSE/poll 帧重复进入；单次完整拉取上限为 30,000 个事件，随后对列表进行 `replaceChildren()`。未做浏览器性能 trace 或长事件量压力测试，因此没有当前 CPU/请求量实测值。
- **影响范围：** 大轨迹的 API 延迟、CPU、内存分配和滚动/输入响应；多个同时任务会进一步叠加。
- **修改建议：** 按 trace 缓存已加载状态并去重 in-flight 请求；初次仅拉取最新/当前页，gap 时从 cursor 补齐；事件到达按 animation frame 批量提交。轨迹列表增量更新或对大量调用采用虚拟列表，避免重建展开状态和所有卡片。
- **预期收益：** 已实测收益：无；代码行为已确认，尚未收集生产请求数。估算验收目标：单个 trace 首次只加载所需页面；每个 progress 帧不再触发从 cursor=0 的分页；一次事件批次最多一次 DOM 更新。请求减少比例取决于进度帧数，不在此承诺固定百分比。
- **实施成本：** 2–5 人日。
- **风险与验证：** 确保 SSE gap 补全、重连、事件排序、搜索和展开状态不丢失。使用 1k/10k/30k 事件 fixture 记录网络请求数、渲染耗时、长任务和 DOM 节点数；验收请求量随新增事件增量增长，而非随进度帧重复乘以历史页数。
- **排序理由：** 属于热路径，发生在正常流式交互中，影响越长的任务越明显。

### P1-05 SessionStore 锁顺序反转存在死锁路径

- **问题描述：** checkpoint 路径先持有 session mutex，再进入 `commit()` 获取 journal mutex；`delete_cascade()` 先获取 sessions 全局锁和 journal 锁，之后再等待 session mutex。并发 checkpoint 与删除同一会话时可能形成循环等待，影响共享 journal 与后续会话操作。
- **代码位置：** [`session.rs:503`](../crates/samho-core/src/session.rs#L503)、[`session.rs:618`](../crates/samho-core/src/session.rs#L618)、[`session.rs:635`](../crates/samho-core/src/session.rs#L635)、[`session.rs:657`](../crates/samho-core/src/session.rs#L657)。
- **判断依据：** 静态锁顺序分析，未做并发故障注入。已运行的 135 项测试通过，但现有测试不代表覆盖这个并发交错。
- **影响范围：** 删除会话、任务 checkpoint、共享 session store 的请求可能挂起；若 journal 全局锁被阻塞，影响可超出单个会话。
- **修改建议：** 先定义并文档化唯一锁顺序；优先避免在持有全局 sessions/journal 锁时 await 单个 session 锁，采用快照/版本检查或每会话操作协调器。检查所有 commit/fork/rename/archive/delete 路径的一致顺序。
- **预期收益：** 已实测收益：无。预期定性收益：消除该锁环死锁路径，改善删除与任务并发时的可用性。
- **实施成本：** 1–3 人日，需谨慎处理快照一致性与 journal 原子提交。
- **风险与验证：** 修改存储并发顺序可能破坏删除/分支语义。加可控 barrier 并发测试循环执行 checkpoint 与级联删除，并设置超时；验证 journal checksum、恢复和未完成工具调用语义。
- **排序理由：** 低频但可使关键持久化操作永久挂起，影响稳定性和恢复。

### P1-06 新建动态工作区时只持久化新条目

- **问题描述：** 动态工作区 create 将记录表以 `std::slice::from_ref(&record)` 传给 `persist()`，文件被覆盖为只含新工作区的一项。先前创建的动态 workspace 仍在当前进程 map 中，但服务重启后无法从 `workspaces.json` 恢复注册。其目录/数据未删除，属于注册信息丢失而非目录内容被删除。
- **代码位置：** [`hub.rs:274`](../crates/samho-cli/src/hub.rs#L274)、[`hub.rs:284`](../crates/samho-cli/src/hub.rs#L284)、[`hub.rs:290`](../crates/samho-cli/src/hub.rs#L290)、[`hub.rs:69`](../crates/samho-cli/src/hub.rs#L69)。
- **判断依据：** `persist()` 会把传入 slice 全量序列化并原子替换注册文件；create 传入只含当前 record 的 slice。现有测试覆盖持久化单条 roundtrip，但未覆盖连续创建多条后重启。
- **影响范围：** 使用动态工作区多项目管理和服务重启的用户。
- **修改建议：** 在注册表写锁内生成完整 `hub_record_list` 快照并持久化；明确持久化失败时是否回滚内存状态，避免 API 返回成功但重启丢注册。必要时使用 generation/version 乐观并发避免并发 create/rename/remove 互相覆盖。
- **预期收益：** 已实测收益：未运行多条重启复测。估算/定性收益：连续新增的全部 workspace 元数据可跨重启保留。
- **实施成本：** 0.5–1.5 人日。
- **风险与验证：** create/rename/remove 并发时要保证快照不会丢更新；测试创建 3 个、改名 1 个、删除 1 个、重启后恰好恢复剩余 2 个且路径和 data_dir 正确。
- **排序理由：** 直接破坏已暴露的动态工作区持久化功能，修复简单且可明确验收。

### P1-07 工作区加载和任务提交均缺少请求代际/提交锁

- **问题描述：** 工作区切换的多个 API Promise 返回后直接更新全局 state，没有 workspace generation 校验；用户快速切换时，旧工作区响应可覆盖当前状态。Composer 在 `/api/run` 完成前没有设置提交锁；连续按 Enter 可以发送多个请求，后返回任务覆盖 `state.task`，失败恢复还可能覆盖新输入。
- **代码位置：** [`main.js:442`](../web/js/main.js#L442)、[`main.js:476`](../web/js/main.js#L476)、[`main.js:499`](../web/js/main.js#L499)、[`main.js:346`](../web/js/main.js#L346)、[`main.js:367`](../web/js/main.js#L367)、[`composer.js:95`](../web/js/composer.js#L95)、[`composer.js:116`](../web/js/composer.js#L116)。
- **判断依据：** 静态异步状态追踪。尚未用延迟/乱序响应 fixture 复现。
- **影响范围：** 多 workspace 用户的数据视图一致性、任务追踪/取消能力和草稿内容。
- **修改建议：** 每次 workspace load 增加递增 generation/token，在每个响应提交 state 前检查其 generation 和 workspace ID；必要时用 AbortController。任务提交在 API 请求发出前置 pending 锁，成功后转 running，失败只在输入内容未变化时恢复原 prompt。
- **预期收益：** 已实测收益：无。预期定性收益：避免旧响应覆盖新状态、重复启动未被 UI 追踪的任务和草稿丢失。
- **实施成本：** 1–2 人日。
- **风险与验证：** 需要区分请求失败、网络超时但服务端已接受、切换后任务仍运行等状态。用可控乱序 API fixture 验证 A→B 切换只显示 B，重复提交期间最多一个新任务，失败恢复不覆盖用户新文本。
- **排序理由：** 正常用户快速操作即可进入竞态，可能造成任务不可取消或数据视图错乱。

### P1-08 窄屏隐藏侧栏且没有导航替代入口

- **问题描述：** 820px 以下 CSS 隐藏 `.sidebar`，顶栏导航也隐藏；页面没有侧栏开关或会话/工作区抽屉。375px 实测无横向溢出，但侧栏 `display:none`、导航树从可访问页面消失。移动端无法切换历史会话或 workspace。
- **代码位置：** [`app.css:1395`](../web/app.css#L1395)、[`index.html:17`](../web/index.html#L17)、[`index.html:28`](../web/index.html#L28)。
- **判断依据：** 已在本机 release 服务和浏览器中以 375×812 视口实测；DOM snapshot 不含会话侧栏，浏览器计算样式显示 `display:none`，`documentWidth=375`。
- **影响范围：** 手机及小屏平板的会话管理和多工作区工作流。
- **修改建议：** 提供顶栏导航按钮，打开带遮罩的侧栏抽屉；设置 `aria-expanded`、`aria-controls`，支持 Escape 关闭、焦点进入/返回、触摸和键盘操作。
- **预期收益：** 已实测收益：确认当前侧栏在 375px 不可见。预期收益：移动端重新可访问会话/工作区；横向溢出保持为 0。
- **实施成本：** 1–2 人日。
- **风险与验证：** 不应遮挡 composer、设备抽屉或系统安全区域。验收 375、560、820px，触摸/键盘打开会话和切换 workspace，关闭后焦点可预测。
- **排序理由：** 是确定的设备适配缺陷，导致核心导航能力在窄屏不可用。

### P2-09 大文件、目录、代理和 Git 子进程缺少执行前资源上限

- **问题描述：** `/api/file` 对整个文件 `read()` 后才截断到 1 MiB；`/api/file/raw` 完整读入；文件/设备目录枚举不限制条目数；代理先将完整上游 body 读入内存后才检查 4 MiB；Git `.output()` 缓冲完整 stdout/stderr，diff 截断发生在子进程输出缓冲之后。并发大响应可显著增大内存。
- **代码位置：** [`panels.rs:20`](../crates/samho-cli/src/panels.rs#L20)、[`panels.rs:99`](../crates/samho-cli/src/panels.rs#L99)、[`panels.rs:150`](../crates/samho-cli/src/panels.rs#L150)、[`panels.rs:176`](../crates/samho-cli/src/panels.rs#L176)、[`panels.rs:290`](../crates/samho-cli/src/panels.rs#L290)、[`panels.rs:397`](../crates/samho-cli/src/panels.rs#L397)、[`panels.rs:809`](../crates/samho-cli/src/panels.rs#L809)。
- **判断依据：** 静态代码确认分配/截断顺序和无界枚举；没有运行资源耗尽压力测试。HTTP request body 的 1 MiB 限制不能限制上游响应和本地读取文件的大小。
- **影响范围：** 本地面板在大仓库、大文件、慢/恶意上游或并发调用下的 CPU、内存和请求延迟。
- **修改建议：** 文本文件按 `take(cap+1)` 流式读取；raw 资源限制最大 size 或以流式 body 返回；目录分页/条目上限；代理检查 Content-Length 并按 chunk 计数、超限即取消，同时限制并发；Git 子进程使用超时、管道流式采集和 stdout/stderr 上限。对大型操作返回清晰 413/超限信息。
- **预期收益：** 已实测收益：无。预期定性收益：峰值内存受配置预算约束，超大操作有可控失败，而不是先完整分配后拒绝。
- **实施成本：** 2–4 人日。
- **风险与验证：** 大文件预览/下载行为可能改变；覆盖 cap-1、cap、cap+1、缺 Content-Length、慢响应、超时、超大 Git 输出和并发请求，测量 peak RSS 和服务响应。
- **排序理由：** 未认证但限 loopback 的服务仍会处理本机大资源；属于明确的稳定性和资源耗尽改善，风险低于数据边界问题。

### P2-10 动态 workspace 的 SSE shutdown token 与 PTY 回收不完整

- **问题描述：** 根 `AppState::stop()` 只取消根状态的 token；动态 workspace 使用各自 AppState token。服务优雅退出要等 Axum 当前连接完成，而动态工作区 SSE 等待自身取消；`HubState::shutdown_all()` 又在 `serve()` 返回后才调用，可能形成关停等待。另 `workspace_remove()` 摘除 entry 后只 shutdown runtime，没有关闭其 `TermHub` 中的 PTY。
- **代码位置：** [`server.rs:67`](../crates/samho-cli/src/server.rs#L67)、[`server.rs:591`](../crates/samho-cli/src/server.rs#L591)、[`server.rs:601`](../crates/samho-cli/src/server.rs#L601)、[`hub.rs:146`](../crates/samho-cli/src/hub.rs#L146)、[`hub.rs:366`](../crates/samho-cli/src/hub.rs#L366)、[`term.rs:231`](../crates/samho-cli/src/term.rs#L231)。
- **判断依据：** 静态生命周期追踪；没有在打开动态 SSE/PTY 时做服务退出故障注入。
- **影响范围：** 有长期 SSE 或终端会话时的服务重启、workspace 移除和子进程资源回收。
- **修改建议：** 用 root shutdown token 的 child token 统一创建所有 workspace state；优雅关停阶段先取消所有 workspace/task SSE，再等待有限 grace period，之后关 PTY、运行时和 MCP。workspace remove 时先停止接受新请求、取消任务、关闭 PTY，最后 shutdown runtime 并持久化结果。
- **预期收益：** 已实测收益：无。预期收益：关停时间有上界，不遗留 PTY/子进程。
- **实施成本：** 1–3 人日。
- **风险与验证：** 先关闭连接可能影响客户端重连体验，需保持明确 shutdown 语义。自动化测试打开动态 SSE 和 PTY 后请求移除/服务退出，断言 5 秒内完成、子进程退出、journal 可恢复。
- **排序理由：** 依赖动态工作区的长期连接/终端场景才触发，影响生产关停可靠性，优先级略低于正常任务流问题。

### P2-11 durable event sync 与 session checkpoint 随历史增长

- **问题描述：** 默认 `durable_logs=true`，事件追加在持锁路径中每条 `flush` 后执行 `sync_data()`；会话 checkpoint 将 session 序列化/脱敏、克隆已提交消息，再扫描共同前缀并复制增量。Agent 在模型响应、工具观察等边界多次 checkpoint。工具密集任务和长会话可能因磁盘同步与历史复制增长而变慢。
- **代码位置：** [`config.rs:254`](../crates/samho-core/src/config.rs#L254)、[`events.rs:464`](../crates/samho-core/src/events.rs#L464)、[`events.rs:481`](../crates/samho-core/src/events.rs#L481)、[`session.rs:503`](../crates/samho-core/src/session.rs#L503)、[`session.rs:507`](../crates/samho-core/src/session.rs#L507)、[`agent.rs:870`](../crates/samho-core/src/agent.rs#L870)、[`agent.rs:1212`](../crates/samho-core/src/agent.rs#L1212)。
- **判断依据：** 代码路径确认；本轮 Demo 基准不含工具调用，且没有区分 storage/checkpoint 阶段，故不能证明该优化对当前样本有收益。
- **影响范围：** 慢磁盘、长会话、事件密集和工具调用密集任务的任务延迟及 journal 锁持有时间。
- **修改建议：** 首先为事件 append、session commit、provider、tool execution 分段埋点；使用借用切片减少 base clone，评估维护 committed length/version 的增量前缀比较。若做 group commit，必须提供明确 durability window 和故障保证，保持 strict-sync 模式可选，不能静默降低持久性。
- **预期收益：** 已实测收益：无。估算：长会话 checkpoint 的优化上限取决于消息长度和提交次数；未做 profiling 前不承诺百分比。group commit 可减少 sync 次数，但会扩大崩溃时最后一批事件的丢失窗口。
- **实施成本：** profiling 1–2 人日，优化 2–5 人日，group commit 还需故障恢复验证。
- **风险与验证：** 可能影响已承诺的恢复点。对 1k/10k/100k 消息和工具密集 fixture 测 p50/p95、分配量、锁持有时间、sync 次数；强杀/重启后校验 checksum、序号与 session 内容。
- **排序理由：** 已有持久化保证优先于速度，不应仅凭 Demo 基准改弱；先补齐关键路径测量再优化。

### P2-12 Smoke/benchmark 脚本对 Demo 文案和 Windows UTF-8 解码过于脆弱

- **问题描述：** `smoke_runtime.py` 和 benchmark 脚本通过输出文本包含精确 `[DEMO simulation]` 判定成功。当前 Demo 输出实际以 `【模拟模式】[DEMO simulation / demo-fast]` 开头，结构仍成功但旧断言不匹配。本轮 Python smoke 因 marker 断言失败；现成 PowerShell benchmark 运行成功退出子进程，但 Windows PowerShell 5.1 的 stdout 解码造成中文 mojibake，JSON 解析失败。`smoke-web.ps1` 仍期望 `app.js`/`style.css`，当前入口使用 `/js/main.js` 和 `/app.css`。
- **代码位置：** [`smoke_runtime.py:89`](../scripts/smoke_runtime.py#L89)、[`benchmark-runtime.ps1:130`](../scripts/benchmark-runtime.ps1#L130)、[`benchmark-runtime.ps1:145`](../scripts/benchmark-runtime.ps1#L145)、[`benchmark-runtime.ps1:156`](../scripts/benchmark-runtime.ps1#L156)、[`smoke-web.ps1:90`](../scripts/smoke-web.ps1#L90)、[`smoke-web.ps1:91`](../scripts/smoke-web.ps1#L91)、[`index.html:8`](../web/index.html#L8)、[`index.html:13`](../web/index.html#L13)。
- **判断依据：** 已在当前 release 二进制上实跑：Python smoke 到任务完成，因前缀断言失败；benchmark 产生失败报告到系统临时路径，退出码 0、任务状态实际 done，但 PowerShell 解析显示 `unparseable` 且 stdout 中文乱码。未覆盖仓库报告文件。
- **影响范围：** 本地发布验收、持续集成和后续性能对比可信度。
- **修改建议：** 测试 JSON 的 `status`、`model_id`、simulation 字段或不变的结构化字段，不检查自然语言完整前缀。PowerShell 进程显式使用 UTF-8 stdout/stderr decoder，或统一用 Python/Node UTF-8 驱动子进程。Smoke 资源清单从实际 HTML/服务端 asset manifest 验证，避免硬编码旧文件名；报告默认使用唯一临时输出。
- **预期收益：** 已实测收益：本轮确认测试 harness 失败而当前服务任务完成。预期收益：减少假阴性和版本升级后的脚本失效，恢复可重复验收。
- **实施成本：** 0.5–2 人日。
- **风险与验证：** 字符编码需覆盖 PowerShell 5.1/7；验收三种脚本在当前 ESM 资源和本地化 Demo 输出下均通过，并证明不会覆盖 `reports/` 既有产物。
- **排序理由：** 会误报核心功能失败，也使基准不可复现，需在下一次性能基线建立前修好。

### P2-13 CI 未验证 MSRV，Actions 与依赖公告检查不完整

- **问题描述：** Workspace 声明 Rust 1.88，但 CI matrix 安装 stable；没有单独的 1.88 check。GitHub Actions 依赖 tag（如 `@v4`、`@stable`）而非 commit SHA 固定；workflow 中没有依赖 advisory 扫描。`cargo audit` 本机不可用，因此本轮无法给出已知漏洞清单。
- **代码位置：** [`Cargo.toml:9`](../Cargo.toml#L9)、[`.github/workflows/ci.yml:13`](../.github/workflows/ci.yml#L13)、[`.github/workflows/ci.yml:18`](../.github/workflows/ci.yml#L18)、[`.github/workflows/ci.yml:19`](../.github/workflows/ci.yml#L19)、[`.github/workflows/ci.yml:32`](../.github/workflows/ci.yml#L32)。
- **判断依据：** 静态 CI 配置审查；未实际运行 Rust 1.88、actionlint 或 advisory DB 检查。
- **影响范围：** 声明的最低编译器支持、供应链可复现性及依赖漏洞发现时效。
- **修改建议：** 添加 Rust 1.88 `cargo check/test --locked` job；对第三方 Actions pin 完整 SHA 并用 Dependabot 更新；加入定期 `cargo audit` 或等价 OSV 扫描，明确忽略项和责任人。对 WASM/跨平台依赖分别按 target 扫描。
- **预期收益：** 已实测收益：无。预期收益：发布前发现 MSRV 破坏和公开 advisory；固定 SHA 降低 tag 被移动的供应链风险。
- **实施成本：** 0.5–1.5 人日，持续的 CI 维护成本小。
- **风险与验证：** SHA pin 更新略繁琐，MSRV job 可能暴露真实兼容问题。验收 CI 对 rustc 1.88 和 stable 均通过；审计输出有版本/日期并对未处理高危项失败。
- **排序理由：** 当前没有发现已知漏洞，但这是低成本基础防线，适合近期完善。

### P2-14 屏幕阅读器会对累计流式全文重复播报

- **问题描述：** 整个 live turn section 标记 `aria-live="polite"`，每轮更新先 `replaceChildren()`，随后重新插入累计模型回复；屏幕阅读器可能持续重播全部增长文本，而不仅播报新内容。
- **代码位置：** [`timeline/render.js:65`](../web/js/timeline/render.js#L65)、[`timeline/render.js:67`](../web/js/timeline/render.js#L67)、[`timeline/render.js:76`](../web/js/timeline/render.js#L76)、[`main.js:244`](../web/js/main.js#L244)。
- **判断依据：** 静态 DOM/ARIA 更新路径；本轮没有屏幕阅读器动态测试。
- **影响范围：** 使用 NVDA、Narrator、VoiceOver 等辅助技术的用户，长回答尤其明显。
- **修改建议：** 正文从 live region 移除；用单独、节流的 `role=status` 节点播报开始/工具执行/完成/失败摘要，避免逐 token announce。确认焦点不因渲染移动。
- **预期收益：** 已实测收益：无。预期收益：减少重复语音内容，保持重要状态可感知。
- **实施成本：** 0.5–1 人日。
- **风险与验证：** 不可完全用自动 DOM 测试替代真实屏幕阅读器。添加 ARIA contract 测试并用 NVDA/Windows 和至少一个浏览器手工验收长回答。
- **排序理由：** 不影响视觉用户，但会显著损害特定用户的长任务使用体验。

### P3-15 项目持久化失败后内存与磁盘状态分离

- **问题描述：** ProjectStore 的 create/update/delete 先修改内存 map 再持久化；写盘失败时 API 返回错误，但内存变更未回滚。启动加载用 `if let Ok(bytes)` 把任何读取错误都当作无项目，而不只把 NotFound 视作空集合。
- **代码位置：** [`projects.rs:55`](../crates/samho-core/src/projects.rs#L55)、[`projects.rs:58`](../crates/samho-core/src/projects.rs#L58)、[`projects.rs:80`](../crates/samho-core/src/projects.rs#L80)、[`projects.rs:93`](../crates/samho-core/src/projects.rs#L93)、[`projects.rs:104`](../crates/samho-core/src/projects.rs#L104)、[`projects.rs:122`](../crates/samho-core/src/projects.rs#L122)。
- **判断依据：** 静态追踪；现有 storage lifecycle 测试覆盖其他 journal 写失败路径，但项目清单路径需单独测试。
- **影响范围：** 磁盘满、权限错误或只读数据目录情况下的项目列表一致性和重启行为。
- **修改建议：** 先构造候选 map/clone 并成功原子持久化后替换内存状态，或写失败回滚；启动时仅 NotFound 返回空，其余 IO 错误显式报告并保留原始数据。
- **预期收益：** 已实测收益：无。预期收益：API 错误后内存/磁盘状态一致，损坏/权限问题不再伪装为空项目。
- **实施成本：** 0.5–1.5 人日。
- **风险与验证：** 原子 rename 与 Windows 文件替换差异；模拟权限拒绝、磁盘写失败、无文件、损坏 JSON，确认状态不变且错误可定位。
- **排序理由：** 低频持久化错误场景，影响范围小于 session journal，因此列为持续改进。

### P3-16 任务索引按条目数回收但无字节预算

- **问题描述：** 任务快照索引到 1024 条才清理完成项；结果对象/错误体没有字节预算，`GET /api/tasks` 会克隆并返回所有快照。大量长输出任务会消耗常驻内存并产生大响应。
- **代码位置：** [`server.rs:961`](../crates/samho-cli/src/server.rs#L961)、[`server.rs:1006`](../crates/samho-cli/src/server.rs#L1006)、[`server.rs:1052`](../crates/samho-cli/src/server.rs#L1052)。
- **判断依据：** 静态代码；没有长期运行/大量结果 soak 数据。
- **影响范围：** 长时间不重启的本地服务及大量大结果/错误响应。
- **修改建议：** 设置总字节估算预算、按完成时间 TTL/LRU 回收、列表分页；将大结果保存在 session/trace 中，任务列表保留摘要并提供按 ID 查询。定义运行中任务不被回收的规则。
- **预期收益：** 已实测收益：无。估算收益：常驻索引由无界字节增长变为配置预算上限；具体上限应结合任务负载配置。
- **实施成本：** 1–2 人日。
- **风险与验证：** 前端任务看板可能依赖历史完整结果。用 10k 个不同大小任务 fixture 测内存上界、分页正确性、运行中任务保留和客户端兼容。
- **排序理由：** 有数量阈值但不常见；属于长期运行的稳健性改进。

### P3-17 验收文档基线和资源体积需要可重复管理

- **问题描述：** README 记录 93 项 Rust 测试，但当前 workspace 实际测试 135 项；实现状态文档为 135 项且日期标为 2026-10-22。历史报告里 release 二进制为 8,461,824 bytes，新构建为 14,310,912 bytes，但代码提交、依赖与构建条件不同，不能据此宣称增长。工作台没有传统静态 bundler，面板 ESM 主要随主入口加载，但终端 xterm 已惰性加载；目前无浏览器冷启动/资源时序基线证明还需要进一步拆包。
- **代码位置：** [`README.md:62`](../README.md#L62)、[`implementation-status.md:18`](../docs/implementation-status.md#L18)、[`Cargo.toml:29`](../Cargo.toml#L29)、[`terminal.js:18`](../web/js/panels/terminal.js#L18)。
- **判断依据：** 本轮 `cargo test --workspace --locked` 实测 135 项通过；`wc -c` 实测 release exe 14,310,912 bytes，xterm 289,441 bytes、canvas addon 94,894 bytes、文档数据 117,157 bytes。没有浏览器网络面板资源计时数据。历史 binary 不同 commit，非同条件对照。
- **影响范围：** 维护者对项目健康度、升级风险和启动/产物回归的判断。
- **修改建议：** 用 CI 生成测试数/版本/构建体积摘要，统一 README 和实现状态基线；将 benchmark JSON 强制记录 commit/hash/rustc/target/features/配置；前端先采集真实页面 cold/warm resource timing，再依据慢模块/体积决定是否 dynamic import 或内容哈希缓存。当前 loopback 服务选择 `no-store` 有版本一致性理由，不建议无测量改成长期缓存。
- **预期收益：** 已实测收益：当前测试数和当前 binary/静态资源字节数已记录。估算收益：建立可比基线后才能量化体积或启动变化；不预先承诺拆包百分比。
- **实施成本：** 1–3 人日，取决于 CI 报告自动化范围。
- **风险与验证：** 不同 OS 构建产物不可直接互比；统一 Windows 同机、同 commit、同 profile、同依赖缓存条件，至少 20 样本，报告 p50/p95 和 binary SHA。前端报告分别记录 cold load、module 请求数、DOM ready 与首次可交互时间。
- **排序理由：** 不属于当下功能故障，但能避免未来优化建立在过时或不可比数据上。

## 实测结果

| 检查 | 结果 | 说明 |
|---|---|---|
| `cargo fmt --all -- --check` | 通过 | 当前 Rust 格式检查 |
| `cargo test --workspace --locked` | **135 通过，0 失败** | CLI 38、core unit 19、Agent 6、contracts 15、Facet 6、MCP 5、MCP optional 4、OAuth 3、Provider streaming 15、runtime semantics 10、Skill policy 5、storage lifecycle 9；doc tests 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 通过 | 无 warning |
| `node --test scripts/test-chinese-pages.mjs` | **13/13 通过** | 覆盖中文页面/计划页，不覆盖全部工作台交互 |
| `node scripts/check-web-esm.mjs` | 通过语法检查 | 共检查脚本列出的 12 个 ESM；部分模块因没有 DOM 环境出现被脚本预期处理的 runtime 报错，故不是完整浏览器运行测试 |
| `cargo build --release --locked -p samho` | 通过 | 本次约 7m46s；Windows release exe 14,310,912 bytes |
| 当前二进制本地 API/任务 | 通过 | 隔离临时 workspace/data；health、`/api/run`、task poll 均通过，使用 Demo，无外部模型/付费调用 |
| 当前二进制 Demo 基准 | **20 样本通过** | 3 次预热；进程到完成 p50 254.154ms、p95 271.439ms；峰值工作集 p50 12,060,672 bytes、p95 12,107,776 bytes。指标定义见报告开头 |
| 浏览器桌面工作台 | 通过加载 | API 连接、会话侧栏、任务输入、事件流和检查面板可见 |
| 浏览器 375×812 | 缺陷确认 | 横向宽度等于视口，但会话侧栏被 `display:none` 隐藏且没有替代导航 |
| `scripts/smoke_runtime.py` | 测试脚本失败 | 当前真实任务已完成，但旧的 `[DEMO simulation]` 字符串断言不匹配现有本地化前缀；不是服务任务执行失败 |
| `scripts/benchmark-runtime.ps1` | 测试脚本失败 | 子进程返回 0、Demo 任务完成；Windows PowerShell 5.1 stdout 中文被错误解码，JSON parse 失败。失败报告写在系统临时路径，没有覆盖仓库报告 |
| 符号链接越界动态测试 | 未能运行 | Windows runner 不授予创建符号链接的特权（WinError 1314）；结论来自静态检查，尚需有权限的隔离 fixture 复现 |
| `cargo audit` | 未运行 | 本机未安装 cargo-audit；本报告不声称依赖无漏洞 |

本轮启动的本机服务使用唯一临时数据目录，已关闭并删除。浏览器测试标签已关闭。仓库工作树保持干净；未修改业务代码、既有报告、数据库或用户数据。本次没有开展真实 Provider、外部网络、长期 soak、跨平台 CI 和并发死锁故障注入。

## 分阶段整改路线

### 阶段 1：立即降低安全和数据风险（建议先做）

范围：P1-01、P1-02、P1-03。明确 shell/write 授权边界；修复符号链接写入；将轨迹高亮改为纯 DOM 文本节点。

**验收门槛：** 默认配置安全语义与文档一致；外部符号链接/悬空链接写入被拒绝且哨兵未改变；轨迹恶意 HTML fixture 不产生可执行或伪造节点；现有 135 项 Rust 测试和前端契约测试继续通过。

### 阶段 2：修复核心交互竞态和热路径

范围：P1-04、P1-05、P1-07、P1-08。先修轨迹加载/渲染重复工作、SessionStore 锁顺序、异步响应 generation 和 submit lock，再补移动导航抽屉。

**验收门槛：** 延迟/乱序测试通过；progress 帧不会重新从 cursor=0 拉取完整轨迹；并发 checkpoint/delete 不超时且 journal 可重放；一个 pending submit 不能启动多个未跟踪任务；375/560/820px 能管理会话与 workspace，无水平溢出。

### 阶段 3：动态工作区生命周期与资源限额

范围：P1-06、P2-09、P2-10、P3-16。先全量快照持久化并修关停，再对文件/代理/Git/目录 API 增加流式限制和任务索引字节预算。

**验收门槛：** 多 workspace 创建/改名/删除/重启结果一致；开启动态 SSE 和 PTY 后服务可在约定 grace period 内退出且无残留子进程；大响应超限前停止读取；压力场景 RSS 有明确上限。

### 阶段 4：建立可信性能和供应链基线

范围：P2-11、P2-12、P2-13、P2-14、P3-15、P3-17。修复 UTF-8/契约脆弱的测试脚本，添加 MSRV 与依赖检查，随后以长会话、工具密集、事件密集、长轨迹分别 profile。

**验收门槛：** 本地和 CI 的 smoke/benchmark 均以结构化字段通过；Rust 1.88 与 stable 构建成功；依赖审计产物可追溯；至少 20 次固定条件性能样本给出 p50/p95、峰值 RSS、配置与 commit；存储优化必须通过强杀/重启恢复测试。

## 收益口径与限制

报告没有把估算写成已实现收益。只有当前 Demo 进程任务基线、release binary/资源字节数、浏览器窄屏表现和测试命令结果属于本轮实测。shell 权限、symlink 写保护、轨迹增量加载、锁顺序和持久化策略的收益均是建议目标，需实现后依照本报告验收步骤重新测量。真实模型延迟由供应商、模型和网络主导，不能用 Demo 基线外推。
