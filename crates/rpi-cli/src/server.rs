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
    routing::{any, delete, get, patch, post},
};
use futures_util::{FutureExt, stream};
use rpi_core::{AgentRuntime, TaskRequest};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) runtime: Arc<AgentRuntime>,
    tasks: Arc<RwLock<BTreeMap<String, TaskEntry>>>,
    pub(crate) shutdown: CancellationToken,
    ui: Arc<RwLock<rpi_core::config::UiConfig>>,
    /// 动态工作区注册表（仅默认工作区持有；嵌套前缀为 None）。
    pub(crate) hub: Option<Arc<crate::hub::HubState>>,
    /// 每工作区独立 PTY 终端池。
    pub(crate) terms: std::sync::Arc<crate::term::TermHub>,
    /// 任务类型 → 模型覆盖表（与该运行时的 RoleRouter 共享）。
    pub(crate) roles: Arc<crate::roles::ModelRoles>,
}

struct TaskEntry {
    cancel: CancellationToken,
    abort: Option<tokio::task::AbortHandle>,
    snapshot: TaskSnapshot,
}

impl AppState {
    pub(crate) fn new(runtime: Arc<AgentRuntime>) -> Self {
        let terms = crate::term::TermHub::new(runtime.config.workspace.clone());
        Self {
            ui: Arc::new(RwLock::new(runtime.config.ui.clone())),
            runtime,
            tasks: Arc::new(RwLock::new(BTreeMap::new())),
            shutdown: CancellationToken::new(),
            hub: None,
            terms,
            roles: crate::roles::ModelRoles::detached(),
        }
    }

    /// 生产装配：角色表与运行时内置的 RoleRouter 共享同一份 Arc。
    pub(crate) fn with_roles(
        runtime: Arc<AgentRuntime>,
        roles: Arc<crate::roles::ModelRoles>,
    ) -> Self {
        let mut state = Self::new(runtime);
        state.roles = roles;
        state
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
#[derive(Debug)]
pub(crate) struct ApiError(pub(crate) StatusCode, pub(crate) String);

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

// 内嵌静态资源白名单：UI 重构后前端拆分为 ES Modules + 组件化 CSS。
const STATIC_ASSETS: &[(&str, &str, &str)] = &[
    (
        "/index.html",
        "text/html; charset=utf-8",
        include_str!("../../../web/index.html"),
    ),
    (
        "/ui/tokens.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/ui/tokens.css"),
    ),
    (
        "/ui/base.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/ui/base.css"),
    ),
    (
        "/ui/components.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/ui/components.css"),
    ),
    (
        "/ui/cult.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/ui/cult.css"),
    ),
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/app.css"),
    ),
    (
        "/js/main.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/main.js"),
    ),
    (
        "/js/api.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/api.js"),
    ),
    (
        "/js/store.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/store.js"),
    ),
    (
        "/js/ui.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/ui.js"),
    ),
    (
        "/js/markdown.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/markdown.js"),
    ),
    (
        "/js/sessions.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/sessions.js"),
    ),
    (
        "/js/sidebar.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/sidebar.js"),
    ),
    (
        "/js/panels/files.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/files.js"),
    ),
    (
        "/js/panels/git.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/git.js"),
    ),
    (
        "/js/panels/models.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/models.js"),
    ),
    (
        "/js/panels/agents.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/agents.js"),
    ),
    (
        "/js/panels/tasks.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/tasks.js"),
    ),
    (
        "/js/panels/browser.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/browser.js"),
    ),
    (
        "/js/projects.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/projects.js"),
    ),
    (
        "/js/composer.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/composer.js"),
    ),
    (
        "/js/model-select.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/model-select.js"),
    ),
    (
        "/js/timeline/projection.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/timeline/projection.js"),
    ),
    (
        "/js/timeline/render.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/timeline/render.js"),
    ),
    (
        "/js/timeline/tools.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/timeline/tools.js"),
    ),
    (
        "/js/trajectory/render.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/trajectory/render.js"),
    ),
    (
        "/js/trajectory/expansion.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/trajectory/expansion.js"),
    ),
    (
        "/js/motion.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/motion.js"),
    ),
    (
        "/js/cult/gridbeam.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/cult/gridbeam.js"),
    ),
    (
        "/vendor/xterm.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/vendor/xterm.js"),
    ),
    (
        "/vendor/xterm-addon-canvas.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/vendor/xterm-addon-canvas.js"),
    ),
    (
        "/vendor/xterm.css",
        "text/css; charset=utf-8",
        include_str!("../../../web/vendor/xterm.css"),
    ),
    (
        "/js/workspaces.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/workspaces.js"),
    ),
    (
        "/js/dock.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/dock.js"),
    ),
    (
        "/js/panels/terminal.js",
        "text/javascript; charset=utf-8",
        include_str!("../../../web/js/panels/terminal.js"),
    ),
];

pub(crate) fn router_with_state(app: AppState) -> Router {
    let mut router = Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../../../web/index.html")) }),
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
        .route(
            "/api/sessions/{id}",
            get(session).patch(patch_session).delete(delete_session),
        )
        .route("/api/sessions/{id}/fork", post(fork_session))
        .route("/api/projects", get(list_projects).post(create_project))
        .route(
            "/api/projects/{id}",
            get(get_project).patch(patch_project).delete(delete_project),
        )
        .route("/api/run", post(run_task))
        .route("/api/tasks", get(tasks_list))
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
        .route(
            "/api/workspaces",
            get(workspaces).post(crate::hub::workspace_create),
        )
        .route(
            "/api/workspaces/{id}",
            patch(crate::hub::workspace_rename).delete(crate::hub::workspace_remove),
        )
        .route(
            "/api/model-roles",
            get(model_roles_get).put(model_roles_put),
        )
        .route("/api/browse", get(crate::panels::browse_list))
        .route("/api/browse/mkdir", post(crate::panels::browse_mkdir))
        .route("/w/{id}", any(crate::hub::workspace_dispatch))
        .route("/w/{id}/{*rest}", any(crate::hub::workspace_dispatch))
        .route("/api/term", post(crate::term::term_open))
        .route("/api/term/{id}/stream", get(crate::term::term_stream))
        .route("/api/term/{id}/input", post(crate::term::term_input))
        .route("/api/term/{id}/resize", post(crate::term::term_resize))
        .route("/api/term/{id}", delete(crate::term::term_close))
        .route("/api/skills/refresh", post(refresh_skills))
        .route(
            "/api/fs",
            get(crate::panels::fs_list).delete(crate::panels::fs_delete),
        )
        .route(
            "/api/file",
            get(crate::panels::file_read).put(crate::panels::file_write),
        )
        .route("/api/file/raw", get(crate::panels::file_raw))
        .route("/api/fs/mkdir", post(crate::panels::fs_mkdir))
        .route("/api/fs/rename", post(crate::panels::fs_rename))
        .route("/api/git/status", get(crate::panels::git_status))
        .route("/api/git/diff", get(crate::panels::git_diff))
        .route("/api/git/log", get(crate::panels::git_log))
        .route("/api/git/branches", get(crate::panels::git_branches))
        .route("/api/git/stage", post(crate::panels::git_stage))
        .route("/api/git/unstage", post(crate::panels::git_unstage))
        .route("/api/git/commit", post(crate::panels::git_commit))
        .route("/api/git/checkout", post(crate::panels::git_checkout))
        .route("/api/git/discard", post(crate::panels::git_discard))
        .route("/api/proxy", get(crate::panels::proxy));
    for (path, mime, body) in STATIC_ASSETS {
        let mime = *mime;
        let body = *body;
        router = router.route(
            path,
            get(move || async move { ([(header::CONTENT_TYPE, mime)], body) }),
        );
    }
    router
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
    // 内嵌前端随二进制发布：禁止缓存，升级后旧模块不得残留（回环本地服务无带宽顾虑）。
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    // /api/proxy 自带面向远端页面的专用 CSP（禁脚本、放行样式/图片），不得覆盖。
    if !response
        .headers()
        .contains_key(header::CONTENT_SECURITY_POLICY)
    {
        response.headers_mut().insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; connect-src 'self'; img-src 'self' data: http: https:; style-src 'self'; script-src 'self'; frame-src http: https:; frame-ancestors 'none'; base-uri 'none'".parse().unwrap());
    }
    response
}

// 附加工作区各自持有独立运行时（独立 data_dir/会话/预算），挂在 /w/{id} 前缀下；
// 工作台前端按 location.pathname 推导同前缀 API 路径。
// axum 0.8 的 nest 匹配 /w/{id} 与子路径，但不匹配 /w/{id}/（尾斜杠）；
// 路由匹配先于中间件执行，无法用层重写 URI，因此在 fallback 里对尾斜杠形态 308 重定向。
pub(crate) async fn workspace_fallback(uri: axum::http::Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/w/") && path.ends_with('/') && path.len() > 4 {
        let trimmed = path.trim_end_matches('/');
        let location = match uri.query() {
            Some(query) => format!("{trimmed}?{query}"),
            None => trimmed.to_string(),
        };
        return (
            StatusCode::PERMANENT_REDIRECT,
            [(header::LOCATION, location)],
        )
            .into_response();
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn attach_workspaces(
    runtime: Arc<AgentRuntime>,
    router: Router,
) -> Result<(Router, Vec<Arc<AgentRuntime>>)> {
    let mut router = router;
    let mut extra_runtimes = Vec::new();
    for workspace in &runtime.config.workspaces {
        let mut workspace_config = runtime.config.clone();
        workspace_config.workspace = workspace.path.clone();
        workspace_config.data_dir = workspace.data_dir.clone();
        let (workspace_runtime, workspace_roles) =
            crate::roles::build_runtime(workspace_config).await?;
        router = router.nest(
            &format!("/w/{}", workspace.id),
            router_with_state(AppState::with_roles(
                workspace_runtime.clone(),
                workspace_roles,
            )),
        );
        extra_runtimes.push(workspace_runtime);
    }
    let router = router.fallback(workspace_fallback);
    Ok((router, extra_runtimes))
}

pub async fn serve(
    runtime: Arc<AgentRuntime>,
    roles: Arc<crate::roles::ModelRoles>,
    address: &str,
) -> Result<()> {
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
    let app = AppState::with_roles(runtime.clone(), roles);
    // 动态工作区注册表：恢复上次保存的条目并挂到默认 AppState 上。
    let hub = crate::hub::HubState::new(runtime.config.data_dir.join("workspaces.json"));
    hub.load_saved(runtime.config.clone()).await;
    let mut app_state = app.clone();
    app_state.hub = Some(Arc::new(hub.clone()));
    let default_terms = app_state.terms.clone();
    let (router, extra_runtimes) =
        attach_workspaces(runtime, router_with_state(app_state.clone())).await?;
    let shutdown_state = app.clone();
    let outcome = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_state.stop().await;
        })
        .await;
    app.stop().await;
    app.settle().await;
    for workspace_runtime in extra_runtimes {
        workspace_runtime.shutdown().await?;
    }
    hub.shutdown_all().await;
    default_terms.shutdown_all().await;
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

// 双层 Option 的区分反序列化：字段缺省 → None（保持不变），显式 null → Some(None)（清除）。
fn deserialize_some<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: serde::Deserializer<'de>,
{
    Deserialize::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionPatch {
    title: Option<String>,
    archived: Option<bool>,
    // 缺省=保持不变；null=移出项目；字符串=归属指定项目。
    #[serde(default, deserialize_with = "deserialize_some")]
    project_id: Option<Option<String>>,
}

async fn patch_session(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SessionPatch>,
) -> ApiResult<Value> {
    if app.runtime.session(&id).await?.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "会话不存在".into()));
    }
    if let Some(project_id) = &body.project_id
        && let Some(pid) = project_id
        && app.runtime.projects.get(pid).await.is_none()
    {
        return Err(ApiError(StatusCode::BAD_REQUEST, "项目不存在".into()));
    }
    let mut updated = None;
    if let Some(title) = &body.title {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 200 {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "title 不能为空且不超过 200 字符".into(),
            ));
        }
        updated = Some(
            app.runtime
                .sessions_store
                .rename(&id, title.to_string())
                .await
                .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?,
        );
    }
    if let Some(archived) = body.archived {
        updated = Some(
            app.runtime
                .sessions_store
                .set_archived(&id, archived)
                .await
                .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?,
        );
    }
    if let Some(project_id) = body.project_id {
        updated = Some(
            app.runtime
                .sessions_store
                .set_project(&id, project_id)
                .await
                .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?,
        );
    }
    match updated {
        Some(session) => Ok(Json(app.redact(json!(session)))),
        None => Err(ApiError(
            StatusCode::BAD_REQUEST,
            "请求体至少包含 title、archived 或 project_id".into(),
        )),
    }
}

async fn delete_session(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    if app.runtime.session(&id).await?.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "会话不存在".into()));
    }
    let removed = app
        .runtime
        .sessions_store
        .delete_cascade(&id)
        .await
        .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
    Ok(Json(json!({"deleted": removed})))
}

// ---------- 项目分类管理 ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectCreate {
    name: String,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectPatch {
    name: Option<String>,
    // 缺省=保持不变；null=清除颜色；字符串=设置颜色。
    #[serde(default, deserialize_with = "deserialize_some")]
    color: Option<Option<String>>,
}

fn invalid_input(error: &anyhow::Error) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, error.to_string())
}

async fn list_projects(State(app): State<AppState>) -> ApiResult<Value> {
    Ok(Json(app.redact(json!(app.runtime.projects.list().await))))
}

async fn get_project(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    match app.runtime.projects.get(&id).await {
        Some(project) => Ok(Json(app.redact(json!(project)))),
        None => Err(ApiError(StatusCode::NOT_FOUND, "项目不存在".into())),
    }
}

async fn create_project(
    State(app): State<AppState>,
    Json(body): Json<ProjectCreate>,
) -> std::result::Result<(StatusCode, Json<Value>), ApiError> {
    let name = rpi_core::projects::validate_name(&body.name).map_err(|e| invalid_input(&e))?;
    let color = rpi_core::projects::validate_color(body.color).map_err(|e| invalid_input(&e))?;
    let project = app
        .runtime
        .projects
        .create(name, color)
        .await
        .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
    Ok((StatusCode::CREATED, Json(app.redact(json!(project)))))
}

async fn patch_project(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ProjectPatch>,
) -> ApiResult<Value> {
    if app.runtime.projects.get(&id).await.is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, "项目不存在".into()));
    }
    if let Some(name) = &body.name {
        rpi_core::projects::validate_name(name).map_err(|e| invalid_input(&e))?;
    }
    if let Some(color) = &body.color {
        rpi_core::projects::validate_color(color.clone()).map_err(|e| invalid_input(&e))?;
    }
    let project = app
        .runtime
        .projects
        .update(&id, body.name, body.color)
        .await
        .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
    Ok(Json(app.redact(json!(project))))
}

async fn delete_project(State(app): State<AppState>, Path(id): Path<String>) -> ApiResult<Value> {
    let project = app
        .runtime
        .projects
        .delete(&id)
        .await
        .map_err(|e| ApiError(StatusCode::NOT_FOUND, e.to_string()))?;
    // 会话本身保留，仅回到未分类。
    let mut cleared = 0u64;
    for summary in app.runtime.session_summaries().await {
        if summary.project_id.as_deref() == Some(id.as_str()) {
            app.runtime
                .sessions_store
                .set_project(&summary.id, None)
                .await
                .map_err(|e| ApiError(StatusCode::CONFLICT, e.to_string()))?;
            cleared += 1;
        }
    }
    Ok(Json(
        json!({"deleted": {"id": project.id, "name": project.name}, "sessions_cleared": cleared}),
    ))
}

/// GET /api/model-roles —— 任务类型（路由层级）→ 模型覆盖表。
async fn model_roles_get(State(app): State<AppState>) -> Json<Value> {
    let config = &app.runtime.config;
    let tiers: Vec<Value> = config
        .routing
        .tiers
        .iter()
        .map(|tier| {
            json!({
                "id": tier.id,
                "rank": tier.rank,
                "min_complexity": tier.min_complexity,
                "max_complexity": tier.max_complexity,
                "requires_reasoning": tier.requires_reasoning,
            })
        })
        .collect();
    let models: Vec<Value> = config
        .models
        .iter()
        .map(|model| json!({ "id": model.id, "tier": model.tier_name() }))
        .collect();
    Json(json!({
        "roles": app.roles.snapshot(),
        "default_model": app.roles.default_model(),
        "tiers": tiers,
        "models": models,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelRolesBody {
    roles: BTreeMap<String, String>,
}

/// PUT /api/model-roles —— 覆盖任务类型→模型分配并持久化。
/// 模型 id 为空串表示该类型跟随自动路由。
async fn model_roles_put(
    State(app): State<AppState>,
    Json(body): Json<ModelRolesBody>,
) -> ApiResult<serde_json::Value> {
    let valid_models: std::collections::BTreeSet<String> = app
        .runtime
        .config
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect();
    for (tier, model) in &body.roles {
        if !app.roles.tier_ids().iter().any(|known| known == tier) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("未知任务类型：{tier}"),
            ));
        }
        if !model.is_empty() && !valid_models.contains(model) {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                format!("未知模型：{model}"),
            ));
        }
    }
    app.roles
        .apply(body.roles.clone())
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?;
    if let Err(error) = app.roles.persist().await {
        return Err(ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存模型分配失败：{error}"),
        ));
    }
    Ok(Json(json!({ "roles": app.roles.snapshot() })))
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

/// GET /api/tasks —— 任务看板数据：全部快照 + 运行中任务附实时进度。
async fn tasks_list(State(app): State<AppState>) -> Json<Value> {
    let tasks = app.tasks.read().await;
    let mut items = Vec::with_capacity(tasks.len());
    for (id, entry) in tasks.iter() {
        let mut snapshot = entry.snapshot.clone();
        if snapshot.status == "running" {
            snapshot.progress = app.runtime.progress(id).await;
        }
        items.push(json!({
            "task_id": snapshot.task_id,
            "status": snapshot.status,
            "error": snapshot.error,
            "result": snapshot.result,
            "progress": snapshot.progress,
        }));
    }
    // 运行中在前，其余按创建序倒排（BTreeMap key 即 UUID，无时间序；快照有限，够看板用）。
    items.reverse();
    Json(json!({ "tasks": items }))
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
        json!({"id":"default","name":"默认工作区","path":app.runtime.config.workspace,"url":"/","managed":false}),
    ];
    entries.extend(
        app.runtime
            .config
            .workspaces
            .iter()
            .map(|w| {
                json!({"id":w.id,"name":w.name,"path":w.path,"url":format!("/w/{}/",w.id),"managed":false})
            }),
    );
    // 根服务附带动态注册表；嵌套前缀（hub=None）保持各自独立视图。
    if let Some(hub) = app.hub.as_ref() {
        for (id, entry) in hub.dynamic.read().await.iter() {
            entries.push(json!({
                "id": id,
                "name": entry.name,
                "path": entry.runtime.config.workspace,
                "url": format!("/w/{id}/"),
                "managed": true,
            }));
        }
    }
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
    use rpi_core::config::WorkspaceConfig;
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
    async fn model_roles_roundtrip_and_validation() {
        // 走生产装配（build_runtime + with_roles），角色表按层级种子化。
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let (runtime, roles) = crate::roles::build_runtime(config).await.unwrap();
        let router = router_with_state(AppState::with_roles(runtime, roles));
        let get = |router: axum::Router| {
            router.clone().oneshot(
                Request::builder()
                    .uri("/api/model-roles")
                    .body(Body::empty())
                    .unwrap(),
            )
        };
        let response = get(router.clone()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert!(
            body["tiers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|t| t["id"] == "nano"),
            "tiers should expose routing tiers: {body}"
        );
        assert_eq!(body["roles"]["nano"], "", "无默认模型时种子为空串");
        // 未知模型被拒绝。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/model-roles")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"roles":{"nano":"no-such-model"}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // 未知任务类型被拒绝。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/model-roles")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"roles":{"nope":""}}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        // 合法分配写入后可读回；空串 = 跟随自动路由。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/model-roles")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"roles":{"nano":"demo-balanced","fast":""}}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["roles"]["nano"], "demo-balanced");
        assert_eq!(body["roles"]["fast"], "");
    }

    #[tokio::test]
    async fn browse_lists_directories_and_mkdir() {
        let (router, dir) = fixture().await;
        std::fs::create_dir_all(dir.path().join("pick-me/nested")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();
        let encoded: String = dir
            .path()
            .to_string_lossy()
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    (byte as char).to_string()
                }
                _ => format!("%{byte:02X}"),
            })
            .collect();
        let url = format!("/api/browse?path={encoded}");
        let response = router
            .clone()
            .oneshot(Request::builder().uri(url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap();
        let names: Vec<&str> = body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"pick-me") && !names.contains(&"file.txt"),
            "只列目录，不列文件：{names:?}"
        );
        // 根视图（Windows 列盘符）全部为 dir。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/browse")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap();
        assert!(
            body["entries"]
                .as_array()
                .unwrap()
                .iter()
                .all(|e| e["kind"] == "dir")
        );
        // mkdir 递归创建。
        let target = dir.path().join("made/here");
        let payload = json!({ "path": target.to_string_lossy() });
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/browse/mkdir")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(target.is_dir());
    }

    async fn hub_fixture() -> (Router, tempfile::TempDir, Arc<crate::hub::HubState>) {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        let hub = Arc::new(crate::hub::HubState::new(
            runtime.config.data_dir.join("workspaces.json"),
        ));
        let mut app_state = AppState::new(runtime.clone());
        app_state.hub = Some(hub.clone());
        let (router, _) = attach_workspaces(runtime, router_with_state(app_state))
            .await
            .unwrap();
        (router, dir, hub)
    }

    #[tokio::test]
    async fn dynamic_workspaces_dispatch_isolate_and_remove() {
        let (router, dir, _hub) = hub_fixture().await;
        std::fs::create_dir_all(dir.path().join("target-ws")).unwrap();
        let payload = json!({ "path": dir.path().join("target-ws").to_string_lossy() });
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        let id = body["id"].as_str().unwrap().to_string();
        assert_ne!(id, "default");
        // 分发命中独立运行时：会话列表为空数组。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/w/{id}/api/sessions"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body_bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "dispatch body: {}",
            String::from_utf8_lossy(&body_bytes)
        );
        let body: Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body, json!([]));
        // 嵌套路由带 Path 提取器的接口经分发必须正常（外层参数不得泄漏进内层）。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/w/{id}/api/sessions/no-such-session"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "nested Path extractor through dispatch must 404, not 500"
        );
        // 合并列表含动态项且 managed=true。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert!(
            body.as_array()
                .unwrap()
                .iter()
                .any(|w| w["id"] == id && w["managed"] == json!(true))
        );
        // 摘除后 404。
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/workspaces/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/w/{id}/api/sessions"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dynamic_workspace_create_rejects_missing_path() {
        let (router, _dir, _hub) = hub_fixture().await;
        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/workspaces")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"path":"Z:/definitely/missing"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn workspace_prefix_serves_isolated_runtime_and_unknown_ids_404() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("aux-ws")).unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            workspaces: vec![WorkspaceConfig {
                id: "aux".into(),
                name: "辅助工作区".into(),
                path: dir.path().join("aux-ws"),
                data_dir: dir.path().join("aux-data"),
            }],
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        let (app, extras) = attach_workspaces(runtime.clone(), router(runtime.clone()))
            .await
            .unwrap();
        assert_eq!(extras.len(), 1);
        for path in ["/w/aux", "/w/aux/api/health", "/w/aux/api/sessions"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }
        // 浏览器默认访问的 /w/aux/ 由 fallback 308 到无尾斜杠形态。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/w/aux/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(
            response.headers().get(header::LOCATION),
            Some(&header::HeaderValue::from_static("/w/aux"))
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/w/ghost/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        // 前缀路由命中独立运行时：aux 的 /api/workspaces 首项指向其自身 workspace。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/w/aux/api/workspaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let workspaces: Vec<Value> = serde_json::from_slice(&body).unwrap();
        // 运行时会把 workspace 解析为 canonical 路径（Windows 带 \\?\ 前缀），只断言目标目录名。
        assert!(workspaces[0]["path"].as_str().unwrap().ends_with("aux-ws"));
        runtime.shutdown().await.unwrap();
        extras[0].shutdown().await.unwrap();
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
    async fn session_patch_delete_and_static_assets_served() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        let handle = runtime
            .sessions_store
            .create_or_get(None, "系统指令".into())
            .await
            .unwrap();
        let session_id;
        {
            let mut s = handle.lock().await;
            session_id = s.id.clone();
            s.messages.push(json!({"role":"user","content":"补丁会话"}));
            runtime.sessions_store.commit(&s).await.unwrap();
        }
        let app = router(runtime.clone());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"title":"新标题"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(patched["title"], "新标题");
        assert_eq!(patched["archived"], false);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"archived":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(patched["archived"], true);
        // 汇总列表同步暴露 archived。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summaries: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(summaries[0]["archived"], true);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/sessions/{session_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let deleted: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(deleted["deleted"][0], session_id);
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
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        // 空补丁与未知会话的错误契约。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // 重构后的静态资源：模块与组件 CSS 可达，旧入口下线。
        for path in [
            "/js/main.js",
            "/js/timeline/render.js",
            "/js/trajectory/render.js",
            "/ui/tokens.css",
            "/ui/components.css",
            "/app.css",
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
                    .uri("/style.css")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn projects_classify_sessions_and_cascade_clear() {
        let dir = tempfile::tempdir().unwrap();
        let config = RuntimeConfig {
            workspace: dir.path().to_path_buf(),
            data_dir: dir.path().join("data"),
            durable_logs: false,
            ..RuntimeConfig::default()
        };
        let runtime = AgentRuntime::new(config).await.unwrap();
        let handle = runtime
            .sessions_store
            .create_or_get(None, "系统指令".into())
            .await
            .unwrap();
        let session_id = handle.lock().await.id.clone();
        let app = router(runtime.clone());

        // 创建项目：校验 + 重复名拒绝。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r##"{"name":"前端重构","color":"#7FAE32"}"##))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let project: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        let project_id = project["id"].as_str().unwrap().to_string();
        assert_eq!(project["color"], "#7fae32");

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"前端重构"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/projects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"name":"","color":"blue"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        // 会话归属项目：摘要同步携带 project_id。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"project_id":"{project_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(patched["project_id"], project_id.as_str());

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let summaries: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 256 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(summaries[0]["project_id"], project_id.as_str());

        // 未知项目拒绝；null 清除归属。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"project_id":"missing"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"project_id":null}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let patched: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(patched["project_id"], Value::Null);

        // 归属回来再删除项目：会话回到未分类。
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/sessions/{session_id}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(r#"{{"project_id":"{project_id}"}}"#)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/api/projects/{project_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let deleted: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(deleted["sessions_cleared"], 1);
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
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(session["project_id"], Value::Null);
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
