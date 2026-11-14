//! 沙箱 HTML 预览：对话中的 HTML 代码块与 workspace 内的网页工程（含多文件小游戏）
//! 在独立文档中真正运行，而工作台自身的严格 CSP 不放宽。
//!
//! 隔离手段：响应 CSP 带 `sandbox`（不含 allow-same-origin → 不透明源，无法读写工作台
//! 的 DOM/存储，也不能以同源身份调用 /api），并以 `connect-src 'none'` 禁止网络请求；
//! 前端 iframe 同时声明 sandbox 属性作为第二道防线。

use crate::panels::confined;
use crate::server::{ApiError, AppState};
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::sync::Mutex;
use tokio::io::AsyncReadExt;

/// 内存中最多保留的预览条数与总字节（超出按时间淘汰最旧）。
const MAX_PREVIEWS: usize = 64;
const MAX_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
/// workspace 文件预览的单文件上限（与 /api/file/raw 一致）。
const FILE_CAP: u64 = 8 * 1024 * 1024;

/// 预览文档 CSP：脚本/样式可内联与外链（游戏、图表库），禁止任何 fetch/XHR/WebSocket。
pub(crate) const PREVIEW_CSP: &str = "sandbox allow-scripts allow-forms allow-modals allow-pointer-lock allow-popups allow-downloads; \
default-src 'none'; script-src 'self' 'unsafe-inline' 'unsafe-eval' http: https: data: blob:; \
style-src 'self' 'unsafe-inline' http: https: data:; img-src 'self' http: https: data: blob:; \
font-src 'self' http: https: data:; media-src 'self' http: https: data: blob:; worker-src blob:; \
connect-src 'none'; frame-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";

#[derive(Default)]
pub(crate) struct PreviewStore {
    entries: Mutex<VecDeque<(String, String)>>,
}

impl PreviewStore {
    fn insert(&self, html: String) -> String {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        entries.push_back((id.clone(), html));
        while entries.len() > MAX_PREVIEWS
            || entries.iter().map(|(_, body)| body.len()).sum::<usize>() > MAX_PREVIEW_BYTES
        {
            if entries.pop_front().is_none() {
                break;
            }
        }
        id
    }

    fn get(&self, id: &str) -> Option<String> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, body)| body.clone())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewBody {
    html: String,
}

/// POST /api/preview —— 暂存一段 HTML，返回可在沙箱 iframe 中加载的 id。
pub(crate) async fn preview_create(
    State(state): State<AppState>,
    Json(body): Json<PreviewBody>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if body.html.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "预览内容为空".into()));
    }
    let id = state.previews.insert(body.html);
    Ok((StatusCode::CREATED, Json(json!({ "id": id }))))
}

fn sandboxed(mime: &str, body: impl Into<Body>) -> Result<Response, ApiError> {
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CONTENT_SECURITY_POLICY, PREVIEW_CSP)
        .body(body.into())
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// GET /api/preview/{id} —— 以沙箱 CSP 返回暂存的 HTML。
pub(crate) async fn preview_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    match state.previews.get(&id) {
        Some(html) => sandboxed("text/html; charset=utf-8", html),
        None => Err(ApiError(
            StatusCode::NOT_FOUND,
            "预览已过期，请重新打开".into(),
        )),
    }
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" | "cjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" | "md" | "csv" | "log" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// GET /api/preview/fs/{*path} —— 以沙箱 CSP 提供 workspace 文件，使相对路径引用的
/// 脚本/样式/资源（多文件网页、小游戏）可直接运行。路径约束与文件面板一致。
pub(crate) async fn preview_file(
    State(state): State<AppState>,
    Path(path): Path<String>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let root = state.runtime.config.workspace.clone();
    let target = confined(&root, &path, true).await?;
    let meta = tokio::fs::metadata(&target)
        .await
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "文件不存在".into()))?;
    if meta.is_dir() {
        let index = target.join("index.html");
        if !index.is_file() {
            return Err(ApiError(
                StatusCode::NOT_FOUND,
                "目录下没有 index.html".into(),
            ));
        }
        // 重定向到带尾斜杠的 index，保证相对资源按目录解析。
        let location = if path.ends_with('/') {
            "index.html".to_string()
        } else {
            format!("{}/index.html", path.rsplit('/').next().unwrap_or_default())
        };
        return Response::builder()
            .status(StatusCode::TEMPORARY_REDIRECT)
            .header(header::LOCATION, location)
            .header(header::CONTENT_SECURITY_POLICY, PREVIEW_CSP)
            .body(Body::empty())
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    let mut bytes = Vec::new();
    tokio::fs::File::open(&target)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("无法读取文件：{e}")))?
        .take(FILE_CAP + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("无法读取文件：{e}")))?;
    if bytes.len() as u64 > FILE_CAP {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "文件超过预览上限（>8MiB）".into(),
        ));
    }
    let mut response = sandboxed(mime_for(&target), bytes)?;
    // 沙箱文档是不透明源：其 module script / 字体等 CORS 子资源请求带 `Origin: null`，
    // 中间件仅在 Referer 来自本服务时放行（见 server::local_origin），这里回显许可。
    if headers
        .get(header::ORIGIN)
        .is_some_and(|origin| origin == "null")
    {
        response.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::HeaderValue::from_static("null"),
        );
    }
    Ok(response)
}

/// 中间件辅助：`Origin: null` 仅允许用于沙箱预览的只读子资源，
/// 且 Referer 必须来自本服务（防止任意站点的沙箱 iframe 借此读取 workspace 文件）。
pub(crate) fn allows_null_origin(
    method: &axum::http::Method,
    path: &str,
    referer: Option<&str>,
    host: &str,
) -> bool {
    let preview_path = path.starts_with("/api/preview/fs/")
        || (path.starts_with("/w/") && path.contains("/api/preview/fs/"));
    let own_referer =
        referer.is_some_and(|referer| referer.starts_with(&format!("http://{host}/")));
    method == axum::http::Method::GET && preview_path && own_referer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_evicts_oldest_beyond_limits() {
        let store = PreviewStore::default();
        let first = store.insert("<p>1</p>".into());
        for _ in 0..MAX_PREVIEWS {
            store.insert("<p>x</p>".into());
        }
        assert!(store.get(&first).is_none());
        let big = store.insert("a".repeat(MAX_PREVIEW_BYTES));
        assert_eq!(store.get(&big).map(|b| b.len()), Some(MAX_PREVIEW_BYTES));
        assert_eq!(store.entries.lock().unwrap().len(), 1);
    }

    #[test]
    fn null_origin_requires_preview_get_and_own_referer() {
        let get = axum::http::Method::GET;
        let host = "127.0.0.1:7432";
        let own = Some("http://127.0.0.1:7432/");
        assert!(allows_null_origin(
            &get,
            "/api/preview/fs/game/main.js",
            own,
            host
        ));
        assert!(allows_null_origin(
            &get,
            "/w/demo/api/preview/fs/a.js",
            own,
            host
        ));
        assert!(!allows_null_origin(&get, "/api/file", own, host));
        assert!(!allows_null_origin(
            &get,
            "/api/preview/fs/a.js",
            Some("https://evil.example/"),
            host
        ));
        assert!(!allows_null_origin(
            &get,
            "/api/preview/fs/a.js",
            None,
            host
        ));
        assert!(!allows_null_origin(
            &axum::http::Method::POST,
            "/api/preview/fs/a.js",
            own,
            host
        ));
    }
}
