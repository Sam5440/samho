# 跨平台安装与终端操作

## 构建环境与锁定依赖

本项目要求 Rust 1.88 或更高版本。复现实验请记录 rustc -Vv、Cargo.lock、二进制 SHA-256、操作系统和硬件；不同编译器或硬件不应直接比较加速倍率。上一次 Windows 本机构建使用 Rust 1.96.0，GitHub CI 使用当前 stable 并分别记录日志，不能视为相同编译器基准。

从源码构建：

~~~powershell
cargo fetch --locked
cargo build --release --locked -p samho
~~~

Windows 可执行文件为 target/release/samho.exe；Linux/macOS 为 target/release/samho。SQLite 采用 bundled SQLite，无需用户安装独立数据库。Windows 需要 Rust 的 MSVC 构建工具，Linux 需要系统 C 编译器，macOS 需要 Xcode 命令行工具。

MCP Python 样例测试需要 Python 3，中文网页契约需要 Node 22；这些不是模型供应商凭据。TUI 额外依赖 pulldown-cmark、syntect（纯 Rust fancy-regex 后端）与 webbrowser，均为 MIT/Apache 许可并锁进 Cargo.lock。

## 中文命令行与动态层级

~~~powershell
cargo run -p samho -- serve            # 默认 0.0.0.0:7432，局域网可达
cargo run -p samho -- tui --url http://127.0.0.1:7432
cargo run -p samho -- run "设计模块接口" --intelligence-tier architect
cargo run -p samho -- fork 会话ID --cutoff 3 --title "审阅分支"
~~~

服务没有身份认证；只希望在本机访问时使用 `serve --bind 127.0.0.1:7432`，局域网地址见启动日志、TUI 状态栏与网页顶栏。

architect 必须在配置 routing.tiers 中存在；普通 --tier 仍用于六个内置兼容层级。TUI 的 F2 层级列表从服务 state.tiers 获取，不把自定义层级写死在终端。

## 终端视图与键位

TUI 与网页工作台功能对齐：六个视图（`Alt+1..6`）为对话、任务看板、文件、Git、终端、浏览器；`Esc` 返回对话，`F7` 切换工作区，`Ctrl+Q` 退出。

- **对话（Alt+1）**：项目侧栏（分组/搜索/行菜单，`Shift+Tab` 与输入框、检查面板循环焦点）、流式 Markdown 转录（标题、嵌套列表、GFM 表格、任务清单、代码高亮、引用）、工具卡与思考块折叠（`Ctrl+R`）、模型浮层（`Ctrl+M`）、技能多选（`F6`）。
- **检查面板（Tab 循环）**：轨迹调用卡（展开/搜索/`E` 导出）、预算、对象、技能（`R` 刷新）、MCP、模型角色（`Enter` 改派、`R` 恢复默认）。
- **任务（Alt+2）**：四段看板，`s` 停止、`Enter` 打开会话。
- **文件（Alt+3）**：目录浏览、`Enter` 打开、`M` Markdown 渲染切换、`e` 多行编辑器 + `Ctrl+S` 保存、`n`/`N` 新建、`r` 改名、`d` 删除、`s` 设树根、`p` 沙箱预览、图片外开。
- **Git（Alt+4）**：状态/diff/历史/分支四页，`s`/`S`/`u` 暂存操作、`D` 丢弃、`c` 提交、`b` 分支切换与新建。
- **终端（Alt+5）**：`n` 新建、`Enter` 接管——挂起 TUI 透传 PTY（vim/htop 等全屏程序可用），`Ctrl+Q` 脱离保活、重入续接历史，`K` 关闭。
- **浏览器（Alt+6）**：`o`/`u` 输入 URL 系统浏览器打开；`t` 文本模式经 `/api/proxy` 剥标签滚动阅读。

终端读取 GET /api/ui。支持 Ctrl、Alt、Shift 和字母、Enter、Tab、F1–F24 的组合。默认使用 Ctrl+Enter 提交、Alt+N 新建、Ctrl+K 取消；部分终端不能区分 Ctrl+Enter 与 Enter，因此默认配置额外提供 Enter 兼容提交，自定义提交键则严格匹配。

Shift+Enter 插入换行。Ctrl+C 取消运行任务或在空闲时退出。

输入以下命令并按提交键执行：

- /help：显示中文命令帮助。
- /steer 内容：向运行中的任务追加引导（运行中裸 Enter 亦然）。
- /follow-up 内容 / `Ctrl+F` 切换 follow-up 模式：把后续内容放入任务队列。
- /fork [消息截止数] [标题]：创建独立分支，不修改父会话。
- /theme dark、light 或 contrast：更新共享界面主题。
- /bind submit|new|cancel 快捷键：更新共享提交、新建或取消快捷键。
- /auto：层级跟随自动路由；/complexity 0-10：设置任务复杂度。

主题与快捷键通过 PUT /api/ui 生效，下一次资源更新同步显示。终端展示累计脱敏的 progress 快照，后续快照替换之前内容，避免重复拼接或泄漏跨分块凭据。进度与轨迹走 SSE 长连接（断线 1s→8s 退避重连），资源快照 800ms 轮询兜底。`log_content = false` 时轨迹面板显示"内容脱敏"徽标，与网页一致。

## 本地验证与跨平台 CI

~~~powershell
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked --no-deps -- -D warnings
node --test scripts/test-chinese-pages.mjs
python scripts/smoke_runtime.py --binary target/release/samho
~~~

.github/workflows/ci.yml 配置 Linux、macOS 和 Windows 三平台，执行 Rust 测试、静态检查、中文网页契约、发布构建及真实本地服务冒烟，保留 reports/ci-smoke.json。冒烟脚本使用临时目录、随机回环端口和明确模拟模型，不调用收费供应商。

当前仅在本机实际运行过的命令能标为通过；添加 workflow 文件不等于 GitHub 上的三平台 CI 已运行，更不等于真实终端像素一致性、供应商模型质量或完整 MCP conformance 已通过。TUI TestBackend 覆盖窗口大小、CJK、组合字符、emoji 布局、Markdown 渲染、SSE 解析与各视图键位（Rust 测试 201 项），但实际终端仍需独立 PTY 验收——尤其 Windows Terminal/ConPTY 下的终端接管模式。
