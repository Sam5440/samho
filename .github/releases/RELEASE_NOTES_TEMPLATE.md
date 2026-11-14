# Samho · 安装与使用教程

本地优先的 Agent 运行时：一份 Rust 核心同时驱动网页工作台与终端 TUI。本节为随版本更新的完整上手教程；「完整变更」清单由 GitHub 自动生成并附在教程之后。

## 📥 下载安装

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows x64 | `samho-x86_64-pc-windows-msvc.zip` | 静态链接 CRT，无需安装 VC 运行库 |
| Linux x64 | `samho-x86_64-unknown-linux-gnu.tar.gz` | Ubuntu 24.04 构建（glibc 2.39）；更老发行版请从源码构建 |
| macOS Apple Silicon | `samho-aarch64-apple-darwin.tar.gz` | macOS 14 构建 |

每个压缩包含：`samho` 可执行文件、`README.md`、`LICENSE`、`examples/` 配置示例。完整性校验见 `SHA256SUMS.txt`。

### Windows

1. 下载 `samho-x86_64-pc-windows-msvc.zip`，解压到任意目录（如 `C:\Tools\samho`）
2. 可选：把该目录加入 `PATH`
3. 验证：

```powershell
Expand-Archive samho-x86_64-pc-windows-msvc.zip -DestinationPath C:\Tools\samho
cd C:\Tools\samho
.\samho.exe --help
```

### Linux / macOS

```bash
tar -xzf samho-*.tar.gz
chmod +x samho
./samho --help
```

无需其他运行时依赖；访问真实模型只需要一个 OpenAI 兼容（或 Anthropic 协议）网关与其 API key。

## 🚀 三步上手

### 第 1 步：生成配置并设置密钥

```bash
samho config init samho.toml
```

- 目标文件已存在时会拒绝覆盖；省略的配置节在加载时回退内置默认值
- 默认 Provider 是明确标注 `[DEMO simulation]` 的离线模拟器——不配置密钥也能完整跑通全流程
- 接真实模型：先设置密钥环境变量（默认模板读取 `SAMHO_API_KEY`），密钥只从环境变量读取，不写入配置文件

```powershell
setx SAMHO_API_KEY "你的密钥"   # Windows（重开终端生效）
export SAMHO_API_KEY="你的密钥"  # Linux / macOS
```

### 第 2 步：启动服务（网页工作台）

```bash
samho serve --bind 0.0.0.0:7432
```

- 本机访问 `http://127.0.0.1:7432`；启动日志与页面顶栏会显示局域网地址
- `--bind 127.0.0.1:7432` 可收回本机
- 指定配置：`samho --config samho.toml serve`；配置在进程启动时读取，修改后需重启服务

### 第 3 步：连接 TUI 或使用 CLI

另开终端连接同一个服务（网页与 TUI 共享会话、任务与轨迹）：

```bash
samho tui --url http://127.0.0.1:7432
```

不起服务的本地 CLI 三件套：

```bash
samho run "/demo read README.md" --complexity 2 --json   # 执行一条任务并输出 JSON
samho inspect                                            # 查看运行时状态
samho replay --export trace.jsonl                        # 导出调用轨迹
```

> `run` / `inspect` / `replay` 会打开本地数据目录；服务运行时应改用 TUI/Web 访问同一运行时，或为 CLI 设置另一数据目录——文件锁会拒绝两个进程同时写同一目录。

## ⚙️ 接入真实模型

编辑 `samho.toml` 的 `[[models]]` 段（也可直接参考压缩包内 `examples/` 下的完整示例：OpenAI 兼容 `samho.deepseek.toml`、`samho.chat.toml` 与 Anthropic 协议 `samho.anthropic.toml`）：

```toml
[[models]]
id = "deepseek-v4.1-flash"          # RoleRouter 默认绑定按此 id 匹配
model = "wuyou/deepseek-v4.1-flash" # 供应商处的物理模型 ID
provider = "chat_completions"       # chat_completions | anthropic | demo
base_url = "http://127.0.0.1:3425/v1"
api_key_env = "SAMHO_API_KEY"
tier = "reasoning"
context_window = 272000
max_output_tokens = 32768
```

要点：模型分级共六级 `nano / fast / balanced / reasoning / frontier / specialist`，可交给自动路由按任务复杂度选择，也可手动指定 tier 或 model；上下文窗口、费用、能力全部显式声明，分级标签不代表已测性能。

## 🔒 安全须知

- `serve` **没有身份认证**，也不是 OS 沙箱：只应在可信局域网使用
- 默认工具策略包含 `shell` 与 `write`：Agent 以服务进程的 OS 用户权限执行命令、写 workspace 内文件；处理不可信内容时在 `[tools]` 中显式关闭 `shell`
- 文件工具拒绝符号链接目标与越界路径，`workspace` 之外不可写
- `log_content = false` 可省略轨迹正文（会话恢复仍保存经脱敏的完整消息）

## 📚 文档

| 文档 | 内容 |
| --- | --- |
| [中文使用说明](https://github.com/Sam5440/samho/blob/master/docs/中文使用说明.md) | 启动、网页/TUI 操作、任务执行、复盘 |
| [agent.md](https://github.com/Sam5440/samho/blob/master/agent.md) | 权威工程参考 |
| [自定义配置说明](https://github.com/Sam5440/samho/blob/master/docs/customization.md) | 配置全字段与扩展方式 |
| [MCP 支持说明](https://github.com/Sam5440/samho/blob/master/docs/mcp-support.md) | MCP 能力清单与未实现项 |
| [实现状态](https://github.com/Sam5440/samho/blob/master/docs/implementation-status.md) | 当前能力边界 |

## 🛠️ 从源码构建（可选）

需要 Rust 1.88+（修改 Web 工作台另需 Node 22）：

```bash
git clone https://github.com/Sam5440/samho.git
cd samho
cargo build --release
cargo run -p samho -- serve --bind 0.0.0.0:7432
```

## ⚠️ 已知边界

- 三平台二进制由 CI 自动构建并通过真实服务冒烟，但 macOS/Linux 尚未做长期实机验收
- 完整 MCP 官方 conformance、真实供应商模型质量、24 小时 soak 仍未验收；限制不能由本地模拟器结果推断
- trace replay 为事件查看/导出，不会重执行外部副作用
