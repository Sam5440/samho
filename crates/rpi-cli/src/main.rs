mod server;
mod tui;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use rpi_core::{AgentRuntime, RuntimeConfig, TaskProfile, TaskRequest};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "rpi", version, about = "面向 Object 的 Rust Agent 运行时")]
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
    /// 连接正在运行的 `rpi serve` 服务。
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
        #[arg(default_value = "rpi.toml")]
        path: PathBuf,
    },
}

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
        config.write_toml(path)?;
        println!(
            "已创建 {}。配置真实 Provider 后即可退出模拟模式。",
            path.display()
        );
        return Ok(());
    }
    let runtime = AgentRuntime::new(config).await?;
    let outcome = execute(&runtime, cli.command).await;
    let shutdown = runtime.shutdown().await;
    outcome?;
    shutdown
}

async fn execute(runtime: &Arc<AgentRuntime>, command: Command) -> Result<()> {
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
            server::serve(runtime.clone(), &address).await?;
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
