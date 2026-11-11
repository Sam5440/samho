# Samho

Pi coding agent 的 Rust 重写工程。已下载并分析完整上游仓库，当前交付为可运行的增量重写版本：Rust 核心、共享本地服务、Ratatui TUI 和本地 Web GUI。未完成项仍按实现状态文档保留边界。

## 启动

需要 Rust 1.88+；MCP stdio 集成测试还需要 Python 3。

```powershell
cargo run -p samho -- serve --bind 0.0.0.0:7432
# 本机打开 http://127.0.0.1:7432；局域网地址见启动日志与页面顶栏
```

另开终端连接同一个服务：

```powershell
cargo run -p samho -- tui --url http://127.0.0.1:7432
```

默认 Provider 是明确标注 `[DEMO simulation]` 的离线模拟器，不提供真实模型推理。以下命令可独立验证真实文件读取、会话持久化和调用轨迹：

```powershell
cargo run -p samho -- run "/demo read README.md" --complexity 2 --json
cargo run -p samho -- inspect
cargo run -p samho -- replay --export trace.jsonl
```

`run`、`inspect` 和 `replay` 会打开本地数据目录；服务运行时应使用 TUI/Web 访问同一运行时，或为独立 CLI 设置另一数据目录。文件锁会拒绝两个进程同时写同一目录。

生成可编辑的完整配置：

```powershell
cargo run -p samho -- config init samho.toml
cargo run -p samho -- --config samho.toml serve
```

真实 Provider 示例见 [examples/samho.chat.toml](examples/samho.chat.toml)。填写端点、物理模型 ID、能力、费用与延迟后，通过指定环境变量读取 API key。所有模型配置均由使用者维护；分级标签和延迟提示不代表已测模型性能。

## 已实现

- **项目制工作区管理**：工作区内按「项目」组织——项目可关联多个 workspace 文件夹（entry 为主目录，新会话终端/文件面板默认起始），项目下展开多个会话；每个会话可挂附属终端/浏览器（默认折叠，浏览器标签持久化到 attachments.json，重启后按会话恢复）。存量会话首次启动自动归入「默认项目」（幂等迁移）。
- Object 公共契约、依赖拓扑、失败回滚、逆序销毁，以及运行时/Model/Tool/Skill/MCP server 的注册与调用。
- 异步 Agent 循环、任务并发限制、取消、失败状态、会话持久化与重启恢复；可注册自定义 Rust Tool。
- 六级模型策略：`nano / fast / balanced / reasoning / frontier / specialist`，可配置分类规则、评分权重、上下文、预算、延迟和能力约束；手动指定 tier 或 model。
- Chat Completions 兼容 Provider，显式模拟 Provider；真实 HTTP fixture 验证模型→工具→模型循环。
- JSONL trace、序列号、trace/session/span/parent 关联、路由理由、工具参数与结果摘要、usage、耗时、脱敏、导出和 SSE。
- MCP stdio/Streamable HTTP；工具、资源、模板、prompts、分页、超时、取消和子进程回收，基于 `rmcp 3.5.1`。
- `SKILL.md` frontmatter 校验、确定性发现与冲突诊断、按需正文加载及显式用户调用。
- 工作区文件工具（read/write/edit/list_files/glob/grep/webfetch/todo）。**write 与 shell 默认开启**：shell 不是沙箱，命令以服务进程的 OS 用户权限执行，可访问 workspace 之外的系统资源；处理不可信内容时请在 `[tools]` 中显式关闭（见下文安全边界）。
- Web GUI 与 TUI 共用 Rust 服务；任务提交/取消、会话、模型、Object、Skill、MCP 状态和 trace 查询。

Runtime 已将 Session/Task 纳入 Object 生命周期，并提供 scope、middleware、lease 和 generation 热替换。远程 Facet、生产级跨进程隔离和跨平台运维仍按状态文档列为后续验收项。自定义范围和限制见 [docs/customization.md](docs/customization.md)。

## 验证

```powershell
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
node --test scripts/test-chinese-pages.mjs scripts/test-workbench-contracts.mjs
```

Rust 当前 workspace 共 **155 项测试**（CLI 46、core 单元 31、Agent 6、契约 15、Facet 6、MCP 5、MCP optional 4、OAuth 3、Provider streaming 15、runtime semantics 10、Skill policy 5、storage lifecycle 9，另 0 个 doc test）。测试使用本地 fixture，无需供应商凭证。CI 另含 Rust 1.88 MSRV 检查与 RustSec 依赖公告扫描；第三方 Action 固定 commit SHA 并由 Dependabot 维护升级。

本机验收脚本默认拒绝覆盖既有报告，重跑需显式 `-Force`（PowerShell）或 `--force`（Python）：

```powershell
powershell -NoProfile -File scripts/smoke-web.ps1 -Force
powershell -NoProfile -File scripts/benchmark-runtime.ps1 -Force
python scripts/smoke_runtime.py --force
```

浏览器验收已覆盖自动分级、同会话续接、手动 reasoning tier、固定 model、重启恢复与 trace/Object 展示；桌面及 560×900 窄屏无水平溢出，控制台无 error/warning。历史验收截图与报告已清理；当前回归由 `scripts/smoke_runtime.py`、`scripts/smoke-web.ps1` 与 Rust/Node 契约测试持续覆盖。浏览器验收未配置实际 MCP server/Skill，也未调用真实语言模型；对应协议和加载逻辑由 Rust 集成测试覆盖。

当前实现状态和性能边界见 [docs/implementation-status.md](docs/implementation-status.md)。release 本地基准脚本为 `scripts/benchmark-runtime.ps1`；不据此宣称相对 Pi 的速度倍率。

## 中文文档与确认计划

- [中文使用说明](docs/中文使用说明.md)：中文启动、网页/TUI 操作、任务执行、复盘和配置入口。
- [中文确认计划说明](docs/中文确认计划说明.md)：网页 `/plan` 的勾选、导出、回传和执行规则。
- [当前架构](docs/architecture.md)、[自定义配置](docs/customization.md)、[实现状态](docs/implementation-status.md)。

打开本地服务后访问 `/docs` 阅读中文文档，访问 `/plan` 勾选下一阶段目标并下载 `samho-确认执行计划-YYYY-MM-DD.txt`。网页只生成计划文本，不自动执行；将完整文本回传本对话后，才按确认范围开展实现。

也可直接打开本地 [中文文档中心](web/docs.html) 与 [确认计划网页](web/plan.html)，无需先启动 Rust 服务。真正运行 Agent 任务仍需启动服务。

## 上游与分析

上游参考仓库（pi/dsh/opencode/zcode）为分析期临时 clone，未入库，现已随本轮清理移除；固定提交记录保留在 [docs/upstream-lock.json](docs/upstream-lock.json)，需要时可据此重新 clone。

| 项目 | 固定提交 |
| --- | --- |
| Pi，`earendil-works/pi` | `2db5e359bf84c1c0be51d2c5c5c5c7cf27072c2b` |
| DSH，`deepseek-ai/deepseek-harness` | `5badb15009ae1756c3afe0ae0cef1faafc290ccc` |
| ZCode，`zai-org/ZCode` | `29628c9acdb81b703bbd4080c207a0e7ce5e276e` |

源码证据与重写设计：

- [Pi 全面分析](docs/pi-analysis.md)：模型/provider、Agent、工具、持久化、会话树、TUI、扩展与性能热路径。
- [DSH/ZCode 参考分析](docs/reference-analysis.md)：DSH 实际采用 Everything is a Plugin；Object 设计保留类型契约和生命周期。
- [当前架构](docs/architecture.md)、[MCP 能力矩阵](docs/mcp-support.md)、[多场景验证计划](docs/validation-plan.md)。

## 当前边界

当前已实现 Provider streaming、上下文投影与 lineage、会话 fork/steering、失败重试与降级路由、SQLite/file journal、事件索引分页、MCP optional fixture、OAuth PKCE、Skill 权限策略、Facet 握手和 generation lease。完整 MCP 官方 conformance、真实供应商质量、跨平台实机、24 小时 soak 与 Pi/Rust 同机性能对照仍未验收；这些限制不能由本地模拟器结果推断。

`log_content = false` 仅省略 trace 正文；会话恢复仍保存经脱敏的完整消息。trace replay 当前为事件查看/导出，不会重执行外部副作用。真实模型质量、官方 MCP conformance、跨平台 TUI 与 24 小时稳定性测试尚未完成。性能目标必须以同机、同输入、同语义的 Pi 对照实验校准。

## 安全边界

- `serve` 默认绑定 `0.0.0.0:7432` 并拒绝跨源请求，但**没有身份认证**，也不是 OS 沙箱；只应在可信局域网使用，可用 `--bind 127.0.0.1:7432` 收回本机。启动日志与页面顶栏会显示可用的局域网地址。
- 默认工具策略包含 `shell` 与 `write`：Agent 可以以当前用户权限执行任意命令、写 workspace 内文件。处理不可信内容（外部仓库、网页、MCP 数据）时建议在 `runtime.toml` 的 `[tools]` 中关闭 `shell`。
- 文件面板与内置文件工具都会拒绝符号链接目标与越界路径；Git 面板操作有超时与输出上限。
- 性能基线与不同版本二进制体积不可直接互比；比较需同 commit、同构建条件、同工作负载。
