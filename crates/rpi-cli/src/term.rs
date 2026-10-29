//! PTY 终端：portable-pty 驱动（Windows=ConPTY），每个工作区独立的真实终端。
//!
//! 输出经 reader 线程 → broadcast + 历史环形缓冲；SSE 订阅时先补发历史再推
//! 实时块。输入走 POST（write+flush 于阻塞线程）。会话上限 + 闲置回收 +
//! serve 关停全杀。

use crate::panels;
use crate::server::{ApiError, AppState};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::response::Sse;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Deserialize;
use serde_json::json;
use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

/// 每工作区终端会话上限。
const MAX_TERMS: usize = 8;
/// 闲置回收阈值。
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// 历史缓冲（字符块数；用于 SSE 晚订阅补发）。
const HISTORY_BLOCKS: usize = 512;

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

pub(crate) struct TermHub {
    sessions: tokio::sync::RwLock<BTreeMap<String, TermSession>>,
    root: PathBuf,
    max: usize,
}

struct TermSession {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    output: broadcast::Sender<String>,
    history: Arc<Mutex<VecDeque<String>>>,
    last_active: Instant,
}

impl TermHub {
    pub(crate) fn new(root: PathBuf) -> Arc<Self> {
        Self::with_max(root, MAX_TERMS)
    }

    pub(crate) fn with_max(root: PathBuf, max: usize) -> Arc<Self> {
        let hub = Arc::new(Self {
            sessions: tokio::sync::RwLock::new(BTreeMap::new()),
            root,
            max,
        });
        // 闲置回收：持有弱引用，服务关停后循环自然退出。
        let weak = Arc::downgrade(&hub);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let Some(hub) = weak.upgrade() else { break };
                hub.reap(IDLE_TIMEOUT).await;
            }
        });
        hub
    }

    pub(crate) async fn open(
        &self,
        cwd: Option<String>,
    ) -> Result<(String, broadcast::Receiver<String>), ApiError> {
        let root = self.root.clone();
        let cwd_path = match cwd.as_deref().filter(|s| !s.trim().is_empty()) {
            None => root.clone(),
            Some(rel) => panels::confined(&root, rel, true).await?,
        };
        if !cwd_path.is_dir() {
            return Err(ApiError(StatusCode::BAD_REQUEST, "起始目录不存在".into()));
        }
        let mut sessions = self.sessions.write().await;
        if sessions.len() >= self.max {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("终端会话已达上限（{MAX_TERMS}），请先关闭空闲会话"),
            ));
        }
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| {
                ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("PTY 打开失败：{e}"),
                )
            })?;
        let mut command = shell_command();
        command.cwd(&cwd_path);
        let child = pair.slave.spawn_command(command).map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("shell 启动失败：{e}"),
            )
        })?;
        let reader = pair.master.try_clone_reader().map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("PTY 读取器获取失败：{e}"),
            )
        })?;
        let writer = pair.master.take_writer().map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("PTY 写入器获取失败：{e}"),
            )
        })?;
        let (tx, rx) = broadcast::channel::<String>(1024);
        let tx_reader = tx.clone();
        let history = Arc::new(Mutex::new(VecDeque::new()));
        // reader 线程：持续抽干 PTY 输出（无订阅者也不能停，否则 ConPTY 管道会堵）。
        let history_for_thread = history.clone();
        std::thread::spawn(move || {
            let mut reader = reader;
            let tx = tx_reader;
            let mut buffer = [0u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buffer[..n]).into_owned();
                        if let Ok(mut deque) = history_for_thread.lock() {
                            deque.push_back(text.clone());
                            let mut excess = deque.len().saturating_sub(HISTORY_BLOCKS);
                            while excess > 0 {
                                deque.pop_front();
                                excess -= 1;
                            }
                        }
                        let _ = tx.send(text);
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send("\r\n\x1b[2m[进程已退出]\x1b[0m\r\n".to_string());
        });
        let id = uuid::Uuid::new_v4().to_string();
        let killer = child.clone_killer();
        sessions.insert(
            id.clone(),
            TermSession {
                writer: Arc::new(Mutex::new(writer)),
                master: Mutex::new(pair.master),
                killer,
                output: tx,
                history,
                last_active: Instant::now(),
            },
        );
        drop(child);
        Ok((id, rx))
    }

    pub(crate) async fn subscribe(
        &self,
        id: &str,
    ) -> Option<(broadcast::Receiver<String>, String)> {
        let sessions = self.sessions.read().await;
        let session = sessions.get(id)?;
        let backfill = session
            .history
            .lock()
            .map(|deque| deque.iter().cloned().collect::<String>())
            .unwrap_or_default();
        Some((session.output.subscribe(), backfill))
    }

    pub(crate) async fn write(&self, id: &str, text: &str) -> Result<(), ApiError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "终端会话不存在".into()))?;
        session.last_active = Instant::now();
        let writer = session.writer.clone();
        let payload = text.to_string();
        tokio::task::spawn_blocking(move || {
            let mut guard = writer
                .lock()
                .map_err(|_| std::io::Error::other("writer poisoned"))?;
            guard.write_all(payload.as_bytes())?;
            guard.flush()
        })
        .await
        .map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("写入任务失败：{e}"),
            )
        })?
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("写入失败：{e}")))?;
        Ok(())
    }

    pub(crate) async fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), ApiError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(id)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "终端会话不存在".into()))?;
        session
            .master
            .lock()
            .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "PTY 状态损坏".into()))?
            .resize(PtySize {
                rows: rows.max(2),
                cols: cols.max(2),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("调整尺寸失败：{e}")))?;
        Ok(())
    }

    pub(crate) async fn close(&self, id: &str) -> Result<(), ApiError> {
        let session = self.sessions.write().await.remove(id);
        let Some(mut session) = session else {
            return Err(ApiError(StatusCode::NOT_FOUND, "终端会话不存在".into()));
        };
        let _ = session.killer.kill();
        // master 落 drop 关闭 PTY → reader 线程读到 EOF 自然退出。
        Ok(())
    }

    pub(crate) async fn reap(&self, idle: Duration) {
        let stale: Vec<String> = self
            .sessions
            .read()
            .await
            .iter()
            .filter(|(_, s)| s.last_active.elapsed() > idle)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            tracing::info!(%id, "回收闲置终端会话");
            let _ = self.close(&id).await;
        }
    }

    pub(crate) async fn shutdown_all(&self) {
        let ids: Vec<String> = self.sessions.read().await.keys().cloned().collect();
        for id in ids {
            let _ = self.close(&id).await;
        }
    }
}

fn shell_command() -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut command = CommandBuilder::new("powershell.exe");
        command.args(["-NoProfile", "-NoLogo"]);
        command.env("TERM", "xterm-256color");
        command
    }
    #[cfg(not(windows))]
    {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut command = CommandBuilder::new(program);
        command.env("TERM", "xterm-256color");
        command
    }
}

// ---------- HTTP handlers ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TermOpen {
    #[serde(default)]
    cwd: Option<String>,
}

pub(crate) async fn term_open(
    State(app): State<AppState>,
    Json(body): Json<TermOpen>,
) -> ApiResult<serde_json::Value> {
    let (id, _) = app.terms.open(body.cwd).await?;
    Ok(Json(json!({ "id": id })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TermInput {
    text: String,
}

pub(crate) async fn term_input(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TermInput>,
) -> ApiResult<serde_json::Value> {
    app.terms.write(&id, &body.text).await?;
    Ok(Json(json!({ "written": body.text.chars().count() })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TermResize {
    cols: u16,
    rows: u16,
}

pub(crate) async fn term_resize(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<TermResize>,
) -> ApiResult<serde_json::Value> {
    app.terms.resize(&id, body.cols, body.rows).await?;
    Ok(Json(json!({ "cols": body.cols, "rows": body.rows })))
}

pub(crate) async fn term_close(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    app.terms.close(&id).await?;
    Ok(Json(json!({ "closed": id })))
}

/// SSE：先补发历史（backfill 事件），再推实时 data 事件。
/// Event.data 内的换行由 axum 拆成多行 data:，EventSource 会原样重组。
pub(crate) async fn term_stream(State(app): State<AppState>, Path(id): Path<String>) -> Response {
    let Some((rx, backfill)) = app.terms.subscribe(&id).await else {
        return ApiError(StatusCode::NOT_FOUND, "终端会话不存在".into()).into_response();
    };
    let shutdown = app.shutdown.clone();
    let stream = futures_util::stream::unfold(
        (Some(backfill), rx, shutdown),
        |(mut backfill, mut rx, shutdown)| async move {
            if let Some(text) = backfill.take() {
                let event: Result<axum::response::sse::Event, std::convert::Infallible> =
                    Ok(axum::response::sse::Event::default()
                        .event("backfill")
                        .data(text));
                return Some((event, (backfill, rx, shutdown)));
            }
            let item = tokio::select! {
                _ = shutdown.cancelled() => return None,
                item = rx.recv() => item,
            };
            let event: Result<axum::response::sse::Event, std::convert::Infallible> = match item {
                Ok(text) => Ok(axum::response::sse::Event::default()
                    .event("data")
                    .data(text)),
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    Ok(axum::response::sse::Event::default().event("gap").data(
                        json!({"missed": missed, "recovery": "输出有丢失，已继续"}).to_string(),
                    ))
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((event, (backfill, rx, shutdown)))
        },
    );
    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    async fn next_with_timeout(rx: &mut broadcast::Receiver<String>, marker: &str) -> bool {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(text)) if text.contains(marker) => return true,
                Ok(Ok(_)) => continue,
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Err(broadcast::error::RecvError::Closed)) => return false,
                Err(_) => return false,
            }
        }
    }

    #[tokio::test]
    async fn term_cap_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let hub = TermHub::with_max(dir.path().to_path_buf(), 1);
        let first = hub.open(None).await;
        assert!(first.is_ok(), "第一个会话应可创建");
        let second = hub.open(None).await;
        assert!(second.is_err(), "超出上限应被拒绝");
        let (id, _) = first.unwrap();
        hub.close(&id).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn term_echoes_command_output() {
        let dir = tempfile::tempdir().unwrap();
        let hub = TermHub::with_max(dir.path().to_path_buf(), MAX_TERMS);
        let (id, mut rx) = hub.open(None).await.expect("打开终端");
        // 等 shell 提示符出现再发命令，避免输出交叠影响断言。
        let _ = next_with_timeout(&mut rx, "\n").await;
        hub.write(&id, "echo rpi-term-marker-7f3a\r").await.unwrap();
        assert!(
            next_with_timeout(&mut rx, "rpi-term-marker-7f3a").await,
            "应在超时前回显命令输出"
        );
        hub.close(&id).await.unwrap();
        assert!(hub.open(None).await.is_ok(), "关闭后应可再次创建");
    }

    #[tokio::test]
    async fn term_resize_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let hub = TermHub::with_max(dir.path().to_path_buf(), MAX_TERMS);
        let (id, _) = hub.open(None).await.expect("打开终端");
        hub.resize(&id, 120, 40).await.unwrap();
        hub.close(&id).await.unwrap();
    }
}
