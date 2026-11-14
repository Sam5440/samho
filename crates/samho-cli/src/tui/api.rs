//! TUI 的 HTTP/SSE 客户端封装：普通请求 + 两条常驻 SSE 流
//! （`/api/live/stream` 任务进度、`/api/events/stream` 轨迹事件），
//! 断线按 1s→8s 指数退避重连。

use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc;

use super::Update;
use super::sse::SseParser;

/// 常规 JSON GET。
pub async fn fetch(client: &reqwest::Client, url: &str, path: &str) -> Result<Value> {
    let response = client.get(format!("{url}{path}")).send().await?;
    let status = response.status();
    let value: Value = response.json().await.context("服务返回了无效 API 响应")?;
    anyhow::ensure!(
        status.is_success(),
        "{}",
        value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("API 请求失败")
    );
    Ok(value)
}

/// SSE 专用客户端：不设总超时（流是长连接），只限制连接建立。
pub fn sse_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

/// `/api/live/stream`：模型流式进度（TaskProgress 累计快照整体替换）。
pub fn spawn_live_sse(
    client: reqwest::Client,
    url: String,
    sender: mpsc::Sender<Update>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            let connected = consume_live(&client, &url, &sender).await.is_ok();
            let _ = sender
                .send(Update::Sse {
                    live: Some(connected),
                    trace: None,
                })
                .await;
            let _ = sender
                .send(Update::Notice(if connected {
                    "进度流已连接（SSE）".into()
                } else {
                    format!("进度流中断，{backoff}s 后重连")
                }))
                .await;
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(8);
        }
    })
}

async fn consume_live(
    client: &reqwest::Client,
    url: &str,
    sender: &mpsc::Sender<Update>,
) -> Result<()> {
    let response = client
        .get(format!("{url}/api/live/stream"))
        .header("accept", "text/event-stream")
        .send()
        .await?;
    anyhow::ensure!(response.status().is_success(), "SSE {}", response.status());
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("SSE 流中断")?;
        for message in parser.feed(&bytes) {
            match message.event.as_str() {
                "progress" => {
                    let value: Value = serde_json::from_str(&message.data)
                        .map_err(|error| anyhow!("进度帧解析失败：{error}"))?;
                    sender.send(Update::Progress(value)).await?;
                }
                "gap" => {
                    let _ = sender
                        .send(Update::Notice("进度流检测到断帧，正在用轮询兜底".into()))
                        .await;
                }
                _ => {}
            }
        }
    }
    anyhow::bail!("SSE 连接被服务端关闭")
}

/// `/api/events/stream`：轨迹事件（TraceEvent 全量 JSON），`after` 游标断点续传。
pub fn spawn_trace_sse(
    client: reqwest::Client,
    url: String,
    sender: mpsc::Sender<Update>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        let mut after: u64 = 0;
        loop {
            let result = consume_trace(&client, &url, &mut after, &sender).await;
            let connected = result.is_ok();
            let _ = sender
                .send(Update::Sse {
                    live: None,
                    trace: Some(connected),
                })
                .await;
            let _ = sender
                .send(Update::Notice(if connected {
                    "轨迹流已连接（SSE）".into()
                } else {
                    format!("轨迹流中断，{backoff}s 后重连")
                }))
                .await;
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(8);
        }
    })
}

async fn consume_trace(
    client: &reqwest::Client,
    url: &str,
    after: &mut u64,
    sender: &mpsc::Sender<Update>,
) -> Result<()> {
    let response = client
        .get(format!("{url}/api/events/stream?after={after}"))
        .header("accept", "text/event-stream")
        .send()
        .await?;
    anyhow::ensure!(response.status().is_success(), "SSE {}", response.status());
    let mut stream = response.bytes_stream();
    let mut parser = SseParser::default();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.context("SSE 流中断")?;
        for message in parser.feed(&bytes) {
            match message.event.as_str() {
                "trace" => {
                    let value: Value = serde_json::from_str(&message.data)
                        .map_err(|error| anyhow!("轨迹帧解析失败：{error}"))?;
                    if let Some(sequence) = value["sequence"].as_u64() {
                        *after = (*after).max(sequence);
                    }
                    sender.send(Update::TraceEvent(value)).await?;
                }
                "gap" => {
                    let _ = sender
                        .send(Update::Notice("轨迹流检测到断帧或索引回收".into()))
                        .await;
                }
                _ => {}
            }
        }
    }
    anyhow::bail!("SSE 连接被服务端关闭")
}
