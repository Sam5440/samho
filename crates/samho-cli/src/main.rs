mod hub;
mod panels;
mod preview;
mod roles;
mod server;
mod term;
mod tui;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use samho_core::{AgentRuntime, RuntimeConfig, TaskProfile, TaskRequest};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "samho", version, about = "面向 Object 的 Rust Agent 运行时")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 在本地运行任务；默认 Provider 是明确标注的模拟器。
    Run(RunArgs),
    /// 在本机提供共享 Agent API 与网页工作台。
    Serve {
        #[arg(long)]
        bind: Option<String>,
    },
    /// 连接正在运行的 `samho serve` 服务。
    Tui {
        #[arg(long, default_value = "http://127.0.0.1:7432")]
        url: String,
    },
    /// 从已有会话的完整工具调用边界创建分支。
    Fork {
        session: String,
        #[arg(long)]
        cutoff: Option<usize>,
        #[arg(long)]
        title: Option<String>,
    },
    /// 查看运行时配置，不执行任务。
    Inspect,
    /// 创建完整且可编辑的运行时配置。
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// 打印轨迹事件，或导出 JSONL。
    Replay {
        trace_id: Option<String>,
        #[arg(long)]
        export: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    Init {
        #[arg(default_value = "samho.toml")]
        path: PathBuf,
    },
}

/// `config init` 生成的默认模板：接入本机 OpenAI 兼容网关（Chat Completions）。
/// 模型 id 与 RoleRouter 的默认绑定（DEFAULT_MODEL_IDS 首项 "deepseek-v4.1-flash"）
/// 一致，因此所有任务类型自动路由到该模型；省略的配置节回退内置默认值。
const DEFAULT_CONFIG_TEMPLATE: &str = r#"# Samho 运行时配置（`samho config init` 生成）
# 默认 API：http://127.0.0.1:3425/v1 ｜ 模型 wuyou/deepseek-v4.1-flash ｜ OpenAI Chat Completions 协议
# 认证密钥只从环境变量读取：Git Bash `export SAMHO_API_KEY=magpie`；Windows `setx SAMHO_API_KEY magpie`

display_name = "Samho"
workspace = "."
data_dir = ".samho"
http_bind = "0.0.0.0:7432"
max_turns = 40
request_timeout_ms = 600000
streaming = true

[tools]
shell_timeout_ms = 120000

[retry]
max_attempts = 3
initial_backoff_ms = 500
max_backoff_ms = 8000

[budget]
max_task_tokens = 4000000

[[models]]
id = "deepseek-v4.1-flash"    # 与内置默认模型绑定一致，全部任务类型自动路由到此
model = "wuyou/deepseek-v4.1-flash"
provider = "chat_completions" # POST {base_url}/chat/completions，流式 SSE + reasoning_content
tier = "reasoning"
base_url = "http://127.0.0.1:3425/v1"
api_key_env = "SAMHO_API_KEY"
context_window = 272000
max_output_tokens = 32768     # 思考 token 计入输出，必须给足
supports_tools = true
supports_vision = true
supports_reasoning = true
streaming = true
output_token_parameter = "max_tokens"
input_usd_per_million = 0.0
output_usd_per_million = 0.0
latency_ms = 3000
"#;

#[derive(Args)]
struct RunArgs {
    prompt: String,
    #[arg(long)]
    session: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=10))]
    complexity: Option<u8>,
    #[arg(long)]
    tier: Option<String>,
    /// 配置中的自定义智能层级 ID，独立于内置兼容层级。
    #[arg(long)]
    intelligence_tier: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    skill: Vec<String>,
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    if let Command::Tui { url } = &cli.command {
        return tui::run(url).await;
    }
    let config = RuntimeConfig::load(cli.config.as_deref())?;
    if let Command::Config {
        command: ConfigCommand::Init { path },
    } = &cli.command
    {
        anyhow::ensure!(!path.exists(), "配置文件已存在：{}", path.display());
        // 2026-11-12 起 config init 默认生成本机 OpenAI 兼容网关模板（deepseek-v4.1-flash）；
        // 省略的配置节在加载时回退内置默认值（serde default）。
        std::fs::write(path, DEFAULT_CONFIG_TEMPLATE)?;
        println!(
            "已创建 {}。默认接入 OpenAI 兼容网关 http://127.0.0.1:3425/v1（wuyou/deepseek-v4.1-flash）；请先设置环境变量 SAMHO_API_KEY。",
            path.display()
        );
        return Ok(());
    }
    let (runtime, roles) = roles::build_runtime(config).await?;
    let outcome = execute(&runtime, roles, cli.command).await;
    let shutdown = runtime.shutdown().await;
    outcome?;
    shutdown
}

async fn execute(
    runtime: &Arc<AgentRuntime>,
    roles: Arc<roles::ModelRoles>,
    command: Command,
) -> Result<()> {
    match command {
        Command::Run(args) => {
            let tier = args
                .tier
                .map(|value| serde_json::from_value(serde_json::Value::String(value)))
                .transpose()
                .context(
                    "未知模型层级；可用 nano、fast、balanced、reasoning、frontier 或 specialist",
                )?;
            let request = TaskRequest {
                session_id: args.session,
                prompt: args.prompt,
                profile: TaskProfile {
                    complexity: args.complexity.unwrap_or(4),
                    automatic: args.complexity.is_none(),
                    tier,
                    intelligence_tier: args.intelligence_tier,
                    model: args.model,
                    ..TaskProfile::default()
                },
                skills: args.skill,
            };
            let cancel = CancellationToken::new();
            let cancel_on_signal = cancel.clone();
            let signal = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    cancel_on_signal.cancel();
                }
            });
            let result = runtime.run(request, cancel).await;
            signal.abort();
            let result = result?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&result)?);
            } else {
                println!("{}", result.output);
                eprintln!(
                    "状态={} 会话={} 轨迹={} 模型={}",
                    result.status, result.session_id, result.trace_id, result.model_id
                );
            }
        }
        Command::Serve { bind } => {
            let address = bind.unwrap_or_else(|| runtime.config.http_bind.clone());
            server::serve(runtime.clone(), roles, &address).await?;
        }
        Command::Fork {
            session,
            cutoff,
            title,
        } => {
            let parent = runtime.session(&session).await?.context("找不到父会话")?;
            let fork = runtime
                .fork(&session, cutoff.unwrap_or(parent.messages.len()), title)
                .await?;
            println!("{}", serde_json::to_string_pretty(&fork)?);
        }
        Command::Inspect => println!(
            "{}",
            serde_json::to_string_pretty(&runtime.events.redactor().value(runtime.state().await))?
        ),
        Command::Replay { trace_id, export } => {
            if let Some(path) = export {
                runtime.events.export_jsonl(&path).await?;
                println!("已导出 {}", path.display());
            } else {
                for event in runtime.events.events(trace_id.as_deref()).await? {
                    println!("{}", serde_json::to_string(&event)?);
                }
            }
        }
        Command::Tui { .. } | Command::Config { .. } => unreachable!(),
    }
    Ok(())
}
