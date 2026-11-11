//! 动态工作区 Hub：运行期按路径创建 / 重命名 / 摘除工作区，持久化到
//! `data_dir/workspaces.json`（temp+rename 原子写，与 projects.json 同模式）。
//!
//! 静态 `config.workspaces` 仍由 `attach_workspaces` nest 挂载；动态项由
//! `/w/{id}` 参数路由 dispatch（matchit 静态优先，两种挂载共存）。未知 id
//! 的请求委托给 `workspace_fallback`，保持尾斜杠 308 与 404 语义不变。

use crate::server::{ApiError, AppState};
type ApiResult<T> = std::result::Result<Json<T>, ApiError>;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use samho_core::AgentRuntime;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// 动态工作区数量上限：每个工作区一个独立运行时（含连接池/MCP/技能扫描），需要限界。
const MAX_DYNAMIC_WORKSPACES: usize = 32;

pub(crate) type SharedRouter = axum::Router;

#[derive(Clone)]
pub(crate) struct WorkspaceEntry {
    pub(crate) runtime: Arc<AgentRuntime>,
    pub(crate) router: SharedRouter,
    pub(crate) terms: std::sync::Arc<crate::term::TermHub>,
    pub(crate) name: String,
}

/// 可被 AppState 共享的动态工作区注册表。
#[derive(Clone)]
pub(crate) struct HubState {
    pub(crate) dynamic: Arc<RwLock<BTreeMap<String, WorkspaceEntry>>>,
    registry_path: Arc<PathBuf>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceRecord {
    id: String,
    name: String,
    path: PathBuf,
    data_dir: PathBuf,
}

impl HubState {
    pub(crate) fn new(registry_path: PathBuf) -> Self {
        Self {
            dynamic: Arc::new(RwLock::new(BTreeMap::new())),
            registry_path: Arc::new(registry_path),
        }
    }

    async fn snapshot(&self) -> Vec<(String, WorkspaceEntry)> {
        self.dynamic
            .read()
            .await
            .iter()
            .map(|(id, entry)| (id.clone(), entry.clone()))
            .collect()
    }

    async fn persist(&self, records: &[WorkspaceRecord]) -> Result<(), ApiError> {
        let payload = serde_json::to_vec_pretty(records).map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("序列化失败：{e}"),
            )
        })?;
        let parent = self
            .registry_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let temp = parent.join(format!(
            "workspaces.json.tmp-{}",
            uuid::Uuid::new_v4().simple()
        ));
        tokio::fs::create_dir_all(&parent).await.map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("data_dir 不可用：{e}"),
            )
        })?;
        tokio::fs::write(&temp, &payload)
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("写入失败：{e}")))?;
        tokio::fs::rename(&temp, self.registry_path.as_path())
            .await
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("发布失败：{e}")))?;
        Ok(())
    }

    /// 启动时恢复上次保存的动态工作区；条目损坏或运行时创建失败则告警并剔除。
    /// `root_shutdown` 用于派生每个工作区 AppState 的子取消令牌：根服务优雅
    /// 退出时随之取消动态工作区的 SSE/任务，不再让 graceful shutdown 无界等待。
    pub(crate) async fn load_saved(
        &self,
        base_config: samho_core::RuntimeConfig,
        root_shutdown: CancellationToken,
    ) {
        let Ok(bytes) = tokio::fs::read(self.registry_path.as_path()).await else {
            return;
        };
        let Ok(records) = serde_json::from_slice::<Vec<WorkspaceRecord>>(&bytes) else {
            tracing::warn!("workspaces.json 损坏，忽略动态工作区恢复");
            return;
        };
        let mut kept = Vec::new();
        for record in records {
            if !workspace_id_valid(&record.id)
                || record.id == "default"
                || self.dynamic.read().await.contains_key(&record.id)
            {
                tracing::warn!(id = %record.id, "跳过非法或重复的动态工作区");
                continue;
            }
            let mut config = base_config.clone();
            config.workspace = record.path.clone();
            config.data_dir = record.data_dir.clone();
            match crate::roles::build_runtime(config).await {
                Ok((runtime, roles)) => {
                    let app_state = AppState::with_roles(runtime.clone(), roles)
                        .with_shutdown(root_shutdown.child_token());
                    let router = crate::server::router_with_state(app_state.clone());
                    self.dynamic.write().await.insert(
                        record.id.clone(),
                        WorkspaceEntry {
                            runtime,
                            router,
                            terms: app_state.terms.clone(),
                            name: record.name.clone(),
                        },
                    );
                    kept.push(record);
                }
                Err(error) => {
                    tracing::warn!(id = %record.id, error = %error, "动态工作区恢复失败，已剔除");
                }
            }
        }
        if let Err(error) = self.persist(&kept).await {
            tracing::warn!(error = %error.1, "workspaces.json 重写失败");
        }
    }

    pub(crate) async fn shutdown_all(&self) {
        for (_, entry) in self.snapshot().await {
            entry.terms.shutdown_all().await;
            if let Err(error) = entry.runtime.shutdown().await {
                tracing::warn!(error = %error, "动态工作区关停失败");
            }
        }
    }
}

fn workspace_id_valid(id: &str) -> bool {
    !id.is_empty()
        && id != "default"
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 名字转 ASCII slug（中文名降级为 ws-序号）；与 config 校验规则对齐。
fn slugify(input: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true;
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn unique_id(base: &str, taken: &BTreeSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    for n in 2..u32::MAX {
        let candidate = format!("{base}-{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("unique workspace id");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceCreate {
    path: String,
    #[serde(default)]
    name: Option<String>,
}

/// POST /api/workspaces：按路径创建动态工作区（独立运行时 + 独立 data_dir）。
pub(crate) async fn workspace_create(
    State(app): State<AppState>,
    Json(body): Json<WorkspaceCreate>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let hub = app.hub.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "工作区管理仅支持在根服务上进行".into(),
        )
    })?;
    let raw = body.path.trim();
    if raw.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "路径不能为空".into()));
    }
    let path = tokio::fs::canonicalize(raw)
        .await
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("路径不存在：{raw}")))?;
    if !path.is_dir() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "路径不是目录".into()));
    }
    let name = match body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(n) if n.chars().count() <= 64 => n.to_string(),
        Some(_) => {
            return Err(ApiError(
                StatusCode::BAD_REQUEST,
                "名称过长（≤64 字符）".into(),
            ));
        }
        None => path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "工作区".into()),
    };
    let dynamic = hub.dynamic.read().await;
    let mut taken: BTreeSet<String> = BTreeSet::from(["default".to_string()]);
    taken.extend(dynamic.keys().cloned());
    taken.extend(app.runtime.config.workspaces.iter().map(|w| w.id.clone()));
    drop(dynamic);
    let base = slugify(&name);
    let base = if base.is_empty() {
        "ws".to_string()
    } else {
        base
    };
    let id = unique_id(&base, &taken);
    let data_dir = app.runtime.config.data_dir.join("workspaces").join(&id);
    let mut config = app.runtime.config.clone();
    config.workspace = path.clone();
    config.data_dir = data_dir.clone();
    let (runtime, roles) = crate::roles::build_runtime(config)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("工作区初始化失败：{e}")))?;
    let app_state =
        AppState::with_roles(runtime.clone(), roles).with_shutdown(app.shutdown.child_token());
    let router = crate::server::router_with_state(app_state.clone());
    let mut dynamic = hub.dynamic.write().await;
    if dynamic.len() >= MAX_DYNAMIC_WORKSPACES {
        drop(dynamic);
        let _ = runtime.shutdown().await;
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            format!("动态工作区已达上限（{MAX_DYNAMIC_WORKSPACES}）"),
        ));
    }
    dynamic.insert(
        id.clone(),
        WorkspaceEntry {
            runtime: runtime.clone(),
            router,
            terms: app_state.terms.clone(),
            name: name.clone(),
        },
    );
    // 持久化当前注册表全集（旧实现只写本次新条目，会覆盖先前工作区的
    // 注册记录，重启后丢失）。
    let records = hub_record_list(&dynamic);
    drop(dynamic);
    if let Err(error) = hub.persist(&records).await {
        // 持久化失败不回滚内存态，但要让调用方知道重启会丢失。
        tracing::warn!(error = %error.1, "workspaces.json 持久化失败");
    }
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "id": id,
            "name": name,
            "path": path,
            "url": format!("/w/{id}/"),
            "managed": true,
        })),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorkspaceRename {
    name: String,
}

/// PATCH /api/workspaces/{id}：仅动态工作区可改名（静态项归属 runtime.toml）。
pub(crate) async fn workspace_rename(
    State(app): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<WorkspaceRename>,
) -> ApiResult<serde_json::Value> {
    let hub = app.hub.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "工作区管理仅支持在根服务上进行".into(),
        )
    })?;
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "名称需为 1–64 字符".into(),
        ));
    }
    let mut dynamic = hub.dynamic.write().await;
    let Some(entry) = dynamic.get_mut(&id) else {
        return Err(ApiError(StatusCode::NOT_FOUND, "动态工作区不存在".into()));
    };
    entry.name = name.to_string();
    let records = hub_record_list(&dynamic);
    drop(dynamic);
    hub.persist(&records).await?;
    Ok(Json(json!({"id": id, "name": name})))
}

/// DELETE /api/workspaces/{id}：摘除动态工作区（运行时关停、注册表移除；
/// 磁盘数据保留）。静态 config 工作区拒绝在此删除。
pub(crate) async fn workspace_remove(
    State(app): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let hub = app.hub.as_ref().ok_or_else(|| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "工作区管理仅支持在根服务上进行".into(),
        )
    })?;
    if id == "default" {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "默认工作区不可移除".into(),
        ));
    }
    if app.runtime.config.workspaces.iter().any(|w| w.id == id) {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "配置文件声明的工作区请在 runtime.toml 中移除".into(),
        ));
    }
    let entry = hub.dynamic.write().await.remove(&id);
    let Some(entry) = entry else {
        return Err(ApiError(StatusCode::NOT_FOUND, "动态工作区不存在".into()));
    };
    let records = {
        let map = hub.dynamic.read().await;
        hub_record_list(&map)
    };
    if let Err(error) = hub.persist(&records).await {
        tracing::warn!(error = %error.1, "workspaces.json 持久化失败");
    }
    // 先关闭该工作区的终端会话（否则 PTY 子进程可能残留），再关停运行时。
    entry.terms.shutdown_all().await;
    entry.runtime.shutdown().await?;
    Ok(Json(json!({"removed": id})))
}

fn hub_record_list(dynamic: &BTreeMap<String, WorkspaceEntry>) -> Vec<WorkspaceRecord> {
    dynamic
        .iter()
        .map(|(id, entry)| WorkspaceRecord {
            id: id.clone(),
            name: entry.name.clone(),
            path: entry.runtime.config.workspace.clone(),
            data_dir: entry.runtime.config.data_dir.clone(),
        })
        .collect()
}

/// /w/{id} 与 /w/{id}/{*rest} 的统一分发：剥离前缀后交给该工作区路由；
/// 未知 id 委托 workspace_fallback（保持 308/404 语义）。
/// 不用 Path 提取器：`{*rest}` 通配会让捕获数变成 2，直接从 URI 解析 id。
pub(crate) async fn workspace_dispatch(
    State(app): State<AppState>,
    mut request: axum::extract::Request,
) -> Response {
    let uri = request.uri().clone();
    let raw = uri.path().strip_prefix("/w/").unwrap_or_default();
    let id = raw.split('/').next().unwrap_or_default();
    let entry = match app.hub.as_ref() {
        Some(hub) => hub.dynamic.read().await.get(id).cloned(),
        None => None,
    };
    let Some(entry) = entry else {
        return crate::server::workspace_fallback(
            Uri::from_parts(uri.into_parts()).unwrap_or_default(),
        )
        .await;
    };
    let prefix = format!("/w/{id}");
    let rest = uri.path().strip_prefix(&prefix).unwrap_or("/");
    let rest = if rest.is_empty() { "/" } else { rest };
    let path_and_query = match uri.query() {
        Some(query) => format!("{rest}?{query}"),
        None => rest.to_string(),
    };
    let mut parts = uri.into_parts();
    match path_and_query.parse() {
        Ok(pq) => parts.path_and_query = Some(pq),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    }
    match Uri::from_parts(parts) {
        Ok(new_uri) => *request.uri_mut() = new_uri,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    }
    // 外层 match 留下的 RawPathParams/MatchedPath 会让嵌套路由的 Path 提取器
    // 多算参数（"Expected 1 but got 3"），转发前清掉。
    request.extensions_mut().clear();
    match entry.router.clone().oneshot(request).await {
        Ok(response) => response,
        Err(infallible) => match infallible {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_keeps_ascii_words_and_drops_others() {
        assert_eq!(slugify("前端重构"), "");
        assert_eq!(slugify("Demo Space"), "demo-space");
        assert_eq!(slugify("--A_b--"), "a-b");
        assert_eq!(slugify("///"), "");
    }

    #[test]
    fn unique_id_appends_counters() {
        let taken = BTreeSet::from(["ws".into(), "ws-2".into(), "ws-3".into()]);
        assert_eq!(unique_id("ws", &taken), "ws-4");
        assert_eq!(unique_id("fresh", &taken), "fresh");
    }

    #[test]
    fn workspace_id_rules_reject_default_and_symbols() {
        assert!(workspace_id_valid("aux-1"));
        assert!(!workspace_id_valid("default"));
        assert!(!workspace_id_valid(""));
        assert!(!workspace_id_valid("空 格"));
    }

    #[tokio::test]
    async fn persist_then_load_roundtrips_registry() {
        let dir = tempfile::tempdir().unwrap();
        let hub = HubState::new(dir.path().join("workspaces.json"));
        let records = vec![WorkspaceRecord {
            id: "demo".into(),
            name: "演示".into(),
            path: dir.path().join("demo"),
            data_dir: dir.path().join("data/demo"),
        }];
        hub.persist(&records).await.unwrap();
        assert!(dir.path().join("workspaces.json").exists());
        // load_saved 会真正创建运行时；这里只验证持久化文件可反序列化。
        let bytes = tokio::fs::read(dir.path().join("workspaces.json"))
            .await
            .unwrap();
        let parsed: Vec<WorkspaceRecord> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "demo");
    }
}
