use std::{collections::BTreeMap, convert::Infallible, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{
        Html, IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::{get, post},
};
use futures_util::{FutureExt, stream};
use rpi_core::{AgentRuntime, TaskRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    runtime: Arc<AgentRuntime>,
    tasks: Arc<RwLock<BTreeMap<String, TaskEntry>>>,
    shutdown: CancellationToken,
    ui: Arc<RwLock<rpi_core::config::UiConfig>>,
}

struct TaskEntry {
    cancel: CancellationToken,
    abort: Option<tokio::task::AbortHandle>,
    snapshot: TaskSnapshot,
}

impl AppState {
    fn new(runtime: Arc<AgentRuntime>) -> Self {
        Self {
            ui: Arc::new(RwLock::new(runtime.config.ui.clone())),
            runtime,
            tasks: Arc::new(RwLock::new(BTreeMap::new())),
            shutdown: CancellationToken::new(),
        }
    }

    async fn stop(&self) {
        self.shutdown.cancel();
        for task in self.tasks.read().await.values() {
            task.cancel.cancel();
        }
    }

    async fn settle(&self) {
        let settled = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if self
                    .tasks
                    .read()
                    .await
                    .values()
                    .all(|task| task.snapshot.status != "running")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        if settled.is_err() {
            for task in self.tasks.write().await.values_mut() {
                if task.snapshot.status == "running" {
                    if let Some(abort) = &task.abort {
                        abort.abort();
                    }
                    task.snapshot.status = "cancelled".into();
                    task.snapshot.error = Some("service shutdown interrupted the task".into());
                }
            }
            tracing::warn!("active tasks exceeded the shutdown grace period");
        }
    }

    fn redact(&self, value: Value) -> Value {
        self.runtime.events.redactor().value(value)
    }
}

#[derive(Clone, Serialize)]
struct TaskSnapshot {
    task_id: String,
    status: String,
    result: Option<Value>,
    error: Option<String>,
    progress: Option<rpi_core::TaskProgress>,
    error_details: Option<Value>,
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;
struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, value.to_string())
    }
}

#[cfg(test)]
pub fn router(runtime: Arc<AgentRuntime>) -> Router {
    router_with_state(AppState::new(runtime))
}

fn router_with_state(app: AppState) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../web/index.html")) }),
        )
        .route(
            "/index.html",
            get(|| async { Html(include_str!("../../../web/index.html")) }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/style.css"),
                )
            }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/app.js"),
                )
            }),
        )
        .route(
            "/chinese.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/chinese.css"),
                )
            }),
        )
        .route(
            "/docs",
            get(|| async { Html(include_str!("../../../web/docs.html")) }),
        )
        .route(
            "/docs.html",
            get(|| async { Html(include_str!("../../../web/docs.html")) }),
        )
        .route(
            "/docs.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/docs.css"),
                )
            }),
        )
        .route(
            "/docs.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/docs.js"),
                )
            }),
        )
        .route(
            "/documents-data.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/documents-data.js"),
                )
            }),
        )
        .route(
            "/plan",
            get(|| async { Html(include_str!("../../../web/plan.html")) }),
        )
        .route(
            "/plan.html",
            get(|| async { Html(include_str!("../../../web/plan.html")) }),
        )
        .route(
            "/plan.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../../../web/plan.css"),
                )
            }),
        )
        .route(
            "/plan.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../../../web/plan.js"),
                )
            }),
        )
        .route(
            "/api/health",
            get(|| async { Json(json!({"status":"ok","api_version":1})) }),
        )
        .route("/api/state", get(state))
        .route("/api/models", get(models))
        .route("/api/objects", get(objects))
        .route("/api/skills", get(skills))
        .route("/api/sessions", get(sessions))
        .route("/api/sessions/{id}", get(session))
        .route("/api/sessions/{id}/fork", post(fork_session))
        .route("/api/run", post(run_task))
        .route("/api/tasks/{id}", get(task))
        .route("/api/tasks/{id}/cancel", post(cancel_task))
        .route("/api/tasks/{id}/steer", post(steer_task))
        .route("/api/tasks/{id}/follow-up", post(followup_task))
        .route("/api/live/stream", get(live_stream))
        .route("/api/events", get(events))
        .route("/api/events/stream", get(event_stream))
        .route("/api/events/page", get(event_page))
        .route("/api/traces/{id}/timeline", get(trace_timeline))
        .route("/api/traces/{id}/export", get(trace_export))
        .route("/api/ui", get(ui).put(update_ui))
        .route("/api/workspaces", get(workspaces))
        .route("/api/skills/refresh", post(refresh_skills))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(local_origin))
        .with_state(app)
}

// The service exposes local execution capabilities. It serves a same-origin UI and does not enable CORS.
async fn local_origin(request: Request, next: Next) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let host = request
            .headers()
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let expected = format!("http://{host}");
        if origin.to_str().ok() != Some(expected.as_str()) {
            return ApiError(
                StatusCode::FORBIDDEN,
                "cross-origin requests are not allowed".into(),
            )
            .into_response();
        }
    }
    if let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
    {
        let local = host == "localhost"
            || host.starts_with("localhost:")
            || host == "127.0.0.1"
            || host.starts_with("127.0.0.1:")
            || host == "[::1]"
            || host.starts_with("[::1]:");
        if !local {
            return ApiError(
                StatusCode::FORBIDDEN,
                "this API accepts localhost hosts only".into(),
            )
            .into_response();
        }
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'".parse().unwrap());
    response
}

pub async fn serve(runtime: Arc<AgentRuntime>, address: &str) -> Result<()> {
    let address: SocketAddr = address.parse()?;
    anyhow::ensure!(
        address.ip().is_loopback(),
        "the local execution API must bind to a loopback address"
    );
    let listener = tokio::net::TcpListener::bind(address).await?;
    println!("RPi workspace: http://{}", listener.local_addr()?);
    println!(
        "TUI client: rpi tui --url http://{}",
        listener.local_addr()?
    );
    let app = AppState::new(runtime);
    let shutdown_state = app.clone();
    let outcome = axum::serve(listener, router_with_state(app.clone()))
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_state.stop().await;
        })
        .await;
    app.stop().await;
    app.settle().await;
    outcome?;
    Ok(())
}

async fn state(State(app): State<AppState>) -> Json<Value> {
    let mut state = app.runtime.state().await;
    state["ui"] = json!(*app.ui.read().await);
    Json(app.redact(state))
}
async fn models(State(app): State<AppState>) -> Json<Value> {
    Json(app.redact(json!(app.runtime.config.models)))
}
async fn objects(State(app): State<AppState>) -> Json<Value> {
    Json(app.redact(json!(app.runtime.objects.list())))
}
async fn skills(State(app): State<AppState>) -> Json<Value> {
    let state = app.runtime.state().await;
    if let Some(skills) = state.get("skills").filter(|v| v.is_array()) {
        return Json(app.redact(skills.clone()));
    }
    let objects = json!(app.runtime.objects.list());
    Json(
        app.redact(Value::Array(
            objects
                .as_array()
                .into_iter()
                .flatten()
                .filter(|object| {
                    object
                        .get("kind")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("skill"))
                })
                .cloned()
                .collect(),
        )),
    )
}
async fn sessions(State(app): State<AppState>) -> ApiResult<Value> {
    let sessions = app.runtime.session_summaries().await;
    Ok(Json(app.redact(json!(sessions))))
}
async fn session(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    match app.runtime.session(&id).await? {
        Some(session) => Ok(Json(app.redact(json!(session)))),
        None => Err(ApiError(StatusCode::NOT_FOUND, "session not found".into())),
    }
}

async fn run_task(
    State(app): State<AppState>,
    Json(request): Json<TaskRequest>,
) -> std::result::Result<(StatusCode, Json<Value>), ApiError> {
    if app.shutdown.is_cancelled() {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "service is shutting down".into(),
        ));
    }
    if request.prompt.trim().is_empty() {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "prompt must not be empty".into(),
        ));
    }
    let task_id = Uuid::new_v4().to_string();
    let cancel = app.shutdown.child_token();
    {
        let mut tasks = app.tasks.write().await;
        if app.shutdown.is_cancelled() {
            return Err(ApiError(
                StatusCode::SERVICE_UNAVAILABLE,
                "service is shutting down".into(),
            ));
        }
        if tasks
            .values()
            .filter(|task| task.snapshot.status == "running")
            .count()
            >= app.runtime.config.max_concurrent_tasks
        {
            return Err(ApiError(
                StatusCode::TOO_MANY_REQUESTS,
                "too many active tasks".into(),
            ));
        }
        // Completed submissions are a bounded in-memory index. Durable results remain in sessions/traces.
        if tasks.len() >= 1024 {
            tasks.retain(|_, task| task.snapshot.status == "running");
        }
        tasks.insert(
            task_id.clone(),
            TaskEntry {
                cancel: cancel.clone(),
                abort: None,
                snapshot: TaskSnapshot {
                    task_id: task_id.clone(),
                    status: "running".into(),
                    result: None,
                    error: None,
                    progress: None,
                    error_details: None,
                },
            },
        );
    }
    let id = task_id.clone();
    let task_state = app.clone();
    let spawned = tokio::spawn(async move {
        let outcome = std::panic::AssertUnwindSafe(task_state.runtime.run_with_task_id(
            id.clone(),
            request,
            cancel.clone(),
        ))
        .catch_unwind()
        .await;
        let error_details = outcome
            .as_ref()
            .ok()
            .and_then(|r| r.as_ref().err())
            .and_then(|e| e.downcast_ref::<rpi_core::provider::ProviderError>())
            .map(|e| task_state.redact(json!(e)));
        let (status, result, error) = match outcome {
            Ok(Ok(result)) => {
                let status = if result.status == "cancelled" {
                    "cancelled"
                } else if result.status == "failed" {
                    "failed"
                } else {
                    "completed"
                };
                (status, Some(task_state.redact(json!(result))), None)
            }
            Ok(Err(error)) => (
                if cancel.is_cancelled() {
                    "cancelled"
                } else {
                    "failed"
                },
                None,
                Some(error.to_string()),
            ),
            Err(_) => ("failed", None, Some("task execution panicked".into())),
        };
        if let Some(task) = task_state.tasks.write().await.get_mut(&id) {
            task.snapshot.status = status.into();
            task.snapshot.result = result;
            task.snapshot.error = error;
            task.snapshot.error_details = error_details;
        }
    });
    if let Some(task) = app.tasks.write().await.get_mut(&task_id) {
        task.abort = Some(spawned.abort_handle());
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"task_id":task_id,"status":"running"})),
    ))
}

async fn task(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<TaskSnapshot> {
    let mut snapshot = app
        .tasks
        .read()
        .await
        .get(&id)
        .map(|task| task.snapshot.clone())
        .ok_or_else(|| {
            ApiError(
                StatusCode::NOT_FOUND,
                "task not found; completed task indexes expire on service restart".into(),
            )
        })?;
    snapshot.progress = app.runtime.progress(&id).await;
    Ok(Json(snapshot))
}
async fn cancel_task(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let tasks = app.tasks.read().await;
    let task = tasks
        .get(&id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "task not found".into()))?;
    if task.snapshot.status == "running" {
        task.cancel.cancel();
    }
    Ok(Json(
        json!({"task_id":id,"status":task.snapshot.status,"cancel_requested":task.cancel.is_cancelled()}),
    ))
}

#[derive(Default, Deserialize)]
struct EventQuery {
    trace_id: Option<String>,
    after: Option<u64>,
    limit: Option<usize>,
    session_id: Option<String>,
    kind: Option<String>,
}

async fn events(State(app): State<AppState>, Query(query): Query<EventQuery>) -> ApiResult<Value> {
    let page = app.runtime.events.query(query.into_query()).await?;
    Ok(Json(json!(page.events)))
}
impl EventQuery {
    fn into_query(self) -> rpi_core::events::EventQuery {
        rpi_core::events::EventQuery {
            after_sequence: self.after.unwrap_or(0),
            limit: self.limit.unwrap_or(100).min(5000),
            trace_id: self.trace_id,
            session_id: self.session_id,
            kind: self.kind,
        }
    }
}
async fn event_page(
    State(app): State<AppState>,
    Query(query): Query<EventQuery>,
) -> ApiResult<Value> {
    Ok(Json(json!(
        app.runtime.events.query(query.into_query()).await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForkRequest {
    cutoff: Option<usize>,
    title: Option<String>,
}
async fn fork_session(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ForkRequest>,
) -> ApiResult<Value> {
    let parent = app
        .runtime
        .session(&id)
        .await?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "会话不存在".into()))?;
    let session = app
        .runtime
        .fork(
            &id,
            body.cutoff.unwrap_or(parent.messages.len()),
            body.title,
        )
        .await
        .map_err(|e| {
            ApiError(
                StatusCode::CONFLICT,
                app.runtime.events.redactor().text(&e.to_string()),
            )
        })?;
    Ok(Json(app.redact(json!(session))))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlRequest {
    prompt: String,
}
async fn steer_task(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ControlRequest>,
) -> ApiResult<Value> {
    queue_control(app, id, body, false).await
}
async fn followup_task(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ControlRequest>,
) -> ApiResult<Value> {
    queue_control(app, id, body, true).await
}
async fn queue_control(
    app: AppState,
    id: String,
    body: ControlRequest,
    followup: bool,
) -> ApiResult<Value> {
    Ok(Json(json!(
        app.runtime
            .control(&id, body.prompt, followup)
            .await
            .map_err(|e| ApiError(
                StatusCode::CONFLICT,
                app.runtime.events.redactor().text(&e.to_string())
            ))?
    )))
}
async fn live_stream(State(app): State<AppState>) -> Response {
    let receiver = app.runtime.subscribe_progress();
    let stream = stream::unfold(
        (receiver, app.shutdown),
        |(mut receiver, shutdown)| async move {
            let event =
                tokio::select! {_=shutdown.cancelled()=>return None,event=receiver.recv()=>event};
            let event = match event {
                Ok(progress) => Event::default()
                    .event("progress")
                    .data(serde_json::to_string(&progress).unwrap_or_default()),
                Err(broadcast::error::RecvError::Lagged(count)) => Event::default()
                    .event("gap")
                    .data(json!({"missed":count,"recovery":"GET /api/tasks/{id}"}).to_string()),
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            Some((Ok::<_, Infallible>(event), (receiver, shutdown)))
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}
async fn trace_timeline(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    Ok(Json(
        json!({"trace_id":id,"mode":"展示式复盘，不会重新执行","spans":app.runtime.events.timeline(&id).await?}),
    ))
}
async fn trace_export(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> std::result::Result<Response, ApiError> {
    let mut report = format!(
        "# RPi 调用轨迹复盘\n\n轨迹：{id}\n\n本报告只展示已经记录的事件，不会重新执行工具或恢复外部副作用。缺失的内容可能由内容开关或日志保留策略造成。\n\n"
    );
    let mut cursor = 0;
    loop {
        let page = app
            .runtime
            .events
            .query(rpi_core::events::EventQuery {
                trace_id: Some(id.clone()),
                after_sequence: cursor,
                limit: 500,
                ..Default::default()
            })
            .await?;
        for event in &page.events {
            report.push_str(&format!(
                "## {} · {}\n\n时间：{} ms；span：{}；parent：{}\n\n    {}\n\n",
                event.sequence,
                event.kind,
                event.timestamp_ms,
                event.span_id,
                event.parent_span_id.as_deref().unwrap_or("无"),
                event.payload
            ));
        }
        if report.len() > 16 * 1024 * 1024 {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "轨迹过大，请用游标分页导出".into(),
            ));
        }
        if !page.has_more {
            break;
        }
        cursor = page.next_cursor.unwrap_or(cursor);
    }
    Ok((
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=RPi-trace.md",
            ),
        ],
        report,
    )
        .into_response())
}
async fn ui(State(app): State<AppState>) -> Json<Value> {
    Json(json!(*app.ui.read().await))
}
async fn update_ui(
    State(app): State<AppState>,
    Json(config): Json<rpi_core::config::UiConfig>,
) -> ApiResult<Value> {
    validate_ui(&config).map_err(|message| ApiError(StatusCode::BAD_REQUEST, message))?;
    let mut ui = app.ui.write().await;
    let path = app.runtime.config.data_dir.join("ui.json");
    let temp = path.with_extension("tmp");
    tokio::fs::write(
        &temp,
        serde_json::to_vec_pretty(&config).map_err(anyhow::Error::from)?,
    )
    .await
    .map_err(anyhow::Error::from)?;
    tokio::fs::rename(temp, path)
        .await
        .map_err(anyhow::Error::from)?;
    *ui = config.clone();
    Ok(Json(json!(config)))
}
fn validate_ui(config: &rpi_core::config::UiConfig) -> std::result::Result<(), String> {
    if !["dark", "light", "system"].contains(&config.theme.as_str()) {
        return Err("theme 必须是 dark、light 或 system".into());
    }
    let mut keys = std::collections::BTreeSet::new();
    for key in [
        &config.submit_key,
        &config.new_session_key,
        &config.cancel_key,
    ] {
        let parts = key.split('+').collect::<Vec<_>>();
        let last = parts.last().copied().unwrap_or("");
        if key.len() > 40
            || !parts[..parts.len() - 1]
                .iter()
                .all(|p| ["Ctrl", "Alt", "Shift"].contains(p))
            || !(last.chars().count() == 1
                || ["Enter", "Tab", "Esc"].contains(&last)
                || last
                    .strip_prefix('F')
                    .and_then(|s| s.parse::<u8>().ok())
                    .is_some_and(|n| (1..=12).contains(&n)))
            || !keys.insert(key.to_lowercase())
        {
            return Err("快捷键无效或重复".into());
        }
    }
    Ok(())
}
async fn workspaces(State(app): State<AppState>) -> Json<Value> {
    let mut entries = vec![
        json!({"id":"default","name":"默认工作区","path":app.runtime.config.workspace,"url":"/"}),
    ];
    entries.extend(
        app.runtime
            .config
            .workspaces
            .iter()
            .map(|w| json!({"id":w.id,"name":w.name,"path":w.path,"url":format!("/w/{}/",w.id)})),
    );
    Json(app.redact(json!(entries)))
}
async fn refresh_skills(State(app): State<AppState>) -> ApiResult<Value> {
    Ok(Json(app.redact(json!(app.runtime.skills.refresh()?))))
}

async fn event_stream(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<EventQuery>,
) -> std::result::Result<Response, ApiError> {
    let receiver = app.runtime.events.subscribe();
    let after = query
        .after
        .or_else(|| {
            headers
                .get("last-event-id")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(0);
    struct CursorState {
        receiver: broadcast::Receiver<rpi_core::TraceEvent>,
        pending: std::collections::VecDeque<rpi_core::TraceEvent>,
        trace: Option<String>,
        last: u64,
        caught_up: bool,
        retention_checked: bool,
        runtime: Arc<AgentRuntime>,
        shutdown: CancellationToken,
    }
    let initial = CursorState {
        receiver,
        pending: Default::default(),
        trace: query.trace_id,
        last: after,
        caught_up: false,
        retention_checked: false,
        runtime: app.runtime,
        shutdown: app.shutdown,
    };
    let events = stream::unfold(initial, |mut state| async move {
        loop {
            if state.shutdown.is_cancelled() {
                return None;
            }
            if let Some(event) = state.pending.pop_front() {
                state.last = event.sequence;
                let event = Event::default()
                    .event("trace")
                    .id(event.sequence.to_string())
                    .data(serde_json::to_string(&event).unwrap_or_default());
                return Some((Ok::<_, Infallible>(event), state));
            }
            if !state.caught_up {
                let page = state
                    .runtime
                    .events
                    .query(rpi_core::events::EventQuery {
                        after_sequence: state.last,
                        limit: 128,
                        trace_id: state.trace.clone(),
                        ..Default::default()
                    })
                    .await;
                match page {
                    Ok(page) => {
                        state.caught_up = !page.has_more;
                        let first = page.first_available_sequence;
                        state.pending.extend(page.events);
                        if !state.retention_checked {
                            state.retention_checked = true;
                            if first.is_some_and(|first| state.last > 0 && state.last + 1 < first) {
                                let event=Event::default().event("gap").data(json!({"after":state.last,"first_available_sequence":first,"reason":"retention"}).to_string());
                                return Some((Ok(event), state));
                            }
                        }
                        continue;
                    }
                    Err(error) => {
                        state.caught_up = true;
                        let event = Event::default()
                            .event("error")
                            .data(state.runtime.events.redactor().text(&error.to_string()));
                        return Some((Ok(event), state));
                    }
                }
            }
            let received = tokio::select! {_=state.shutdown.cancelled()=>return None,event=state.receiver.recv()=>event};
            match received {
                Ok(event) => {
                    if event.sequence <= state.last {
                        continue;
                    }
                    state.last = event.sequence;
                    if state
                        .trace
                        .as_ref()
                        .is_some_and(|trace| trace != &event.trace_id)
                    {
                        continue;
                    }
                    let event = Event::default()
                        .event("trace")
                        .id(event.sequence.to_string())
                        .data(serde_json::to_string(&event).unwrap_or_default());
                    return Some((Ok(event), state));
                }
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    state.caught_up = false;
                    state.retention_checked = false;
                    let event=Event::default().event("gap").data(json!({"missed":count,"after":state.last,"recovery":"indexed catch-up follows"}).to_string());
                    return Some((Ok(event), state));
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Ok(Sse::new(events)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keepalive"),
        )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use futures_util::StreamExt;
    use rpi_core::RuntimeConfig;
    use rpi_core::test_support::bind_loopback;
    use tower::ServiceExt;

    async fn fixture() -> (Router, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        (router(AgentRuntime::new(config).await.unwrap()), dir)
    }

    #[tokio::test]
    async fn health_and_gui_are_served_and_unknown_tasks_are_404() {
        let (app, _dir) = fixture().await;
        for path in [
            "/api/health",
            "/api/models",
            "/api/objects",
            "/api/skills",
            "/api/sessions",
            "/",
        ] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/tasks/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reject_empty_prompt_and_cross_origin_execution() {
        let (app, _dir) = fixture().await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/run")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"prompt":"","profile":{},"skills":[]}"#))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        let request = Request::builder()
            .method("POST")
            .uri("/api/run")
            .header(header::HOST, "127.0.0.1:374")
            .header(header::ORIGIN, "https://example.invalid")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn background_task_completes_with_persisted_session_and_trace() {
        let (app, dir) = fixture().await;
        let request = Request::builder()
            .method("POST")
            .uri("/api/run")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"prompt":"Explain this simulated run","profile":{},"skills":[]}"#,
            ))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let id = body["task_id"].as_str().unwrap();
        let mut result = Value::Null;
        for _ in 0..200 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/tasks/{id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            result =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if result["status"] != "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(result["status"], "completed", "{result}");
        let session_id = result["result"]["session_id"].as_str().unwrap();
        let trace_id = result["result"]["trace_id"].as_str().unwrap();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let session: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(session["id"], session_id);
        assert!(
            session["messages"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message["role"] == "user"
                    && message["content"] == "Explain this simulated run")
        );
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/events?trace_id={trace_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let history: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let history = history.as_array().unwrap();
        for kind in ["task.start", "model.route", "model.response", "task.done"] {
            assert!(
                history.iter().any(|event| event["kind"] == kind),
                "missing {kind}"
            );
        }
        assert!(
            history
                .windows(2)
                .all(|pair| pair[0]["sequence"].as_u64() < pair[1]["sequence"].as_u64())
        );
        assert!(
            tokio::fs::read_to_string(dir.path().join("data/sessions.jsonl"))
                .await
                .unwrap()
                .contains(session_id)
        );
        assert!(
            tokio::fs::read_to_string(dir.path().join("data/events.jsonl"))
                .await
                .unwrap()
                .contains(trace_id)
        );
    }

    #[tokio::test]
    async fn event_history_and_sse_respect_sequence_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        for kind in ["task.start", "task.done"] {
            runtime
                .events
                .append(rpi_core::TraceEvent::new(
                    "trace-test",
                    "session-test",
                    "span-test",
                    kind,
                    json!({}),
                ))
                .await
                .unwrap();
        }
        let app = router(runtime);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/events?after=1&trace_id=trace-test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let history: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(history.as_array().unwrap().len(), 1);
        assert_eq!(history[0]["sequence"], 2);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream")
                    .header("last-event-id", "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut data = response.into_body().into_data_stream();
        let chunk = tokio::time::timeout(Duration::from_secs(2), data.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let text = String::from_utf8(chunk.to_vec()).unwrap();
        assert!(text.contains("event: trace"));
        assert!(text.contains("id: 2"));
        assert!(text.contains("task.done"));
        assert!(!text.contains("task.start"));
    }

    #[tokio::test]
    async fn shutdown_ends_live_stream_and_rejects_new_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let state = AppState::new(AgentRuntime::new(config).await.unwrap());
        let app = router_with_state(state.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/events/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let mut data = response.into_body().into_data_stream();
        state.stop().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(2), data.next())
                .await
                .unwrap()
                .is_none()
        );
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"prompt":"late task"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        state.settle().await;
        state.runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn model_and_object_metadata_redact_literal_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        config.models[0].base_url = Some("https://provider.invalid/sk-abcdefgh12345678".into());
        let app = router(AgentRuntime::new(config).await.unwrap());
        for path in ["/api/state", "/api/models", "/api/objects"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            let text = String::from_utf8(body.to_vec()).unwrap();
            assert!(
                !text.contains("sk-abcdefgh12345678"),
                "credential exposed in {path}"
            );
            assert!(
                text.contains("[REDACTED]"),
                "metadata unexpectedly missing in {path}"
            );
        }
    }

    #[tokio::test]
    async fn cancellation_interrupts_a_pending_http_provider() {
        use rpi_core::model::ProviderKind;
        let listener = bind_loopback().await.unwrap();
        let address = listener.local_addr().unwrap();
        let provider = tokio::spawn(async move {
            let app = Router::new().route("/chat/completions", post(|| async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Json(json!({"choices":[{"message":{"role":"assistant","content":"late"},"finish_reason":"stop"}]}))
            }));
            axum::serve(listener, app).await.unwrap();
        });
        let dir = tempfile::tempdir().unwrap();
        let mut config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            max_concurrent_tasks: 1,
            ..RuntimeConfig::default()
        };
        config.models.truncate(1);
        config.models[0].provider = ProviderKind::ChatCompletions;
        config.models[0].base_url = Some(format!("http://{address}"));
        let app = router(AgentRuntime::new(config).await.unwrap());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"prompt":"Wait for provider","skills":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let dispatched: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let id = dispatched["task_id"].as_str().unwrap();
        let overflow = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"prompt":"second pending task"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(overflow.status(), StatusCode::TOO_MANY_REQUESTS);

        // /api/run is asynchronous; wait until the runtime has admitted the
        // task before sending steering controls.
        for _ in 0..100 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/tasks/{id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let snapshot: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if snapshot["progress"].is_object() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        for (endpoint, prompt, sequence, kind) in [
            ("steer", "执行中的引导", 1_u64, "steering"),
            ("follow-up", "完成后的追问", 2_u64, "follow_up"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(format!("/api/tasks/{id}/{endpoint}"))
                        .header(header::CONTENT_TYPE, "application/json")
                        .body(Body::from(
                            serde_json::to_vec(&json!({"prompt":prompt})).unwrap(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{endpoint}");
            let queued: Value =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            assert_eq!(queued["sequence"], sequence);
            assert_eq!(queued["kind"], kind);
            assert_eq!(queued["status"], "queued");
        }
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/tasks/{id}/cancel"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut settled = Value::Null;
        for _ in 0..200 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("/api/tasks/{id}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            settled =
                serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                    .unwrap();
            if settled["status"] != "running" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        provider.abort();
        assert_eq!(settled["status"], "cancelled", "{settled}");
    }

    #[tokio::test]
    async fn event_page_timeline_and_export_are_indexed_and_chinese() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        for (span, kind) in [
            ("root", "task.start"),
            ("model", "model.response"),
            ("done", "task.done"),
        ] {
            runtime
                .events
                .append(rpi_core::TraceEvent::new(
                    "trace-page",
                    "session-page",
                    span,
                    kind,
                    json!({"中文": "可审计", "secret": "[REDACTED]"}),
                ))
                .await
                .unwrap();
        }
        runtime
            .events
            .append(rpi_core::TraceEvent::new(
                "other-trace",
                "session-page",
                "other",
                "task.start",
                json!({}),
            ))
            .await
            .unwrap();
        let app = router(runtime.clone());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/events/page?trace_id=trace-page&limit=1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let page: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(page["events"].as_array().unwrap().len(), 1);
        assert_eq!(page["events"][0]["trace_id"], "trace-page");
        assert_eq!(page["has_more"], true);
        let next = page["next_cursor"].as_u64().unwrap();
        assert!(next >= page["events"][0]["sequence"].as_u64().unwrap());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/traces/trace-page/timeline")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let timeline: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(timeline["trace_id"], "trace-page");
        assert_eq!(timeline["mode"], "展示式复盘，不会重新执行");
        assert_eq!(timeline["spans"].as_array().unwrap().len(), 3);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/traces/trace-page/export")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/markdown; charset=utf-8"
        );
        assert!(
            response.headers()[header::CONTENT_DISPOSITION]
                .to_str()
                .unwrap()
                .contains("RPi-trace.md")
        );
        let report = String::from_utf8(
            to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(report.contains("RPi 调用轨迹复盘"));
        assert!(report.contains("task.start"));
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn fork_and_ui_update_have_stable_json_contracts() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        let parent = runtime
            .sessions_store
            .create_or_get(None, "系统指令".into())
            .await
            .unwrap();
        let parent_id;
        {
            let mut session = parent.lock().await;
            parent_id = session.id.clone();
            session
                .messages
                .push(json!({"role":"user","content":"父分支资料"}));
            session.title = "父会话".into();
            runtime.sessions_store.commit(&session).await.unwrap();
        }
        let app = router(runtime.clone());
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/sessions/{parent_id}/fork"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"cutoff":2,"title":"中文分支"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let fork: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let fork_id = fork["id"].as_str().unwrap();
        assert_ne!(fork_id, parent_id);
        assert_eq!(fork["parent_id"], parent_id);
        assert_eq!(fork["fork_cutoff"], 2);
        assert_eq!(fork["title"], "中文分支");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/ui")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ui: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(ui["theme"], "dark");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/ui")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"theme":"light","submit_key":"Ctrl+Enter","new_session_key":"Alt+N","cancel_key":"Ctrl+K"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let updated: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(updated["theme"], "light");
        assert!(
            tokio::fs::try_exists(dir.path().join("data/ui.json"))
                .await
                .unwrap()
        );

        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/ui")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        r#"{"theme":"blue","submit_key":"Ctrl+Enter","new_session_key":"Alt+N","cancel_key":"Ctrl+K"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn live_progress_sse_reports_the_submitted_task() {
        let (app, _dir) = fixture().await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/live/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/event-stream"
        );
        let mut stream = response.into_body().into_data_stream();
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/run")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"prompt":"实时进度测试","skills":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let submitted: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
                .unwrap();
        let task_id = submitted["task_id"].as_str().unwrap();
        let mut received = String::new();
        for _ in 0..8 {
            let chunk = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            received.push_str(std::str::from_utf8(&chunk).unwrap());
            if received.contains("event: progress") && received.contains(task_id) {
                break;
            }
        }
        assert!(received.contains("event: progress"), "{received}");
        assert!(received.contains(task_id), "{received}");
    }
}
