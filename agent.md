# agent.md — Samho 工程与智能体协作完整参考

> 适用版本：`1.0.0`（2026-11-15，`master` 分支）  
> 读者：人类开发者与在本仓库工作的 AI 编码代理。  
> 事实优先级：**源码与测试 > 本文件 > README / docs/**。发现冲突时先读源码，再更新本文件与相关文档。  
> 本文件只描述当前真实实现；未实测的平台、真实供应商质量与官方 conformance 不写成“已通过”。

---

## 目录

1. [一分钟上手](#1-一分钟上手)
2. [项目定位与硬边界](#2-项目定位与硬边界)
3. [仓库地图与运行期数据](#3-仓库地图与运行期数据)
4. [构建、运行与验证](#4-构建运行与验证)
5. [架构总览](#5-架构总览)
6. [配置参考](#6-配置参考)
7. [运行时核心（samho-core）](#7-运行时核心samho-core)
8. [CLI 与 HTTP 服务（samho-cli）](#8-cli-与-http-服务samho-cli)
9. [Web GUI](#9-web-gui)
10. [测试、CI 与脚本](#10-测试ci-与脚本)
11. [常见开发任务](#11-常见开发任务)
12. [编码规范与不变量](#12-编码规范与不变量)
13. [已知问题、边界与排障](#13-已知问题边界与排障)
14. [文档地图与维护规则](#14-文档地图与维护规则)

---

## 1. 一分钟上手

```powershell
# 构建（Windows 产物 target/release/samho.exe）
cargo build --release --locked -p samho

# 启动共享服务：默认 0.0.0.0:7432，本机与局域网都可访问
cargo run -p samho -- serve

# 浏览器打开工作台
# http://127.0.0.1:7432

# 另开终端连接同一服务（TUI 是纯 HTTP 客户端，不直接开数据目录）
cargo run -p samho -- tui --url http://127.0.0.1:7432

# 不启动服务，直接用 CLI 跑一条任务（会独占 data_dir）
cargo run -p samho -- run "/demo read README.md" --complexity 2 --json

# 提交前验证（全部必须通过）
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
node --test scripts/test-workbench-contracts.mjs
node scripts/check-web-esm.mjs

# 改了 web/app（React 前端）后：先构建再编译 Rust（产物 web/dist 被 include_str! 内嵌并提交）
npm ci --prefix web/app
npm --prefix web/app run build
```

默认配置只有 **Demo 模拟器**，输出会明确标注 `[DEMO simulation]`。真实模型需要配置 `chat_completions` 或 `anthropic` Provider。

**最重要的三个约束**：

1. 一个 `data_dir` 同时只能被一个进程持有（`runtime.lock` 排他锁）。服务运行时，`run/inspect/replay/fork` 要用另一个 `data_dir`，或改用 TUI/Web。
2. Web/TUI 不维护第二套 Agent 状态机，全部走同一个 HTTP 服务。
3. 服务**没有身份认证**，默认监听 `0.0.0.0`；只应在可信局域网使用。

---

## 2. 项目定位与硬边界

Samho 是一个**本地优先的 Agent 运行时**：以 Rust 从零实现 Agent 循环、流式输出、工具调用、会话树、MCP、Skill 等执行语义，并围绕运行时实体管理做了自有设计：

- 统一 **Object** 身份、依赖拓扑、scope、lease 与热替换。
- 六级模型路由（nano / fast / balanced / reasoning / frontier / specialist）+ 可配置评分。
- 确定性上下文投影（摘要 + SHA-256 lineage），不改写原始会话。
- 请求前预算预留与结算，未知失败不退款。
- 单调序号 trace、span 树、脱敏、分页、SSE 与 Markdown 复盘导出。
- 项目制工作区：项目 / 文件夹 / 会话树，会话可挂终端与浏览器标签。
- 静态多工作区与运行时动态工作区，各自独立 data_dir、会话、预算与终端。
- 中文 Web 工作台与 TUI。

**明确不是**：

- 不是操作系统沙箱。`shell` 以服务进程的 OS 用户权限执行；`write` 只限制在 workspace 内，但不隔离进程。
- 不是多租户服务。无认证、无 CORS、无按用户授权；Host/Origin 检查只用于本地/局域网边界。
- 不提供真实模型质量结论。所有集成测试使用 Demo、trait provider 与本地 HTTP/stdio/Python fixture。

**版本信息**：

| 项 | 值 |
| --- | --- |
| workspace 版本 | `1.0.0` |
| Rust edition | 2024 |
| MSRV | 1.88（CI 独立 job 验证） |
| resolver | `"2"`（`Cargo.toml` 显式声明） |
| 许可证 | MIT |
| release profile | `lto = "thin"`、`codegen-units = 1`、`panic = "abort"` |
| 依赖要点 | axum 0.8、tokio、reqwest 0.13.5、rmcp `=3.5.1`、ratatui 0.30.2、portable-pty 0.8 |

---

## 3. 仓库地图与运行期数据

```text
samho/                            # 仓库根
├── Cargo.toml                      # workspace: crates/samho-core + crates/samho-cli
├── Cargo.lock
├── agent.md                        # 本文件（权威工程参考）
├── CLAUDE.md                       # 指向本文件（见 §14.4）
├── README.md                       # 面向用户的总览与启动说明
├── crates/
│   ├── samho-core/                 # 业务运行时库（无二进制）
│   │   ├── src/
│   │   │   ├── agent.rs            # AgentRuntime、任务生命周期、控制队列
│   │   │   ├── config.rs           # RuntimeConfig 与全部子配置
│   │   │   ├── model.rs            # ModelDescriptor、六级路由、评分
│   │   │   ├── provider.rs         # Provider trait、Chat Completions、Demo
│   │   │   ├── anthropic.rs        # Anthropic Messages API Provider
│   │   │   ├── tools.rs            # 内置工具、策略、路径边界、网络策略
│   │   │   ├── session.rs          # 会话 journal v2（file/sqlite）
│   │   │   ├── events.rs           # TraceEvent、EventLog、轮转、脱敏
│   │   │   ├── context.rs          # 确定性上下文投影与 lineage
│   │   │   ├── budget.rs           # 预算预留/结算账本
│   │   │   ├── object.rs           # ObjectRegistry 生命周期
│   │   │   ├── skills.rs           # SKILL.md 发现/校验/权限
│   │   │   ├── mcp.rs              # MCP 客户端（rmcp）
│   │   │   ├── mcp/oauth.rs        # 独立 OAuth PKCE 客户端（未接入运行时）
│   │   │   ├── facet.rs            # samho-facet/1 远程 Object 适配器
│   │   │   ├── projects.rs         # 项目存储（projects.json）
│   │   │   ├── attachments.rs      # 会话附属终端/浏览器（attachments.json）
│   │   │   ├── test_support.rs     # 测试端口 301–399 助手
│   │   │   └── lib.rs
│   │   └── tests/                  # 10 个集成测试文件 + fixtures/
│   └── samho-cli/                  # package 名 samho，二进制 samho
│       └── src/
│           ├── main.rs             # clap CLI：run/serve/tui/fork/inspect/config/replay
│           ├── server.rs           # Axum 路由、静态资源、中间件、任务索引
│           ├── tui/                # Ratatui 客户端（mod/api/sse/markdown/widgets + views/）
│           ├── panels.rs           # 文件/Git/浏览器代理后端
│           ├── panels.rs           # 文件/Git/浏览器代理后端
│           ├── term.rs             # PTY 池 + SSE
│           ├── hub.rs              # 动态工作区注册表与 /w/{id} 分发
│           └── roles.rs            # 任务类型 → 模型覆盖（RoleRouter）
├── web/                            # 编译期内嵌进二进制（include_str!）
│   ├── app/                        # 工作台源码：React 19 + TypeScript + Vite + Tailwind v4 + shadcn/ui
│   │   ├── src/components/ui/      # shadcn/ui 组件（new-york v4，导入 radix-ui）
│   │   ├── src/components/         # 顶栏/侧栏/对话/输入区/检查器/面板
│   │   └── src/lib/                # api、store（zustand）、controller、projection、markdown
│   ├── dist/                       # Vite 构建产物（固定文件名，提交进仓库）
│   └── vendor/xterm*.{js,css}      # 终端惰性加载
├── docs/                           # 11 份 Markdown
├── examples/
│   ├── samho.chat.toml             # OpenAI 兼容 Provider 模板
│   └── samho.anthropic.toml        # Anthropic Messages 网关完整示例
├── scripts/                        # 构建/冒烟/基准/契约/浏览器验收
├── reports/                        # 仅 .gitkeep；脚本按需生成报告
└── .github/workflows/ci.yml        # 三平台 + MSRV + audit
```

### 3.1 运行期数据文件（默认 `data_dir = ".samho"`）

| 文件 / 目录 | 写入方 | 内容与语义 |
| --- | --- | --- |
| `runtime.lock` | `AgentRuntime` | `fs2` 独占文件锁；第二个进程打开同一目录会失败 |
| `sessions.jsonl` | `SessionStore`（file） | 会话增量 journal，schema v2，SHA-256 校验链 |
| `sessions.sqlite` | `SessionStore`（sqlite） | 同上，WAL + `synchronous=FULL` |
| `sessions.legacy.jsonl` | 迁移 | 旧格式 journal 的原样备份（仅迁移时出现） |
| `events.jsonl` / `events.sqlite` | `EventLog` | trace 事件；文件模式支持轮转 |
| `events.jsonl.segment-{20位}` | 轮转 | 历史事件段；超过 `logs.max_archives` 删除最旧 |
| `budget.jsonl` | `BudgetTracker` | 累计用量快照（每行一个累计值，不是每任务增量） |
| `projects.json` | `ProjectStore` | 项目列表（名称、颜色、folders、entry、open） |
| `attachments.json` | `AttachmentStore` | `session_id → {terminal_ids, browser_tabs, updated_at}` |
| `model_roles.json` | `ModelRoles` | 任务层级 → 模型覆盖；`""` 表示跟随自动路由 |
| `workspaces.json` | `HubState` | 动态工作区注册表（id/name/path/data_dir） |
| `ui.json` | `PUT /api/ui` | 主题与快捷键；**当前启动时不回读**（见 §13） |
| `todos/{session_id}.json` | `todo_write` | 会话级任务清单 |
| `workspaces/{id}/` | 动态工作区 | 每个动态工作区独立的完整 data_dir |

### 3.2 生成物与内嵌物

- 工作台由 `web/app` 构建到 `web/dist`（`index.html` + `assets/app.js` + `assets/app.css`，单文件、无代码分割、无哈希），**提交进仓库**；CI 重新构建并 `git diff --exit-code -- web/dist` 校验同步。`.gitattributes` 固定 `web/dist` 为 LF。
- 所有 Web 资源通过 `include_str!` 编译进二进制；**改前端必须先 `npm --prefix web/app run build`，再 `cargo build` 才生效**。
- `STATIC_ASSETS` 共 6 项：`index.html`、`assets/app.js`、`assets/app.css` 与 3 个 xterm vendor 文件。Vite 配置固定产物文件名，新增前端代码不需要改该表。
- `reports/` 只有 `.gitkeep`；冒烟/基准/浏览器报告按需生成，不提交。

---

## 4. 构建、运行与验证

### 4.1 前置条件

- Rust ≥ 1.88（CI 用 stable，另有 1.88.0 MSRV job）。
- Python 3：MCP stdio 集成测试与 `scripts/*.py`。
- Node 22：前端构建（`web/app`，Vite 8 / TypeScript）与网页契约测试；浏览器验收脚本使用原生 WebSocket/CDP，需要本机 Chrome。
- Windows 上 `cargo test` 会真实启动 `git` 与 PowerShell/ConPTY 的测试；Linux/macOS 对应 `sh`。

### 4.2 CLI 命令

全局参数只有 `--config <PATH>`，可放在子命令前后。**没有默认配置文件发现机制**：不传 `--config` 就使用内置默认值。

| 命令 | 关键参数 | 行为 |
| --- | --- | --- |
| `samho run <PROMPT>` | `--session <ID>`、`--complexity 0..=10`、`--tier <NAME>`、`--intelligence-tier <ID>`、`--model <ID>`、`--skill <NAME>`（可重复）、`--json` | 本地直接开运行时执行一条任务。默认输出结果到 stdout，stderr 一行 `状态=… 会话=… 轨迹=… 模型=…`。Ctrl+C 取消任务但不退出进程。 |
| `samho serve` | `--bind <ADDR>` | 启动 HTTP 服务，默认取 `config.http_bind`（`0.0.0.0:7432`）。 |
| `samho tui` | `--url <URL>`，默认 `http://127.0.0.1:7432` | 纯 HTTP 客户端；**不读取 `--config`**。 |
| `samho fork <SESSION>` | `--cutoff <N>`、`--title <T>` | 从父会话创建分支；cutoff 默认父会话消息数。 |
| `samho inspect` | — | 打印脱敏后的 `runtime.state()` JSON。 |
| `samho config init [PATH]` | 默认 `samho.toml` | 若文件已存在则拒绝；写入本机 OpenAI 兼容网关模板（deepseek-v4.1-flash）。 |
| `samho replay [TRACE_ID]` | `--export <PATH>` | 无 `--export`：逐行打印事件；有则导出全部事件 JSONL。 |

`run` 的层级参数：

- 不传 `--complexity`：`complexity = 4` 且 `automatic = true`。
- 传了 `--complexity`：`automatic = false`，按给定复杂度路由。
- `--tier` 只接受内置六名：`nano|fast|balanced|reasoning|frontier|specialist`。
- `--intelligence-tier` 接受 `[routing.tiers]` 中自定义的 id。
- `--model` 直接指定物理模型，优先于层级。

退出码：成功 0；运行时错误 1；clap 参数错误 2。没有自定义退出码映射。

### 4.3 环境变量

| 变量 | 作用 |
| --- | --- |
| `SAMHO_HTTP_BIND` | 覆盖 `http_bind` |
| `SAMHO_WORKSPACE` | 覆盖 `workspace` |
| `SAMHO_DATA_DIR` | 覆盖 `data_dir` |
| `RUST_LOG` | tracing 过滤；默认 `warn`，日志写 **stderr** |
| `*_API_KEY` 等 | 由各 Provider 的 `api_key_env` 指名读取；密钥只放环境变量 |

覆盖顺序：内置默认值 → TOML → 上述三个 `SAMHO_*` 环境变量 → `validate()`。
`Redactor::from_env` 会收集名字含 `api_key/token/secret/password`（不区分大小写）且值长度 ≥8 的环境变量，用于全链路脱敏。

### 4.4 启动与访问

```powershell
samho serve                          # 0.0.0.0:7432，局域网可达
samho serve --bind 127.0.0.1:7432    # 仅本机
samho tui --url http://127.0.0.1:7432
```

服务启动时在 stdout 打印：

```text
Samho workspace: http://<本机地址>:7432
Samho LAN: http://<局域网 IPv4>:7432        # 监听非回环时，每个网卡一行
注意：服务没有身份认证，监听非回环地址时请只在可信局域网使用。
TUI client: samho tui --url http://<本机地址>:7432
```

Web 顶栏也会显示局域网地址，点击可复制。

### 4.5 验证门禁

```powershell
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked          # Windows 基线 162 项（Unix 多 1 项符号链接测试）
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
npm --prefix web/app run build           # tsc --noEmit + vite build → web/dist（改前端后必须）
node --test scripts/test-workbench-contracts.mjs   # 12 项
node scripts/check-web-esm.mjs           # 构建产物语法与单文件约束
cargo build --release --locked -p samho
python scripts/smoke_runtime.py --binary target/release/samho
```

`smoke_runtime.py` 默认拒绝覆盖 `reports/ci-smoke.json`，重跑加 `--force`；PowerShell 脚本对应 `-Force`。

### 4.6 测试端口策略

- 所有测试、fixture、OAuth 回调、浏览器 CDP、smoke 脚本只允许 **300–400**。
- Rust 测试端口由 `samho_core::test_support::bind_loopback()` 在 **301–399** 轮询。
- `smoke_runtime.py` 默认从 373 开始找空闲端口；`smoke-web.ps1` 默认 374。
- 生产运行不受此限制；“300–400”只是测试约束。

---

## 5. 架构总览

```text
┌──────────────────────── 客户端 ────────────────────────┐
│ Web 工作台 (/)   TUI (samho tui)   CLI (run/inspect/…) │
└───────────────┬─────────────────────────┬──────────────┘
                │ HTTP + SSE              │ 直接调用（仅 CLI）
        ┌───────▼────────┐        ┌───────▼────────┐
        │ Axum 本地服务   │        │  AgentRuntime  │
        │ 鉴权边界/静态页 │        │  （同一套核心） │
        └───────┬────────┘        └───────┬────────┘
                └────────────┬────────────┘
                             ▼
                    ┌─────────────────┐
                    │  AgentRuntime   │
                    │ 并发信号量/任务表│
                    └───┬───┬───┬───┬─┘
        ┌───────────────┘   │   │   └────────────────┐
        ▼                   ▼   ▼                    ▼
  ModelRouter         ToolRegistry            SessionStore / EventLog
  （六级/评分/角色）   （内置+自定义+MCP+Skill）  （v2 journal / SQLite）
        │                   │                         │
        ▼                   ▼                         ▼
  Provider            ObjectRegistry            BudgetTracker
  Demo / Chat /       （拓扑/租约/热替换）        （预留/结算）
  Anthropic
```

### 5.1 分层

| 层 | 位置 | 职责 |
| --- | --- | --- |
| 核心运行时 | `samho-core` | 配置、Agent 循环、路由、Provider、工具、会话、事件、预算、Object、Skill、MCP、Facet、项目、附件 |
| 适配层 | `samho-cli` | CLI、Axum HTTP/SSE、静态资源、TUI、文件/Git/终端/浏览器面板、动态工作区、角色路由 |
| 前端 | `web/` | React + Vite 工作台（`web/app` → `web/dist` 内嵌） |

`samho-core` 不依赖 axum/ratatui；`samho-cli` 只做装配与 I/O。

### 5.2 一次 Web 任务的数据流

1. `POST /api/run` 校验 prompt 与并发，返回 `202 {task_id,status:"running"}`。
2. 后台 `spawn` 调用 `AgentRuntime::run_with_task_id`（服务端生成 task id）。
3. Runtime 取会话、写 `task.start`、分类 prompt、`model.route` 选模型。
4. 每轮：上下文投影 → 预算预留 → Provider 调用（流式/非流式）→ 结算 → 工具批次 → 检查点。
5. 进度写入 `broadcast` 通道，`/api/live/stream` 以 SSE 推送累计快照（`revision` 单调）。
6. 事件写入 EventLog，`/api/events/stream` 推送增量，`/api/events/page` 支持游标回补。
7. 终态 `task.done|failed|cancelled`；会话状态同步提交；任务快照保存在服务内存索引。

### 5.3 并发模型

- 全局：`max_concurrent_tasks`（默认 4）个 tokio 信号量许可；排队中可取消。
- 每会话：同一时刻只允许 1 个活动任务（检查 `session.status == "running"`）。
- 工具：同一模型响应里的多个工具调用**严格顺序执行**，不是并行。
- 进度广播：容量 256；落后只收到 `Lagged`，SSE 发 `gap` 让客户端回补。
- 任务索引：最多 1024 条或 32 MiB 完成快照；运行中任务优先保留。重启后完成条目全部丢失（404 属预期）。

### 5.4 安全边界

- 默认绑定 `0.0.0.0:7432`，**无身份认证**。
- `Origin` 若存在必须严格等于 `http://{Host}`，否则 403。
- `Host` 只接受：`localhost`、回环 IPv4/IPv6、本机发现的局域网 IPv4、`COMPUTERNAME`/`HOSTNAME`，否则 403。
- 所有响应：`X-Content-Type-Options: nosniff`、`Cache-Control: no-store`、CSP（除已有 CSP 的代理响应）。
- 请求体统一上限 **1 MiB**（含 `PUT /api/file`、`PUT /api/ui`）。
- 无 CORS、无 CSRF token、无限流。
- 文件工具与文件面板限制在 workspace；`/api/browse*` 是**设备级**目录浏览（为工作区选择器服务），不受 workspace 约束。
- `shell` 不是沙箱；`webfetch`/MCP HTTP/Facet 受 `NetworkPolicy` 约束。

---

## 6. 配置参考

配置结构全部 `#[serde(default, deny_unknown_fields)]`：**未知键直接报错**。新增字段必须给默认值并加入 `validate()`。

### 6.1 顶层 `RuntimeConfig`

| 键 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `display_name` | string | `"Samho"` | 顶栏显示名，不依赖目录名 |
| `workspace` | path | `"."` | 工作区根；必须存在，工具与面板被限制在其内 |
| `data_dir` | path | `".samho"` | 运行期数据目录；创建并加独占锁 |
| `http_bind` | string | `"0.0.0.0:7432"` | `serve` 默认监听地址 |
| `system_prompt` | string | 见下 | 新会话首条 system 消息 |
| `max_turns` | u32 | `20` | Agent 循环轮次上限 |
| `max_concurrent_tasks` | usize | `4` | 全局并发任务数 |
| `request_timeout_ms` | u64 | `120000` | 单次 Provider 请求总超时 |
| `max_tool_output_bytes` | usize | `65536` | 单个工具结果截断阈值 |
| `log_content` | bool | `false` | false 时事件正文替换为占位文本；会话消息仍保存（已脱敏） |
| `durable_logs` | bool | `true` | 每次追加是否 `sync_data` / SQLite `FULL` |
| `skill_roots` | string[] | `[".agents/skills", ".pi/skills"]` | 相对路径按 workspace 解析 |
| `models` | array | 6 个 demo 模型 | 至少 1 个；见 §6.3 |
| `routing` | table | 见 §6.2 | 分类规则、层级定义、评分权重 |
| `tools` | table | 全开 | 工具开关与 shell 超时 |
| `mcp_servers` | array | `[]` | MCP server 配置 |
| `streaming` | bool | `true` | 是否使用流式 Provider |
| `stream_buffer` | usize | `32` | 流式事件通道容量，≤65536 |
| `context` | table | 见 §6.4 | 上下文投影 |
| `retry` | table | 见 §6.4 | 重试与重路由 |
| `budget` | table | 见 §6.4 | 预算上限 |
| `storage` | table | `backend="file"` | `file` 或 `sqlite` |
| `logs` | table | 64 MiB / 8 | 事件轮转 |
| `ui` | table | 见 §6.4 | 主题与快捷键默认值 |
| `workspaces` | array | `[]` | 静态多工作区 |
| `network` | table | 见 §6.5 | URL 准入策略 |
| `remote_facets` | array | `[]` | 远程 Facet 配置 |

默认 `system_prompt`：

```text
You are a coding assistant. Use tools when needed. Skills are workflow guidance. Report failures accurately.
```

### 6.2 `[routing]` 与层级

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `rules` | 3 条 | 关键词 → complexity，按顺序取**第一条**命中 |
| `tier_penalty` | `20.0` | 层级差惩罚 |
| `latency_weight` | `0.001` | 延迟惩罚权重 |
| `cost_weight` | `5.0` | 成本惩罚权重 |
| `output_reserve_tokens` | `1024` | 路由时的输出预留 |
| `tiers` | 六级 | 每项 `{id, rank, min_complexity, max_complexity, requires_reasoning}` |

默认规则：

| 关键词 | complexity |
| --- | --- |
| `architecture`、`proof`、`重写`、`架构`、`证明` | 9 |
| `debug`、`refactor`、`debugging`、`调试`、`重构` | 7 |
| `translate`、`summarize`、`翻译`、`摘要` | 2 |

默认层级：

| id | rank | 复杂度 | requires_reasoning |
| --- | --- | --- | --- |
| `nano` | 0 | 0–1 | false |
| `fast` | 1 | 2–3 | false |
| `balanced` | 2 | 4–5 | false |
| `reasoning` | 3 | 6–7 | true |
| `frontier` | 4 | 8–10 | false |
| `specialist` | 2 | 255–255（仅手动） | false |

`validate()` 要求层级 id 唯一、`min <= max`、非手动层级覆盖 0..10 且不重叠（合计恰好 11 个复杂度点）。

### 6.3 `[[models]]`

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `id` | `"demo-balanced"` | 本仓库内唯一标识 |
| `model` | `"demo"` | 传给供应商的物理模型名 |
| `provider` | `demo` | `demo` \| `chat_completions` \| `anthropic` \| `custom` |
| `tier` | `balanced` | 六级之一 |
| `base_url` | 无 | `chat_completions`/`anthropic` 必填 |
| `api_key_env` | 无 | 从该环境变量读密钥；为空表示无认证 |
| `context_window` | `128000` | 必须 >0 |
| `max_output_tokens` | `4096` | 必须 >0 且 ≤ context_window |
| `supports_tools` | `true` | 是否发送工具 schema |
| `supports_vision` | `false` | 路由过滤条件 |
| `supports_reasoning` | `false` | 路由过滤条件；影响预算预留 |
| `reasoning_effort` | 无 | 原样发给 Chat Completions；Anthropic 不发 |
| `reasoning_reserve_tokens` | 无 | 思考预留；缺省 = `max_output_tokens`（若支持推理） |
| `streaming` | `false` | 模型能力声明；Demo 实际仍按 `config.streaming` 流式 |
| `intelligence_tier` | 无 | 自定义层级 id；与 `tier` 冲突时报错 |
| `output_token_parameter` | `"max_tokens"` | Chat Completions 用 `max_tokens` 或 `max_completion_tokens` |
| `input_usd_per_million` | `0.0` | 成本估算 |
| `output_usd_per_million` | `0.0` | 成本估算 |
| `latency_ms` | `50` | 路由延迟惩罚提示 |
| `specialties` | `[]` | 任务 `specialty` 过滤 |
| `extra_body` | `{}` | 合并进请求体；不能覆盖运行时保留键 |

默认 6 个 demo 模型：`demo-nano`、`demo-fast`、`demo-balanced`、`demo-reasoning`、`demo-frontier`、`demo-specialist`。

`validate()` 还检查：id/model 非空、id 唯一、`intelligence_tier` 已定义、成本有限且 ≥0、`chat_completions`/`anthropic` 必须有 `base_url`。

### 6.4 运行限制子表

`[context]`：

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `enabled` | `true` | 关闭后超限直接报错，不投影 |
| `target_fraction` | `0.7` | 投影目标占输入上限比例（0.1–0.95） |
| `retain_recent_groups` | `4` | 保留最近消息组数 |
| `summary_max_bytes` | `4096` | 摘要上限，≥256 |

`[retry]`：`max_attempts=3`、`initial_backoff_ms=100`、`max_backoff_ms=5000`、`reroute=true`。

`[budget]`：`max_task_usd=None`、`max_total_usd=None`、`max_task_tokens=1_000_000`、`max_total_tokens=None`。

`[storage]`：`backend="file"`，可选 `sqlite`。

`[logs]`：`rotation_bytes=64 MiB`、`max_archives=8`。

`[ui]`：`theme="dark"`（`dark|light|system`）、`submit_key="Ctrl+Enter"`、`new_session_key="Alt+N"`、`cancel_key="Ctrl+K"`。

`[[workspaces]]`（静态工作区，四项全必填）：`id`（不得为 `default`，ASCII 字母数字/`-`/`_`，唯一）、`name`、`path`、`data_dir`。启动时为每个静态工作区创建独立运行时并挂在 `/w/{id}`。

### 6.5 `[tools]` 与 `[network]`

`[tools]`：`read`、`write`、`edit`、`list_files`、`glob`、`grep`、`webfetch`、`todo`、`shell`、`mcp` 全部默认 `true`；`shell_timeout_ms=30000`。

> 安全提示（源码注释同样强调）：`shell` 不是沙箱；`write/edit` 只在 workspace 内受限。处理不可信内容时显式关闭 `shell`。

`[network]`：

| 键 | 默认 | 说明 |
| --- | --- | --- |
| `allowed_hosts` | `[]` | 非空时成为白名单（大小写不敏感） |
| `denied_hosts` | `[]` | 优先于白名单 |
| `allow_loopback` | `true` | 是否允许回环地址 |
| `allow_private` | `false` | 是否允许私网/链路本地等 |
| `allow_http_loopback` | `true` | 回环是否允许明文 HTTP |
| `allow_http_hosts` | `[]` | 允许明文 HTTP 的特定主机 |

解析 DNS 后检查**每一个**地址；拒绝 URL 内嵌凭据；HTTPS 为默认要求。

### 6.6 `[[mcp_servers]]` 与 `[[remote_facets]]`

MCP 关键键：`name`（1–32，`[A-Za-z0-9_-]`，禁 `__`）、`command` + `args` 或 `url`（二选一）、`env`、`bearer_token_env`、`enabled`、`timeout_ms=60000`、`startup_timeout_ms=30000`、`cwd`、`scrub_env=true`、`max_discovery_pages=100`、`client{roots, enable_sampling, enable_elicitation, enable_tasks, max_sampling_tokens}`。

Facet 关键键：`name`、`endpoint`、`auth_token_env`（必填）、`timeout_ms=30000`、`startup_timeout_ms=10000`、`max_response_bytes=4 MiB`、`allowed_capabilities`（非空）、`object_id`、`allow_insecure_loopback=false`。连接失败会让 `AgentRuntime::build` 失败（与 MCP 的“失败但可降级”不同）。

### 6.7 配置模板

```powershell
samho config init samho.toml
samho --config samho.toml serve
```

`config init` 生成的是 **OpenAI 兼容网关模板**（默认 API `http://127.0.0.1:3425/v1`、模型 `wuyou/deepseek-v4.1-flash`、`provider = "chat_completions"`，密钥 `SAMHO_API_KEY=magpie`），模型 id `deepseek-v4.1-flash` 与 RoleRouter 的默认绑定一致。等价完整示例见 `examples/samho.deepseek.toml`；其他 OpenAI 兼容模板见 `examples/samho.chat.toml`，Anthropic 协议示例见 `examples/samho.anthropic.toml`（旧默认 `glm-5.3-flash`，仍被 RoleRouter 兼容识别）。

---

## 7. 运行时核心（samho-core）

### 7.1 `agent.rs` — AgentRuntime

**启动顺序**（`RuntimeBuilder::build`）：

1. `config.validate()`。
2. 规范化 `workspace`（必须存在）与 `data_dir`（创建）。
3. 相对 `skill_roots` 按 workspace 解析。
4. 打开 `runtime.lock` 并取 `fs2` 独占锁。
5. 构造 `Redactor::from_env`。
6. 打开事件存储（注入 backend → SQLite → file + 轮转）。
7. 打开会话存储（注入 backend → SQLite → file）。
8. 打开 `ProjectStore`（`projects.json`）与 `AttachmentStore`（`attachments.json`）。
9. 技能发现 → MCP 连接（仅 `tools.mcp`）→ 工具注册 → 模型路由 → 上下文投影器 → 预算账本 → Provider。
10. 注册 Object：`runtime`、每个 `model:*`、`skill:*`、`tool:*`、`mcp:*`、已有 `session:*`，再连接 `remote_facets`。
11. `objects.activate()`（拓扑顺序）。
12. `migrate_default_project()`：若已有会话且项目列表为空，创建“默认项目”并归入全部会话；幂等。

**任务生命周期**（`run_with_task_id`）：

1. 拒绝空/纯空白 prompt。
2. 取全局并发许可；排队期间可取消（错误 `cancelled while queued`）。
3. 生成 trace id；`create_or_get(session_id)`（显式未知 id 报错）。
4. 任务 id 不能与活动任务重复；会话不能已在 `running`。
5. 会话置 `running`，注册 `ActiveTask`，写 `task.start`。
6. 读取历史；把所有**未配对**的 `tool_calls` 补一条合成 `role:"tool"` 观察：
   `Interrupted previous tool call. Its effect is unknown; reconcile state before repeating it.`
   **绝不自动重放外部副作用。**
7. 追加用户消息；会话标题取 prompt 前 80 字符。
8. 显式 `skills` 以 system 消息注入，并写 `skill.load`。
9. 进入轮次循环（最多 `max_turns`）：
   - 每轮开始检查取消、应用全部 steering。
   - 轮界刷新工具 schema。
   - 按“模型上下文窗口 − 最大输出”投影上下文；必要时发 `context.project`。
   - 预算按 worst-case 预留；发 `model.start`。
   - 流式或非流式调用 Provider；成功结算，失败按可重试性退避/重路由。
   - 校验 assistant 消息并检查点；发 `model.response`。
   - 无工具调用：`finish_reason=="length"` 报“输出被截断”；否则应用控制队列，必要时继续。
   - 有工具调用：批次在 `length` 时整体拒绝；逐个校验 id/名称/参数 JSON；顺序执行；结果截断后写 `role:"tool"`；每个工具后检查点并发 `tool.result`。
10. 达到 `max_turns`：错误 `maximum agent turns reached; task is incomplete`。
11. 终态：`done` / `cancelled` / `failed`；写 `task.{status}`；提交会话；清理活动任务与预算。

**控制队列**：

- `steer`：在下一个安全边界（轮首或终态）作为用户消息注入。
- `follow_up`：只在任务本应结束时、且本轮没有应用 steering 时取一条继续。
- prompt 长度 1 B–1 MiB；两类合计最多 128 条；关闭后拒绝新控制。
- 返回 `{sequence, kind, status:"queued"}`，并写 `task.control.queued` / `task.control.applied`。

**重试与重路由**：

- 可重试：`Timeout | Network | RateLimited | Server`。
- 退避：优先 `Retry-After`，否则 `initial_backoff_ms << (attempt-1)`，封顶 `max_backoff_ms`。
- `retry.reroute=true` 且任务未显式指定模型：把失败模型加入排除表，重新路由并发 `model.reroute`。
- 取消发生在退避期间同样立即结束。

**取消与关闭**：

- 取消贯穿排队、请求、退避、工具执行；工具已被调用后提示“外部副作用可能未知”。
- `shutdown()` 取消所有活动任务、`objects.dispose()`（默认排空 30 s）、关闭 MCP；不等待任务 future 自然结束。
- 服务层 `settle()` 最多等 5 s，然后 abort 并把任务标记为 `cancelled`，错误 `service shutdown interrupted the task`。

**进度广播**：

`TaskProgress{task_id, session_id, trace_id, model_id, turn, attempt, status, text, reasoning, tool_calls, usage, revision}`。

- 状态：`running`、`streaming`、`model_done`、`done`、`failed`、`cancelled`。
- 每次发布 `revision += 1`；客户端必须忽略旧 revision。
- 文本/推理/工具参数在进入快照前脱敏；流式文本对跨 chunk 的秘密前缀做 withhold。

### 7.2 `model.rs` — 路由

**自动分类**（仅当 `automatic=true` 且未指定 `model/tier/intelligence_tier`）：

1. prompt 小写。
2. 按配置顺序找**第一条**关键词命中规则，取其 complexity。
3. 无命中：字符数 ≤32 → 1，否则 → 4。

> **坑**：`automatic=true` 时传入的 `profile.complexity` 会被覆盖。要精确控制复杂度，必须 `automatic=false`。

**期望层级**：显式 `tier` → 复杂度映射（0–1 nano、2–3 fast、4–5 balanced、6–7 reasoning、8–10 frontier）→ 自定义 `tiers` 区间匹配。

**候选过滤**（拒绝理由写入 `RouteDecision.rejected`）：

- 重试排除表。
- 自定义层级/显式层级不匹配。
- 无工具、无视觉、无推理能力。
- 上下文容量不足（`context_tokens + 输出 > context_window`）。
- 预算/延迟/specialty 不满足。

**评分**：

```text
score = 100
      - |实际 rank - 期望 rank| × tier_penalty
      - latency_ms × latency_weight
      - estimated_usd × cost_weight
```

按 score 降序，同分按 model id 升序。`estimated_usd` 基于输入 tokens、最大输出/预留 tokens 与每百万 token 单价。

**RoleRouter（服务层）**：`samho-cli/src/roles.rs` 在基础路由之后，按 `decision.desired_tier_name` 查 `model_roles.json`，把该任务层级改绑到指定模型；任务显式 `profile.model` 时完全不介入。无保存文件时，每个层级默认绑定 `DEFAULT_MODEL_IDS` 中首个已配置的模型（`deepseek-v4.1-flash` 优先，其次兼容旧配置的 `glm-5.3-flash`），都未配置则跟随自动路由。

### 7.3 `provider.rs` — Provider 抽象与 Chat Completions

**错误类型**：`Cancelled`、`Timeout`、`Network`、`RateLimited`、`Server`、`Authentication`、`Protocol`、`Configuration`、`ResponseLimit`、`OutputLimit`、`ConsumerClosed`（serde snake_case）。

**Chat Completions 请求**：

- `base_url` 必须是 HTTP(S)、无凭据/fragment；路径不以 `/chat/completions` 结尾时自动追加。
- `api_key_env` 指定环境变量，值非空，作为 Bearer。
- 请求体：`model`、`messages`、`stream`、`n:1`、`max_tokens` 或 `max_completion_tokens`；流式加 `stream_options.include_usage=true`；有工具且模型支持时发 `tools`；`reasoning_effort` 原样发送。
- `extra_body` 必须为对象，且不能覆盖：`model`、`messages`、`tools`、`stream`、`stream_options`、`n`、输出 token 键、`reasoning_effort`、`api_key`、`base_url`、`timeout`、`max_retries`。

**流式解析规则**：

- 响应必须是 `text/event-stream`；否则协议错误。
- 总响应上限 16 MiB，单帧上限 256 KiB，工具调用最多 128 个。
- 只接受 `[DONE]` 作为正常结束；EOF 前未见 `[DONE]` → Protocol。
- `finish_reason` 只接受 `stop` / `tool_calls`；`length` → OutputLimit；`content_filter` → Protocol。
- 推理增量优先 `reasoning_content`，其次 `reasoning`。
- 工具调用索引必须从 0 连续；id/name/arguments 可跨 chunk 拼接。
- `usage.prompt_tokens` / `completion_tokens` 缺失按 0；非法类型 → Protocol。
- 支持 `Retry-After`（秒）转毫秒。

**Demo Provider**：

- `/demo read PATH` → 真实 `read_file` 工具调用。
- `/demo skill NAME` → 真实 `load_skill`。
- `/demo write PATH\nCONTENT` → 真实 `write_file`。
- 工具结果后的下一条回复：`【模拟模式；以下为真实工具结果】[DEMO simulation; real tool observation]…`。
- 普通 prompt：`【模拟模式】[DEMO simulation / model]…`，usage 0。
- 流式按 32 字符安全分块（不会切坏 UTF-8）；**只要 `config.streaming=true` 就走流式，即使模型声明 `streaming=false`**。

### 7.4 `anthropic.rs` — Anthropic Messages API

用于把内部 OpenAI 风格消息转换为 `/v1/messages` 协议。

**选择与端点**：

- `provider = "anthropic"`；`base_url` 必填。
- 若 `base_url` 路径以 `/messages` 结尾，原样使用；否则追加 `/v1/messages`。
  - `http://host` → `http://host/v1/messages`（推荐）。
  - `http://host/v1` → `http://host/v1/v1/messages`（**不要这样配**）。
- 认证：`api_key_env` → `x-api-key`；始终发送 `anthropic-version: 2023-06-01`；禁用重定向。

**请求转换**：

- 所有 system 消息按顺序用 `\n\n` 拼成顶层 `system`。
- `user` → 单个 text block。
- `assistant` → 可选 text block + 每个 `tool_call` 一个 `tool_use`；参数非法 JSON 退化为 `{}`；空内容且无调用写 `(no content)`。
- `role:"tool"` → `tool_result`；连续工具结果合并进一条 user 消息；`is_error` 仅当内容以 `{"error":` 开头。
- 若首条不是 user，自动插入 `(session resume)` 占位。
- **不回传 thinking 块**（网关不承诺空 signature 历史）；思考内容通过 `reasoning_content` 展示。
- 工具 schema：`{name, description, input_schema}`；重名静默去重；缺 `type` 补 `object`。
- `max_tokens = model.max_output_tokens`；`output_token_parameter` 在 Anthropic 路径被忽略。
- `extra_body` 不允许覆盖 `model/messages/tools/max_tokens/stream/system/stop_sequences/temperature/top_p/metadata/api_key/base_url`；`thinking` **可以**通过 `extra_body` 传入。
- `reasoning_effort` 不发送。

**响应与流**：

- `stop_reason` 映射：`end_turn|stop_sequence`→`stop`；`tool_use`→`tool_calls`；`max_tokens`→`length`；`refusal`→`content_filter`。
- text 块用 `\n` 连接；thinking 块进 `reasoning_content`；`redacted_thinking` 与服务端工具块忽略。
- 无文本且无工具：`max_tokens` → OutputLimit（“thinking-only response truncated”），否则 Protocol。
- 流式映射：`content_block_delta` 的 `text_delta`/`thinking_delta`/`input_json_delta`；`message_start` 取输入 tokens；`message_delta` 记录 stop_reason/output tokens（**不发 Usage/Finish 事件**）；`message_stop` 结束。
- **限制差异**：Anthropic 流式解析没有总字节/单帧/工具数量上限，也没有内层超时；超时与取消依赖外层 `Provider::guarded`。

> **配置坑**：思考 token 计入 `output_tokens`。`max_output_tokens` 太小（如 256）会导致 content 为空；真实思考模型建议 ≥512，示例配置用 16384。

### 7.5 `tools.rs` — 工具、策略与边界

**调用流水线**：

1. 取消检查。
2. 所有活动 Skill 的 `allowed-tools` 门控（任一不允许即拒绝）。
3. 注入的 `ToolPolicy` 授权（默认全允许）。
4. 输入 JSON Schema 校验（禁外部 `$ref`）。
5. 经 `ObjectRegistry` 租约调用（若已绑定对象）。
6. 输出 schema 复验。

**内置工具**：

| 工具 | 开关 | 输入 | 关键行为 |
| --- | --- | --- | --- |
| `read_file` | `read` | `{path, offset?, limit?}` | 1 起始行窗口；默认整文件；单次输出上限 64 KiB（截断）；UTF-8（去 BOM），带 BOM 的 UTF-16LE/BE 也按文本读取（PowerShell 5.1 重定向产物）；返回 `/` 分隔的相对路径与行号/截断信息 |
| `write_file` | `write` | `{path, content}` | 创建父目录，覆盖写；返回相对路径与字节数 |
| `edit_file` | `edit` | `{path, old_string, new_string, replace_all?}` | 精确替换；0 匹配报错；多处匹配且未开 `replace_all` 报错；文件 ≤16 MiB |
| `list_files` | `list_files` | `{path?, depth?, max_entries?}` | 默认根/深 4/500 条；跳过 `.git`、`node_modules`、`target` 与符号链接 |
| `glob` | `glob` | `{pattern, path?, max_entries?}` | 段内 `*`/`?`，`**` 跨目录；无 `/` 的模式等价 `**/pattern`；只返回文件 |
| `grep` | `grep` | `{pattern, path?, glob?, ignore_case?, max_results?}` | 正则内容搜索；跳过二进制、>1 MiB 文件、忽略目录与符号链接；行截 240 字符 |
| `webfetch` | `webfetch` | `{url, max_bytes?}` | 仅 HTTP(S)；逐跳过 `NetworkPolicy`；最多 5 次重定向；30 s 总超时；拒绝二进制；HTML 转纯文本 |
| `todo_write` | `todo` | `{items:[{task,status,note?}]}` | 全量替换会话清单；≤200 项；原子写 `data_dir/todos/{session}.json` |
| `todo_read` | `todo` | `{}` | 读取清单；不存在返回 `exists:false` |
| `shell` | `shell` | `{command}` | Windows `powershell.exe -NoProfile -NonInteractive` 执行固定包装脚本：脚本经 `SAMHO_SHELL_COMMAND` 环境变量传入 ScriptBlock，控制台输出切到 UTF-8，保留 `exit N`/原生命令退出码/最后语句 `$?` 语义，默认设 `PYTHONUTF8=1`；工具描述告知模型是 PowerShell 5.1（无 `&&`/`||`）。Unix `sh -c`。超时/取消杀子进程 |
| `load_skill` | 恒开 | `{name}` | 加载 SKILL.md；受 `disable-model-invocation` 约束 |
| `mcp__{server}__{tool}` | `mcp` | 由 server 决定 | 路由到 MCP；公开名 ≤64 字符 |

**路径规则**（`within_workspace`）：拒绝绝对路径、任何 `..` 段、越界路径、符号链接（含悬空链接）；不存在的路径沿父链找到最近的已存在祖先并校验其仍在 workspace 内（因此 `write_file` 可一次创建多级新目录，如 `pkg/sub/file.py`）。

**MCP 命名**：`mcp__{server}__{normalized}`；server 名不合法或最终名 >64 字符返回协议错误。

### 7.6 `session.rs` — 会话 journal

**Schema v2**：每条记录是一个扁平 JSON 对象：

```json
{
  "schema_version": 2,
  "sequence": 1,
  "previous_checksum": "",
  "metadata": { "id": "...", "title": "...", "...": "..." },
  "truncate_to": 0,
  "append_messages": [],
  "checksum": "sha256(...)"
}
```

- `sequence` 在**所有会话之间全局递增**。
- `checksum = SHA-256(serde_json::to_vec(Delta))`；`previous_checksum` 形成链。
- 新记录只追加与已提交内容的差异（长会话增长是次线性的）。
- 打开时校验 schema/序号/checksum；只截断未完成的尾部；中间损坏直接拒绝。
- 旧格式全量快照自动迁移：原文件备份为 `sessions.legacy.jsonl`，重写为每会话一条全量 delta。
- 后端：file（JSONL，失败回滚偏移，`durable` 时 `sync_data`）或 SQLite（WAL，`synchronous=FULL`，事务替换）。

**状态机**：

```text
ready ──run──> running ──> done | failed | cancelled
                  │
                  └─ 重启恢复 ──> interrupted
```

**API**：

- `create_or_get(Option<id>, system_prompt)`：未知显式 id 报错；新会话 UUID。
- `commit`：脱敏后计算差异并追加。
- `fork(parent, cutoff, title)`：父会话不能 `running`；cutoff 必须在 `1..=len`（保留 system）；不能切开未完成的工具调用组；子会话继承前缀、`parent_id`、`fork_cutoff`、`last_model`、`project_id`；默认标题 `"{parent.title} · 分支"`。
- `rename`、`set_archived`、`set_project`：都写 journal；`project_id=None` 表示移出项目。
- `delete_cascade`：删除目标及全部 fork 后代；任何成员 `running` 时拒绝；返回删除 id 列表。**不级联删除项目或附件记录**（服务层会清附件）。
- `list` / `list_summaries`：按 `updated_at` 降序。

`Session`/`SessionSummary` 含 `project_id`；附件是独立存储。

### 7.7 `events.rs` — 事件与轨迹

`TraceEvent{sequence, timestamp_ms, trace_id, session_id, span_id, parent_span_id, kind, object_id, payload}`。

**Agent 发出的事件**：

```text
task.start
skill.load
model.route
context.project
model.start
model.error
model.reroute
model.retry
budget.overage
model.response
tool.start
tool.result
task.control.queued
task.control.applied
task.done | task.failed | task.cancelled
```

**Span 命名**：

- 根事件 `span_id = task_id`，`parent_span_id = None`。
- 其他事件 `span_id = "{task_id}:{suffix}"`，父 span 为 `task_id`。
- 典型 suffix：`terminal`、`route`、`context:{turn}:{attempt}`、`model:{turn}:{attempt}`、`model:{turn}`、`route:{turn}:{attempt}`、`retry:{turn}:{attempt}`、`tool:{call_id}`、`skill:{name}`、`control:{sequence}`。

**存储与查询**：

- 序号从 1 单调递增；持久化前脱敏。
- `EventQuery{after_sequence, limit, trace_id, session_id, kind}`；`after` 独占；limit 1..=10000（HTTP 默认 100，上限 5000）。
- `EventPage{events, next_cursor, has_more, first_available_sequence}`；`first_available_sequence` 用于检测 retention gap。
- 文件模式按字节轮转：`events.jsonl.segment-{id:020}`；超过 `max_archives` 删除最旧；内存索引按字节偏移。
- 无时间/条数保留策略，只有字节轮转。
- 广播容量 256；落后即 `Lagged`。
- `export_jsonl` 先取 watermark，只导出 ≤ watermark 的事件，拒绝覆盖活动 journal。

**脱敏**：

- `Redactor::from_env` 收集敏感环境变量值（≥8 字符）。
- 文本替换为 `[REDACTED]`；正则覆盖 `Bearer …` 与 `sk-…`。
- 流式文本会暂扣可能被切断的秘密前缀。
- 递归处理 JSON：敏感键（authorization、api_key、password、secret、access_token、refresh_token、bearer_token）整值替换。

### 7.8 `context.rs` — 上下文投影

- 计数器 `ConservativeTokenCounter`：按消息与工具 schema 的序列化 UTF-8 字节计数，**故意保守**，不是供应商 tokenizer。
- 未超限：原样返回，无 lineage。
- 超限且 `enabled=false`：直接报错。
- 投影规则：
  - system 消息全部保留。
  - assistant 工具调用 + 后续全部 `tool` 观察是不可拆分的一组。
  - 最新用户消息所在组受保护。
  - 其余从较早的组开始摘要；摘要是一条 user 消息，包含中文“数据非指令”前缀、源索引范围、源 SHA-256 与前 320 字符 JSON 摘录。
  - 依次尝试 `summary_max_bytes`、512、256。
  - 成功条件：投影后 ≤ 输入上限，且 ≤ target_fraction 或只剩受保护内容。
  - 无法满足时报：`context capacity insufficient for protected system instructions, current user request, tools and complete tool batches`。
- 原始会话消息永不修改；lineage 写入 `context.project`。

### 7.9 `budget.rs` — 预算

- `budget.jsonl` 每行是**累计总用量**；打开时读最后一条完整行，截断残尾。
- 派发前 `reserve(task, usd, tokens, request_limit)`：
  - 拒绝非有限/负值。
  - 任务 USD 上限取 `min(request_limit, config.max_task_usd)`（两者都存在时）。
  - 检查任务/总 USD、任务/总 tokens。
  - 先持久化新总额，再更新内存。
  - 未知失败按预留扣费，不退款。
- `settle(reservation, actual)`：用实际值替换预留；实际超过预留时返回 `true`，Agent 发 `budget.overage` 后**继续任务**，不中止。
- 硬上限仍在派发前拒绝。
- `reasoning_reserve_tokens` 用于思考模型：预留 = `max_output_tokens + reasoning_reserve_tokens`（缺省再 + `max_output_tokens`）。

### 7.10 `object.rs` — Object 生命周期

- `ObjectKind`：`Agent`、`Model`、`Tool`、`Skill`、`McpServer`、`Session`、`Task`、`Plugin`、`Custom`。
- `ObjectEnvelope{id, kind, version, description, dependencies, capabilities, labels, spec}`。
- `Object` trait：`envelope`、`invoke`、可选 `activate`/`dispose`。
- 生命周期：
  - `register` 拒绝空/重复 id；scope 必须存在。
  - 激活按依赖拓扑排序；缺依赖/环/跨 scope 依赖拒绝；失败回滚本次新增对象。
  - `acquire` 增加 in-flight；draining/inactive 时失败。
  - `hot_replace` 要求同 id/kind/依赖、无 dependents；候选激活成功后旧代排空，超时回滚。
  - `dispose_scope` 逆序销毁；`dispose` 默认排空 30 s。
- scope 可见性沿祖先链；`root` 是默认 scope。
- middleware：`before` 可改输入/短路；`after` 逆序执行，成功和失败都执行。

### 7.11 `skills.rs` — 技能

- 只识别名为 `SKILL.md` 的文件；递归 ≤32 层；跳过符号链接与 `.git`/`node_modules`/`target`；支持 `.skillignore`。
- YAML frontmatter：`name`（1–64，小写字母/数字/连字符，无首尾 `-`、无 `--`）、`description`（≤1024）、`disable-model-invocation`、`allowed-tools`（空格分隔字符串或数组，支持 `*`/`?`，`tool:` 前缀可省）、`metadata`。
- 文件 ≤1 MiB；路径任一层是符号链接即拒绝。
- 重名：首个胜出，其余记诊断；非法技能只记诊断，不致命。
- 显式加载（用户/任务）可绕过 `disable-model-invocation`；模型通过 `load_skill` 加载时受其约束。
- 活动技能的 `allowed-tools` 对**后续所有工具**生效，包括内置工具。
- `read_resource(relative, max_bytes)` 只允许技能目录内文件；默认 64 KiB，上限 1 MiB；不执行脚本。

### 7.12 `mcp.rs` 与 `mcp/oauth.rs`

- 传输：stdio（`TokioChildProcess`）与 Streamable HTTP。
- stdio 会剔除继承环境中名字含 `KEY/PASSWORD/SECRET/TOKEN` 的变量，再合并显式 `env`；Windows 隐藏子进程窗口。
- 已实现：initialize、`tools/list`（分页 + 原子代际替换）、`tools/call`、resources（list/templates/read/subscribe）、prompts（list/get）、completion、ping、roots、sampling/elicitation（需显式启用 + 宿主 hook）、tasks（轮询/取消）。
- 未实现并会显式返回 `Unsupported`：OAuth metadata discovery、动态 client registration、URL elicitation、modern subscriptions/listen、`server/discover`、非 task MRTR、stdio 自动重连、外部 `$ref`。
- 工具公开名 `mcp__{server}__{normalized}`。
- 状态：`disabled|failed|connected|degraded|closed`；单个 server 失败不阻塞其他 server。
- `mcp/oauth.rs` 是**独立**的 Authorization Code + PKCE 客户端（回环回调，端口 300–400），**没有接入 RuntimeConfig/McpServerConfig**；不要把它当作 MCP 自动 OAuth。

### 7.13 `facet.rs` — Remote Facet

- 协议 `samho-facet/1`；HTTPS（回环可显式允许 HTTP）+ Bearer + 非空能力白名单。
- `POST {endpoint}/handshake` → `POST {endpoint}/invoke`；取消时 best-effort `POST {endpoint}/cancel`（≤250 ms）。
- 要求 trace/session/span 非空；trace id 为 32 位 hex 时透传 W3C `traceparent`。
- 握手失败、协议不符、能力超出白名单、响应超限都 fail closed。
- 这是受控远程 Object 适配器，不是任意远程执行/进程隔离。

### 7.14 `projects.rs` 与 `attachments.rs`

**Project**：`{id(UUID), name, color?, created_at, folders[], entry?, open}`。

- `name` 非空 ≤64，精确重名拒绝。
- `color` 只接受 `#rrggbb`，统一小写。
- `folders` 是 workspace 相对路径：去首尾 `/`/`\`，禁 `:` 与 `\`，禁空段/`.`/`..`。
- `entry` 必须是 `folders` 之一。
- 持久化 `projects.json`：整表快照，临时文件 + flush + 可选 `sync_all` + rename；失败不影响内存。
- 删除项目**不**删除会话，服务层会清空会话的 `project_id`。

**SessionAttachments**：`{terminal_ids[], browser_tabs[{url,mode}], updated_at}`，持久化 `attachments.json`。浏览器 URL 写入前脱敏；`updated_at` 由服务端覆盖。

---

## 8. CLI 与 HTTP 服务（samho-cli）

### 8.1 `main.rs`

见 §4.2。要点：

- `tui` 在加载配置前就分流，`--config` 对 TUI 无效。
- `config init` 在构建运行时之前处理，不创建数据目录。
- 非 TUI 命令结束后统一 `runtime.shutdown()`；shutdown 错误优先于命令错误。
- tracing 写 stderr；默认 `warn`，所以正常启动不会看到 info 日志。

### 8.2 `server.rs` 启动与安全

启动顺序：

1. 解析并绑定 `--bind` / `http_bind`。
2. 打印 workspace / LAN / 无认证警告 / TUI 提示。
3. 构建根 `AppState`，加载 `<data_dir>/workspaces.json`。
4. 为每个静态 `config.workspaces` 构建独立运行时并 `nest` 到 `/w/{id}`。
5. `axum::serve(...).with_graceful_shutdown(ctrl_c)`。
6. 关闭：取消根 token 与全部任务 → `settle()` 最多 5 s → abort 剩余任务 → 关闭额外运行时/终端/动态工作区 → 根 `runtime.shutdown()`。

中间件与限制：

- body limit 1 MiB。
- `local_origin`：Origin/Host 校验；CSP、nosniff、no-store。
- 错误信封：`ApiError` → `{"error": "..."}`；axum extractor 自身错误（坏 JSON、未知字段、超限）使用 axum 默认响应，不保证 `{error}` 形状。
- 没有 SIGTERM 处理，只处理 Ctrl+C。

LAN 发现：UDP connect 到 8.8.8.8 / 10.255.255.255 / 192.168.255.255 / 172.31.255.255 + 主机名解析；过滤回环、未指定、链路本地、198.18/19；结果用 `OnceLock` 缓存。

任务索引：

- `BTreeMap<task_id, TaskEntry{cancel, abort, snapshot, bytes}>`。
- ≥1024 条时只保留运行中任务。
- 完成快照序列化总量 32 MiB，超限逐条淘汰非运行任务。
- 结果入索引前脱敏；panic → `failed` + `task execution panicked`。

### 8.3 HTTP API

所有 `/api` 与页面路由都在 `router_with_state` 注册。下表省略了统一的安全头。

#### 页面与静态资源

| 路径 | 说明 |
| --- | --- |
| `GET /`、`GET /index.html` | 工作台 |
| `GET /assets/app.js`、`GET /assets/app.css`、`GET /vendor/*` | `STATIC_ASSETS` 6 项 |
| `ANY /w/{id}`、`ANY /w/{id}/{*rest}` | 动态工作区分发 |
| 其他 | 404 纯文本；`/w/{id}/` → 308 到 `/w/{id}`（保留 query） |

#### 状态与注册表

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/health` | `{"status":"ok","api_version":1}` |
| `GET /api/network` | `{addresses:[局域网 IPv4], port}` |
| `GET /api/state` | 运行时全量状态（脱敏） |
| `GET /api/models` | 模型描述数组 |
| `GET /api/objects` | Object 列表 |
| `GET /api/skills` | 技能列表 |
| `POST /api/skills/refresh` | 重新发现技能 |
| `GET /api/model-roles` | `{roles, default_model, tiers, models}` |
| `PUT /api/model-roles` | `{roles:{tier:model}}`；空串 = 自动；未知层级/模型 400 |
| `GET /api/ui` / `PUT /api/ui` | 主题与快捷键；非法组合 400；写 `ui.json` |

#### 会话、项目、附件

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/sessions` | `SessionSummary[]`（含 archived/project_id） |
| `GET /api/sessions/{id}` | 完整会话；未知 404 |
| `PATCH /api/sessions/{id}` | `{title?, archived?, project_id?}`；标题 ≤200；项目不存在 400；空 patch 400 |
| `DELETE /api/sessions/{id}` | 级联删除 fork 后代；同时清附件；返回 `{deleted:[ids]}` |
| `POST /api/sessions/{id}/fork` | `{cutoff?, title?}`；默认父消息数；错误 409 |
| `GET /api/sessions/{id}/attachments` | 返回记录；**不校验会话是否存在**，缺失返回空默认值 |
| `PUT /api/sessions/{id}/attachments` | 校验会话；URL 脱敏；`updated_at` 服务端覆盖 |
| `GET /api/projects` / `POST /api/projects` | 列表 / 创建（201） |
| `GET /api/projects/{id}` | 单个项目 |
| `PATCH /api/projects/{id}` | 改名/颜色/folders/entry/open |
| `DELETE /api/projects/{id}` | 删除项目并把相关会话 `project_id` 清空；返回 `sessions_cleared` |

#### 任务与控制

| 方法与路径 | 说明 |
| --- | --- |
| `POST /api/run` | `TaskRequest{session_id?, prompt, profile?, skills?}` → 202 `{task_id,status:"running"}`；空 prompt 400；并发满 429；关停中 503 |
| `GET /api/tasks` | `{tasks:[...]}`；顺序为 BTreeMap 反向键序，**不是**“运行中优先” |
| `GET /api/tasks/{id}` | `{task_id,status,result,error,progress,error_details}`；未知/重启后 404 |
| `POST /api/tasks/{id}/cancel` | `{task_id,status,cancel_requested}` |
| `POST /api/tasks/{id}/steer` | `{prompt}` → 排队控制；错误 409 |
| `POST /api/tasks/{id}/follow-up` | 同上，任务结束后处理 |

#### 事件与轨迹

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/events` | **裸数组**；query：`trace_id/session_id/kind/after/limit`（limit ≤5000） |
| `GET /api/events/page` | `{events,next_cursor,has_more,first_available_sequence}` |
| `GET /api/events/stream` | SSE：`event: trace`、`event: gap`、`event: error`、`keepalive`；`Last-Event-ID` 回退 |
| `GET /api/live/stream` | SSE：`event: progress`（累计快照）、`event: gap` |
| `GET /api/traces/{id}/timeline` | `{trace_id, mode:"展示式复盘，不会重新执行", spans}` |
| `GET /api/traces/{id}/export` | Markdown 附件，固定文件名 `Samho-trace.md`；>16 MiB → 413 |

SSE 细节：`/api/events/stream` 先按游标分页回补（每页 128），再订阅广播；若 `first_available_sequence` 已超过游标，发 `gap {missed, after, recovery}`；keepalive 15 s。

#### 工作区

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/workspaces` | default + 静态 + 动态；动态 `managed:true` |
| `POST /api/workspaces` | 仅根服务；`{path,name?}`；路径必须存在；≤32 个；201 |
| `PATCH /api/workspaces/{id}` | 仅动态工作区；改名 |
| `DELETE /api/workspaces/{id}` | 仅动态工作区；静态需改配置；default 400 |

动态工作区 id 由路径 basename 转 slug（非 ASCII 会退化为 `ws`），冲突追加 `-2`、`-3`。

#### 终端、文件、Git、代理

| 方法与路径 | 说明 |
| --- | --- |
| `GET /api/term?session_id=` | 该会话的 live PTY 列表 |
| `POST /api/term` | `{cwd?, session_id?}` → `{id}`；每工作区最多 8 个；cwd 必须在 workspace |
| `GET /api/term/{id}/stream` | SSE：先 `backfill`（完整回滚），再 `data`；15 s keepalive |
| `POST /api/term/{id}/input` | `{text}` → 写入 PTY |
| `POST /api/term/{id}/resize` | `{cols,rows}`，均钳制 ≥2 |
| `DELETE /api/term/{id}` | 关闭 PTY |
| `GET /api/fs?path=` | workspace 目录列表；跳过 `.samho`；最多 2000 条 |
| `DELETE /api/fs?path=` | 递归删除；不能删 workspace 根 |
| `GET /api/file?path=` | 文本读取（≤1 MiB）；二进制返回 `content:null` |
| `PUT /api/file` | `{path,content}`；新建/覆盖；受 1 MiB body 限制 |
| `GET /api/file/raw?path=` | 原始字节，≤8 MiB，按扩展名给 MIME |
| `POST /api/fs/mkdir` | `{path}`，递归创建 |
| `POST /api/fs/rename` | `{from,to}`，均受限 |
| `GET /api/browse?path=` | **设备级**目录浏览；空 path 返回盘符/`/`；不受 workspace 限制 |
| `POST /api/browse/mkdir` | **设备级**创建目录 |
| `GET /api/git/status` | `{branch,upstream,changes[]}` |
| `GET /api/git/diff?path=&staged=` | 统一 diff；显示截断 512 KiB |
| `GET /api/git/log?limit=30` | 提交历史，limit 1..200 |
| `GET /api/git/branches` | 分支列表 |
| `POST /api/git/stage` / `unstage` / `discard` | `{paths}` |
| `POST /api/git/commit` | `{message, paths?}`；message ≤4000 |
| `POST /api/git/checkout` | `{branch, create?}` |
| `GET /api/proxy?url=` | 浏览器代理：仅 HTTP(S)，15 s，≤4 MiB；HTML 去脚本/CSP meta、注入 `<base>`；非 HTML 透传；错误 502 |
| `POST /api/preview` | `{html}` → 201 `{id}`；暂存一段 HTML 供沙箱预览（内存 ≤64 条/16 MiB，按时间淘汰，重启失效）；空内容 400 |
| `GET /api/preview/{id}` | 以沙箱 CSP 返回暂存 HTML；未知 404 |
| `GET /api/preview/fs/{*path}` | 以沙箱 CSP 提供 workspace 文件（≤8 MiB，按扩展名 MIME），相对脚本/样式/资源可解析；目录 307 到 `index.html`；路径约束同文件面板 |

文件面板与 Git 面板的路径规则在 `panels.rs`：词法拒绝 `..`、盘符/ADS `:`，规范化后必须仍在 workspace；新建路径的父目录与最终组件都拒绝符号链接。`.samho` 只在目录列表中隐藏，直接访问未被阻止。

### 8.4 工作区挂载语义

- 静态工作区：启动时 `nest` 到 `/w/{id}`，各自独立运行时；这些子服务没有 hub，工作区 CRUD 返回 400。
- 动态工作区：`HubState.dynamic` 保存 `WorkspaceRecord`；请求 `/w/{id}/...` 时去掉前缀、清空 extensions，再 `oneshot` 进工作区路由（避免 `Path` 参数泄漏）。
- `/w/{id}` 无尾斜杠可到达工作区 `/`；`/w/{id}/` 不被 axum 0.8 的 nest 匹配，由根 fallback 308 到 `/w/{id}`（保留 query）。
- 每个工作区独立：data_dir、会话、事件、预算、项目、附件、终端、角色表。
- 动态工作区上限 32；注册表损坏的条目启动时跳过并重写文件。

### 8.5 TUI（`tui/` 模块树，对齐网页工作台）

纯 HTTP/SSE 客户端（不读取 `--config`），模块：`mod.rs`（App 状态/事件循环/键位）、`api.rs`（JSON + 两条 SSE）、`sse.rs`（解析器）、`markdown.rs`（pulldown-cmark → ratatui 完整 Markdown 渲染：标题/嵌套列表/GFM 表格/任务列表/代码围栏 syntect 高亮/引用/删除线/链接/图片降级）、`widgets.rs`（spinner/相对时间/截断/参数摘要等共享件）、`views/`（main/sidebar/tasks/inspector/files/git/term/browser/overlays）。

- 视图：**Alt+1** 对话（项目侧栏 + 流式 Markdown 转录 + 工具卡 + composer）、**Alt+2** 任务看板、**Alt+3** 文件、**Alt+4** Git、**Alt+5** 终端、**Alt+6** 浏览器；**Esc** 回主视图，**F7** 切工作区。
- 渲染 tick 33 ms；资源轮询 800 ms（11 个端点，含 /api/projects、/api/tasks、/api/model-roles）；`/api/live/stream` 与 `/api/events/stream` 断线 1s→8s 退避重连，轮询兜底。
- 主视图焦点三向循环（Shift+Tab）：composer ↔ 侧栏 ↔ 检查面板；侧栏支持 `/` 搜索、`m` 行菜单、`P` 新建项目、项目分组展开态持久化（PATCH open）。
- 检查面板（Tab 循环 6 页）：轨迹（model/tool 调用卡，可展开、`/` 过滤、`E` 导出 trace 到 `reports/`）、预算、对象、技能（`R` 刷新）、MCP、模型（角色改派 `Enter`/`R` 恢复默认，PUT /api/model-roles）。

键位：

| 键 | 行为 |
| --- | --- |
| `Ctrl+Q` | 立即退出（不取消任务）；终端接管态下=脱离 PTY |
| `Ctrl+Enter`（默认提交键） | 提交；当提交键仍是默认值时，裸 `Enter` 也可提交 |
| `Alt+N`（默认新会话键）/ `F5` | 新草稿（空闲时） |
| `Ctrl+K`（默认取消键）/ `Ctrl+C` | 取消活动任务；空闲且未提交时退出 |
| `Alt+1..6` | 对话 / 任务 / 文件 / Git / 终端 / 浏览器 |
| `Shift+Tab` / `Tab` | 主视图焦点循环 / 检查面板循环 |
| `Ctrl+M` / `F6` | 模型选择浮层 / 技能多选浮层 |
| `Ctrl+R` / `Ctrl+F` | 折叠思考块 / 切换 follow-up 模式 |
| `F2` / `F3` / `F4` / `F7` | 层级 / 模型 / 会话 / 工作区 |
| `PageUp` / `PageDown` | 滚动 ±8 行 |
| `Shift+Enter` | 换行 |

- 文件面板：Enter 打开（目录进入/文本查看/图片与 HTML 外开）、`M` Markdown 渲染切换、`e` 自建多行编辑器（`Ctrl+S` 保存 → PUT /api/file）、`n`/`N` 新建文件/目录、`r` 改名、`d` 删除（确认）、`s` 设树根、`p` HTML 沙箱预览（POST /api/preview → 系统浏览器）。
- Git 面板：Enter 看选中路径 diff（红绿着色）、`s`/`S` 暂存、`u` 取消暂存、`D` 丢弃（确认）、`c` 提交（输入浮层）、`l` 历史、`b` 分支（Enter 切换、`n` 新建）。
- 终端面板：`n` 新建（附属当前会话）、`Enter` **接管模式**——挂起 TUI、退出备用屏，`/api/term/{id}/stream` 字节流直通 stdout，键盘翻译 xterm 序列经 `/input` 写回，resize 联动；`Ctrl+Q` 脱离（PTY 保活，Enter 重入续接 backfill）、`K` 关闭。
- 浏览器面板：`o`/`u` 输入 URL 系统浏览器打开、`t` 文本模式（`/api/proxy` 取净化 HTML 后剥标签、实体解码、滚动阅读）——iframe/图片/交互网页属设计内降级。

斜杠命令：`/help`、`/steer <text>`、`/follow-up <text>`、`/fork [cutoff] [title...]`、`/theme dark|light|contrast`、`/bind submit|new|cancel <KeySpec>`、`/auto`（层级跟随自动路由）、`/complexity 0-10`。运行中裸 `Enter`/提交键 = steer（follow-up 模式时入队）。

已知不一致：`/theme contrast` 被本地支持，但服务端 `validate_ui` 只接受 `dark|light|system` → 400。

### 8.6 `panels.rs`、`term.rs`、`hub.rs`、`roles.rs`

- `panels.rs`：文件/Git/代理后端。文件上限 1 MiB、raw 8 MiB、diff 512 KiB、代理 4 MiB、目录 2000 条；Git 超时 30 s、stdout 2 MiB、stderr 64 KiB。
- `term.rs`：portable-pty（Windows ConPTY）；每工作区最多 8 个；30 分钟空闲回收；回滚 512 块；广播容量 1024；初始 24×80；Windows PowerShell、Unix `$SHELL`/`sh`。
- `hub.rs`：动态工作区注册表；`slugify` 只保留 ASCII 字母数字，连续分隔折叠为 `-`，非 ASCII 退化为 `ws`；`MAX_DYNAMIC_WORKSPACES = 32`。
- `roles.rs`：见 §7.2；默认绑定 `deepseek-v4.1-flash`（兼容 `glm-5.3-flash`），文件 `model_roles.json`，临时文件 + rename 持久化。
- `preview.rs`：沙箱 HTML 预览（§8.3）。响应 CSP 以 `sandbox allow-scripts …` 开头且**不含** `allow-same-origin`（不透明源，无法读写工作台或以同源身份调用 `/api`），`connect-src 'none'` 禁止网络请求。沙箱文档的子资源请求带 `Origin: null`：`local_origin` 仅对 `GET /api/preview/fs/*`（含 `/w/{id}` 前缀）且 Referer 来自本服务的请求放行，并回显 `Access-Control-Allow-Origin: null`。

---

## 9. Web GUI

### 9.1 技术栈与构建

工作台是 **React 19 + TypeScript + Vite 8 + Tailwind CSS v4 + shadcn/ui（new-york v4）** 应用，源码在 `web/app`，构建产物在 `web/dist`：

```powershell
npm ci --prefix web/app          # 首次或依赖变更
npm --prefix web/app run build   # tsc --noEmit + vite build → web/dist
cargo build -p samho             # 重新内嵌产物
```

开发时可用 Vite 热更新，代理到已运行的服务：

```powershell
$env:SAMHO_DEV_API = "http://127.0.0.1:375"; npm --prefix web/app run dev -- --port 376
# 浏览器打开 http://localhost:376（dev 模式没有服务端 CSP，最终验收必须用内嵌产物）
```

**CSP 约束**（服务端 `style-src 'self'; script-src 'self'`）决定了依赖选择：

- 不允许运行期注入 `<style>`：`react-remove-scroll` 经 Vite alias 替换为透传垫片（`src/shims/react-remove-scroll.tsx`）；不使用 Radix Select（下拉选择用 shadcn `NativeSelect`）、sonner、vaul、next-themes。Toaster 是同外观的最小自实现。
- React `style={}` 走 CSSOM，CSP 允许；产物 `index.html` 无内联脚本/样式（契约测试校验）。
- xterm 仍从 `/vendor` 惰性加载（UMD 全局），不打进主包。

主题 token 在 `src/index.css`：zinc 中性色 + 单一蓝色强调色（`--brand`）+ 语义状态色（`--success/--warning/--destructive`），`:root` 亮色、`.dark` 暗色；`system` 由 `controller.applyTheme` 解析。

### 9.2 布局

```text
<App>
├── Topbar            品牌 / 工作区名·模拟模式 / 文档 / 计划 / LAN 地址 / 连接状态 / 主题 / 设置 / 工具面板开关
├── aside#sidebar     工作区切换（下拉）+ 新任务 + 搜索 + 项目/文件夹/会话树
├── main              会话头（项目面包屑/标题/阶段/轮次进度/分支）+ 时间线 + Composer
└── aside#inspector   设备组（文件/Git/终端/浏览器）+ 检查组（轨迹/子代理/任务/对象/MCP/技能/模型）+ 事件流状态
```

响应式：≤1180 px 检查器变右侧抽屉（始终挂载，终端/浏览器状态不丢）；≤820 px 侧栏变左侧 Sheet 抽屉。

### 9.3 源码地图（`web/app/src`）

| 领域 | 文件 | 职责 |
| --- | --- | --- |
| 入口 | `main.tsx`、`App.tsx` | 挂载、布局、全局快捷键、sr-only 播报节点 |
| 状态 | `lib/store.ts` | zustand 全局状态（会话/项目/任务/轨迹/面板/请求配置） |
| | `lib/controller.ts` | 任务生命周期、SSE 双流、工作区切换（代际守卫）、会话加载、轨迹分页去重、主题 |
| | `lib/api.ts` | `/w/{id}` 前缀、fetch/EventSource 封装 |
| | `lib/projection.ts` | 消息 + live 工具 → 时间线行；轨迹事件 → 调用卡（含 `omitted` 识别 log_content=false） |
| | `lib/markdown.ts` | markdown-it（CommonMark + GFM 表格/删除线/任务清单/自动链接 + 原始 HTML）→ highlight.js 高亮（29 种语言，按需注册）→ Temml 数学（`$…$`/`$$…$$`/`\(…\)`/`\[…\]` → MathML）→ 预剥离 style 属性 → DOMPurify 净化（禁 script/style/iframe/object/embed/form/base/meta 等与 style 属性）；workspace 相对链接/图片改写为文件面板/`/api/file/raw` |
| | `components/markdown.tsx` | 唯一的 `dangerouslySetInnerHTML` 来源；事件委托处理代码块「复制 / 预览」与 workspace 相对链接（派发 `open-workspace-file` 给文件面板） |
| | `components/preview-dialog.tsx` | 沙箱预览对话框（`previewHtml` / `previewWorkspaceFile`）：iframe `sandbox` 不含 `allow-same-origin`，支持重载/全屏/新标签 |
| | `lib/expansion.ts`、`lib/bridge.ts` | 轨迹按角色展开；终端/浏览器命令式句柄（附属面板） |
| UI | `components/ui/*` | shadcn/ui：button/badge/input/textarea/dialog/sheet/dropdown-menu/popover/tooltip/command/switch/checkbox/slider/native-select/kbd/toaster |
| 布局 | `topbar.tsx`、`sidebar.tsx`、`conversation.tsx`、`composer.tsx`、`inspector.tsx` | 主框架 |
| 对话 | `timeline-items.tsx` | 工具卡（注册表文案/四态/输出钳制）、思考块 |
| 对话框 | `dialogs.tsx`、`settings-dialog.tsx`、`model-picker.tsx`、`projects-dialogs.tsx`、`workspace-dialog.tsx` | 命令式 prompt/confirm、设置、Ctrl+M 模型选择、项目管理/文件夹、新建工作区目录选择器 |
| 面板 | `panels/{files,git,terminal,browser,trajectory,agents,tasks,registry,models}.tsx` | 右栏各面板 |

### 9.4 状态与数据流

1. `bootstrap()` → `loadWorkspaceData()` 并行 `GET /api/state|models|sessions|projects|objects|skills|ui`；`workspaceGen` 丢弃过期响应。
2. 提交：`submitting` 锁 → 乐观追加用户消息 → 必要时取消归档 → `POST /api/run` → 900 ms 轮询 `/api/tasks/{id}`；SSE 同时推 progress（rAF 合帧、忽略旧 `revision`）。
3. 首次拿到 `trace_id` 启动轨迹分页（同 trace 复用 Promise，完成态按游标补洞）。
4. 终态 `finishTask`：切到结果会话、重载消息、项目行发起的草稿写回 `project_id`、刷新侧栏、播报。
5. 工作区切换：有运行中任务时拒绝；记忆每工作区当前会话，清空视图状态，重连 SSE，广播 `workspace-changed` 让设备面板重列。
6. SSE：`/api/events/stream` 更新 live 工具卡与轨迹增量（身份去重），`gap` 触发轨迹重载；`/api/live/stream` 推累计快照，`gap`/`open` 触发轮询。

### 9.5 项目树、附件与终端

- 项目按 `created_at` 排序；无 `project_id` 的会话归入“未分类”。展开状态乐观写入 store 后 `PATCH open` 持久化。
- 会话行：状态点、标题、最后模型、分支标记、相对时间、附属面板开关、菜单（打开/移动到项目/重命名/分支/归档/删除）；fork 按 `parent_id` 缩进最多 3 层；项目内按今天/昨天/更早分组，归档折叠。
- 附属行默认折叠（`attachOpenSet`），数量来自 `bridge.terminal/browser.counts()`。
- 浏览器标签按会话持久化到 `attachments.json`（300 ms 防抖）；工作区级标签只在内存。
- 终端按 `${workspacePrefix}::${sessionId}` 复用；附属会话重开时优先重连服务端活体 PTY（backfill 恢复历史）；输入 404 时自动重建。

### 9.6 修改前端的硬性要求

1. 改 `web/app` 后运行 `npm --prefix web/app run build` 并**提交 `web/dist`**；CI 会重新构建并比对。
2. 不引入运行期注入样式或内联脚本的依赖；新增 Radix 组件前检查其是否用 `react-remove-scroll`/`<style>`（契约测试有黑名单）。
3. 用户/模型/工具文本一律走 React 文本节点；HTML 只允许 `<Markdown>` 组件注入 `renderMarkdown()` 的净化输出。需要**执行**的 HTML（网页、小游戏）一律走沙箱预览，不得放宽工作台 CSP。
4. 流式正文不进 `aria-live`；阶段摘要通过 `announce()` 写入 sr-only status。
5. 跑 `node --test`、`node scripts/check-web-esm.mjs`；涉及布局或交互时用内嵌产物（真实 CSP）做浏览器验收。

---

## 10. 测试、CI 与脚本

### 10.1 Rust 测试（162 项）

| 套件 | 数量 | 覆盖重点 |
| --- | ---: | --- |
| `samho-cli` 单元测试 | 89 | 路由/静态资源/会话/项目/附件/工作区/任务索引/TUI（SSE/Markdown/视图/键位）/PTY（Unix 另有 1 项符号链接测试） |
| `samho-core` 内联单元 | 34 | Anthropic 7、tools 13、session 4、projects 3、context 2、budget 1、attachments 1 |
| `tests/agent_integration.rs` | 6 | 真实本地 HTTP Chat Completions 工具循环、失败/取消/截断参数、策略拒绝、恢复、数据目录锁、并发隔离 |
| `tests/core_contracts.rs` | 15 | Object 生命周期、路由约束、配置往返/校验、Skill、事件并发/恢复/轮转、脱敏、广播 |
| `tests/facet_integration.rs` | 6 | 握手/认证/白名单/traceparent/取消/断连/响应上限/Python fixture |
| `tests/mcp_integration.rs` | 5 | stdio 回收、分页、工具/资源/prompts、错误/超时/取消、HTTP |
| `tests/mcp_optional.rs` | 4 | sampling/elicitation/roots/completion/通知/tasks |
| `tests/oauth_integration.rs` | 3 | PKCE 回环、token 交换/刷新/撤销、失败不落凭据 |
| `tests/provider_streaming.rs` | 15 | 分片 UTF-8/CRLF、帧边界、错误分类、Retry-After、上限、背压、取消 |
| `tests/runtime_semantics.rs` | 10 | 重路由、预算硬顶、steering/follow-up、上下文投影、脱敏、恢复不重放、SQLite 重启、overage |
| `tests/skill_policy.rs` | 5 | `.skillignore`、冲突、刷新、资源限制、allowed-tools、网络策略 |
| `tests/storage_lifecycle.rs` | 9 | 跨 chunk 脱敏、file/SQLite 等价、legacy 迁移、损坏拒绝、尾部恢复、事件轮转、Object 替换 |

运行单个测试：

```powershell
cargo test -p samho-core --test runtime_semantics
cargo test -p samho --locked server::tests::health_pages_and_static_assets
```

### 10.2 Node 契约测试（12 项）

```powershell
node --test scripts/test-workbench-contracts.mjs
```

- `test-workbench-contracts.mjs`（12）：轨迹搜索安全、HTML 注入白名单（仅 `<Markdown>` 组件 + DOMPurify 禁用标签/属性清单）、沙箱预览不同源且禁网络、增量追加去重、工作区 generation、提交锁、移动抽屉、aria、项目树状态、CSP 依赖黑名单与产物无内联脚本/样式、产物被内嵌、后端路由存在性。

### 10.3 CI（`.github/workflows/ci.yml`）

| Job | 内容 |
| --- | --- |
| `contracts` | ubuntu/windows/macos 矩阵：fmt → 前端 `npm ci` + build + `git diff --exit-code web/dist` → test → clippy → Node 契约 + 产物检查 → release 构建 → `smoke_runtime.py` → 上传 `reports/ci-smoke.json` |
| `msrv` | ubuntu + Rust 1.88.0：`cargo check` + `cargo test` |
| `audit` | RustSec 公告扫描 |

第三方 Action 全部固定 commit SHA；Dependabot 每周只更新 GitHub Actions。CI 不跑浏览器验收与基准。

### 10.4 脚本

| 脚本 | 用途 | 默认输出 |
| --- | --- | --- |
| `scripts/smoke_runtime.py` | 跨平台真实二进制冒烟：health/模拟状态/任务/会话/事件分页/UI 配置 | `reports/ci-smoke.json` |
| `scripts/smoke-web.ps1` | Windows Web GUI/会话续接/工具循环/重启恢复 | `reports/release-web-smoke.json` |
| `scripts/benchmark-runtime.ps1` | 外部进程 Demo 基准（p50/p95/峰值内存） | `reports/benchmark-runtime.json` |
| `scripts/test-workbench-contracts.mjs` | 工作台安全/状态/API 契约 | stdout |
| `scripts/check-web-esm.mjs` | 构建产物 `web/dist/assets/app.js` 语法与单文件（无顶层 import/export）检查 | stdout |

所有生成报告的脚本默认拒绝覆盖，需 `--force` / `-Force`。

---

## 11. 常见开发任务

### 11.1 新增内置工具

1. 在 `crates/samho-core/src/tools.rs` 定义输入/输出 schema（`ObjectEnvelope.spec` + `output_schema`）。
2. 在 `ToolRegistry::new` 中按配置开关注册；新增开关则同时改 `config::ToolPolicy` 默认值与 `validate`。
3. 实现路径/网络/取消/输出截断；所有文件路径过 `within_workspace`。
4. 在 `web/app/src/components/timeline-items.tsx` 的 `registry` 加标题/图标/参数徽标（可选，但推荐）。
5. 补集成测试：授权、schema 拒绝、取消、截断、越界。
6. 更新本文件 §7.5、`docs/customization.md` 与 README 工具列表。

### 11.2 新增 Provider

1. `model.rs` 的 `ProviderKind` 加枚举值（serde snake_case）。
2. `config.rs::validate` 补必填项（通常 `base_url`）。
3. `provider.rs` 的 `complete_typed` / `complete_stream` 加分发分支；实现 `ModelProvider`。
4. 复用 `ProviderError`、`ProviderLimits`、SSE 解析与脱敏；不要自建错误字符串协议。
5. 补单元测试（请求体、错误映射、流式分片）与 `examples/` 配置。
6. 更新 §6.3、§7.3 与 `docs/provider-streaming.md`。

### 11.3 新增 HTTP 路由

1. 在 `server.rs` 写 handler，返回 `ApiError` 或 `Json`；所有输出过 redactor。
2. 在 `router_with_state` 注册方法/路径。
3. 若返回 SSE：设置 `text/event-stream`、`Cache-Control: no-store`、keepalive 与 gap 语义。
4. 补 `server.rs` 单元测试（状态码、错误形状、脱敏、未知字段）。
5. 若前端使用，在 `web/app/src/lib/controller.ts` 或对应面板调用，并重新构建 `web/dist`。
6. 更新本文件 §8.3。

### 11.4 新增配置字段

1. 在对应 struct 加字段，带 `#[serde(default)]`；结构体保持 `deny_unknown_fields`。
2. 在 `Default` impl 给值。
3. 在 `validate()` 加范围/互斥校验，错误信息写明字段。
4. 更新 `examples/*.toml`、`config init` 模板与本文件 §6。
5. 补往返/非法值测试（`core_contracts.rs` 风格）。

### 11.5 新增 Web 面板

1. `web/app/src/components/panels/<name>.tsx` 导出 `<Name>Panel({ active })`；`active` 控制轮询/首次加载，监听 `workspace-changed` 重列。
2. 在 `lib/store.ts` 的 `DEVICE_TABS` / `INSPECT_TABS` 与 `PanelName` 加入名称。
3. 在 `components/inspector.tsx` 的 `META`（标签/图标/aria）与 `PanelSlot` 列表注册。
4. UI 只用 `components/ui/*` 的 shadcn 组件；需要新 shadcn 组件时手写到 `components/ui/`（统一从 `radix-ui` 包导入），并确认不注入 `<style>`。
5. 补 `test-workbench-contracts.mjs` 静态断言（如涉及安全边界）。
6. `npm --prefix web/app run build` → `cargo build` → 用内嵌产物做浏览器验收。

### 11.6 修改会话/journal 结构

1. 尽量只加带默认值的字段；不要改变 `Delta` 的校验算法。
2. 若必须升 schema：同时更新 `SESSION_SCHEMA_VERSION`、旧格式探测、迁移路径与 `.legacy.jsonl` 备份。
3. 补 file/SQLite 双后端等价测试、尾部恢复、损坏拒绝、fork 边界。
4. 更新 `docs/storage-lifecycle.md` 与本文件 §7.6。

### 11.7 调整路由/模型角色

- 全局层级与评分：`RuntimeConfig.routing`。
- 任务类型到模型的固定绑定：Web“模型”面板或 `PUT /api/model-roles`，落 `model_roles.json`。
- 单个任务的层级/模型：`TaskProfile.tier` / `.model` / `.intelligence_tier`。
- 显式 `profile.model` 永远优先于 RoleRouter。

---

## 12. 编码规范与不变量

1. **serde**：配置/请求结构 `#[serde(default, deny_unknown_fields)]`；新增字段必须有 `Default`；错误信息具体到字段。
2. **格式与 lint**：`cargo fmt --all`；clippy `-D warnings`；不引入 `unwrap()` 处理外部输入。
3. **秘密**：只从环境变量读取；持久化、广播、progress、HTTP 错误四条路径都必须过 `Redactor`。
4. **取消**：长操作使用 `select!` 取消分支；取消后不得继续写外部副作用。
5. **副作用未知**：已调用工具/远程服务后失败，错误必须说明“效果未知，需人工核对”，恢复时不得自动重放。
6. **错误具体**：禁止静默回退；Provider 失败不得回退到 Demo；MCP 未实现能力返回 `Unsupported`。
7. **原子写**：临时文件 + rename；JSONL 失败回滚偏移；SQLite 用事务；只有完整校验后才截断崩溃尾部。
8. **端口**：测试/fixture 只用 300–400；生产端口不限。
9. **证据**：新功能先补 fixture/契约测试，再宣称“已实现”；区分“本地 fixture 验证”与“真实供应商/跨平台验收”。
10. **文案**：用户可见中文；事件 kind、payload 键、配置键、日志键保持英文。
11. **文档同步**：改行为后更新本文件对应章节、README 与相关 docs 文档。
12. **提交信息**：`类型: 中文摘要`（feat/fix/docs/test/ci/chore），与现有 git 历史一致。
13. **不要提交**：`.samho/`、`target/`、`reports/` 生成物、`__pycache__/`、`.zcode/`。

---

## 13. 已知问题、边界与排障

### 13.1 已确认缺陷（截至 2026-11-11）

| 位置 | 问题 | 影响 |
| --- | --- | --- |
| `server.rs` + `tui/mod.rs` | `PUT /api/ui` 写 `data_dir/ui.json`，启动时不回读 | 服务重启后主题/快捷键回到配置文件或默认值 |
| `tui/mod.rs` | `/theme contrast` 被服务端拒绝 | 该命令返回 400 |
| `panels.rs::git_diff` | 按固定字节偏移截断 UTF-8 | 截断点落在多字节字符中间时可能 panic |
| `anthropic.rs` 流式 | 无总字节/单帧/工具数上限 | 恶意或异常流可能占用过多内存 |
| `GET /api/sessions/{id}/attachments` | 不校验会话存在 | 不存在的会话返回空默认值而非 404 |
| `GET /api/tasks` | 注释称“运行中优先”，实际按 UUID 键逆序 | 顺序不符合直觉 |
| `web/dist/assets/app.js` | 单文件约 1.0 MB（gzip 约 330 KB；markdown-it/highlight.js/Temml/DOMPurify 约占一半），Vite 提示超过 500 KB | 仅本地/局域网加载，暂不做代码分割（分割会引入哈希文件名，需改内嵌方式） |

### 13.2 设计边界（不是 bug）

- 任务索引只在内存：完成条目重启后 404；活动任务不会跨进程恢复。
- `task.start` 恢复只补“未知副作用”观察，不重放工具。
- 无 SIGTERM 处理，只有 Ctrl+C 优雅关闭。
- 事件只有字节轮转，没有按时间/条数保留。
- Anthropic 不回传 thinking 块；`reasoning_content` 只用于展示。
- `mcp/oauth.rs` 未接入 MCP 配置，不会自动做 OAuth。
- 项目删除不级联删除会话；会话删除会清附件，但不删终端 PTY 之外的持久文件（PTY 为内存资源）。
- `/api/browse*` 是设备级、无 workspace 限制，因为工作区选择器需要浏览任意目录；服务无认证时这是有意的权衡。
- 服务无认证、无 CORS、无 CSRF token；Host/Origin 检查不是授权。

### 13.3 排障

| 现象 | 原因与处理 |
| --- | --- |
| `data directory is already in use… os error 33` | 另一个 `samho` 进程持有同一 `data_dir`；用 TUI/Web 连接它，或给 CLI 设置独立 `SAMHO_DATA_DIR` |
| `Address already in use` | 端口被占用；换 `--bind` 或 `SAMHO_HTTP_BIND` |
| 浏览器 403 `this API accepts local hosts only` | Host 不在允许列表；用 `localhost`/`127.0.0.1`/本机网卡 IP 访问 |
| 403 `cross-origin requests are not allowed` | 反向代理或跨源页面；Origin 必须严格等于 `http://{Host}` |
| 413 | 请求体超过 1 MiB；`PUT /api/file` 也受限 |
| 400 `unknown field …` | 配置/请求结构 `deny_unknown_fields`；删掉多余键 |
| Anthropic 返回空 content | 思考 token 吃掉了 `max_output_tokens`；调大（≥512，建议 4096+） |
| 模型路由结果与预期不符 | `automatic=true` 会忽略传入 complexity；改用 `automatic=false` + `complexity/tier/model` |
| Demo 任务正常但真实模型 401 | `api_key_env` 指向的环境变量不存在或为空 |
| MCP server 显示 failed | 检查 command/url、启动超时、`scrub_env` 是否误删所需变量；单个失败不阻塞其他 server |
| Facet 启动失败 | `AgentRuntime::build` 会失败；检查 endpoint、token env、能力白名单、HTTPS/回环设置 |
| `cargo test` 卡住 | MCP stdio 测试需要 Python 3；PTY 测试在 Windows 上会启动真实 shell |
| `CLAUDE.md` 无法创建软链接 | Windows 未开启开发者模式/非管理员；本仓库使用 `@agent.md` 导入回退（见 §14.4） |

---

## 14. 文档地图与维护规则

### 14.1 文档清单

- 开始使用：`docs/中文使用说明.md`、`README.md`
- 实现设计：`docs/architecture.md`、`docs/customization.md`、`docs/mcp-support.md`、`docs/implementation-status.md`、`docs/remote-facets.md`
- 验证与交付：`docs/validation-plan.md`、`docs/test-port-policy.md`
- 其他：`docs/installation-tui.md`、`docs/provider-streaming.md`、`docs/storage-lifecycle.md`

### 14.3 权威顺序

1. 源码与测试。
2. `agent.md`（本文件）。
3. `README.md` 与 `docs/implementation-status.md`（状态与边界）。
4. 其余设计文档（可能滞后，读时对照源码）。

### 14.4 `CLAUDE.md` 的说明

本仓库希望 `CLAUDE.md` 指向 `agent.md`。Windows 上创建符号链接需要开发者模式或管理员权限；当前环境两者都不可用，因此 `CLAUDE.md` 使用 Claude Code 支持的导入语法：

```markdown
@agent.md
```

这不会复制内容，读取时由 Claude Code 展开，效果等同于指向本文件。若以后在开发者模式下工作，可删除该文件并执行：

```powershell
New-Item -ItemType SymbolicLink -Path CLAUDE.md -Target agent.md
```

或（管理员命令提示符）：

```cmd
mklink CLAUDE.md agent.md
```

---

## 附：近期变更摘要（2026-11-11）

- 对话完整渲染 Markdown 与 HTML：markdown-it + highlight.js + Temml（MathML）+ DOMPurify；代码块复制/沙箱预览；文件面板 Markdown 渲染视图与 HTML 运行预览；工具卡对 `.md`/`.html` 文件提供渲染与预览。新增 `/api/preview*` 沙箱路由（`preview.rs`）。
- 默认 API 改为本机 OpenAI 兼容网关 `http://127.0.0.1:3425/v1` + `wuyou/deepseek-v4.1-flash`（`config init`、`examples/samho.deepseek.toml`、RoleRouter 默认绑定）。
- 真实模型 10 项任务验收中发现并修复：`write_file` 无法一次创建多级新目录（`within_workspace` 只检查直接父目录）；Windows `shell` 输出 GBK 乱码且模型误用 bash 语法（改 UTF-8 包装 + PowerShell 方言说明）；`read_file` 拒绝 PowerShell 重定向产生的 UTF-16 文件；工具结果路径带 `\\?\` 前缀与反斜杠。
- Web 工作台重写为 React + TypeScript + Vite + Tailwind v4 + shadcn/ui（`web/app` → `web/dist`），移除旧版原生 ES Module UI（`web/index.html`、`web/app.css`、`web/ui/`、`web/js/`）；功能与 API 调用保持一致，顺带修复旧版文件删除与会话删除报错。

- 全量统一命名为 Samho：crate、二进制、环境变量、数据目录、页面文案。
- 默认监听改为 `0.0.0.0:7432`，启动日志与顶栏显示局域网地址；保留 Host/Origin 防护与无认证警告。
- 新增 Anthropic Messages Provider（`anthropic.rs` + 示例配置 + 测试）。
- 新增项目制工作区：项目/文件夹/会话树、会话附属终端/浏览器、默认项目迁移。
- 新增动态工作区（`hub.rs`）、任务类型 → 模型角色（`roles.rs`）、任务索引字节预算。
- 清理旧版 UI、历史报告与 43 GB 构建缓存；仓库体积约 45 GB → 7.8 GB。
- 当前基线：155 项 Rust 测试 + 21 项 Node 契约全部通过；release 冒烟 5/5；静态/API 路由验证通过。
