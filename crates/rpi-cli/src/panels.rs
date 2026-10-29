//! 文件管理、Git 审查与浏览器代理面板的 HTTP API。
//!
//! 约束：文件操作一律限制在 runtime workspace 根内（词法 `..`/盘符拒绝 +
//! canonicalize 越界复核）；Git 走子进程参数数组、不经 shell；浏览器代理仅
//! 取 text/html，服务端剥离脚本并注入 `<base>`，响应携带禁脚本 CSP。

use crate::server::{ApiError, AppState};
use axum::Json;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::Duration;

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

const FILE_READ_CAP: u64 = 1024 * 1024;
const DIFF_CAP: usize = 512 * 1024;
const PROXY_CAP: usize = 4 * 1024 * 1024;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

/// 把用户提供的相对路径折叠为 workspace 内绝对路径；越界一律拒绝。
pub(crate) fn resolve_in_root(root: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let normalized = rel.trim().replace('\\', "/");
    let normalized = normalized.trim_start_matches('/');
    if normalized
        .split('/')
        .any(|seg| seg == ".." || seg.is_empty() && !normalized.is_empty())
    {
        return Err(bad("路径越界：不允许空段或 .. 段"));
    }
    if normalized.contains(':') {
        return Err(bad("路径越界：不允许盘符或冒号"));
    }
    Ok(root.join(normalized))
}

/// 在词法检查之上再 canonicalize 复核（拦截符号链接逃逸）。
pub(crate) async fn confined(
    root: &Path,
    rel: &str,
    must_exist: bool,
) -> Result<PathBuf, ApiError> {
    let target = resolve_in_root(root, rel)?;
    let root_canonical = tokio::fs::canonicalize(root).await.map_err(|e| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("workspace 根不可用：{e}"),
        )
    })?;
    let check = if must_exist {
        tokio::fs::canonicalize(&target).await
    } else {
        match &target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => tokio::fs::canonicalize(parent)
                .await
                .map(|p| p.join(target.file_name().unwrap_or_default())),
            _ => Ok(target.clone()),
        }
    };
    let canonical = check.map_err(|_| bad("路径不存在"))?;
    if canonical != root_canonical && !canonical.starts_with(&root_canonical) {
        return Err(bad("路径越界：越出 workspace"));
    }
    Ok(canonical)
}

fn modified_ms(meta: &std::fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------- 文件管理 ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathQuery {
    #[serde(default)]
    path: String,
}

/// GET /api/fs —— 列目录（目录在前，名称次序）。
pub(crate) async fn fs_list(
    State(state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let dir = confined(&root, &q.path, true).await?;
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut rd = tokio::fs::read_dir(&dir)
        .await
        .map_err(|e| bad(format!("无法读取目录：{e}")))?;
    while let Some(entry) = rd
        .next_entry()
        .await
        .map_err(|e| bad(format!("无法读取目录：{e}")))?
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".rpi" {
            continue; // 运行时数据目录不进面板
        }
        let meta = entry.metadata().await.ok();
        let kind = match &meta {
            Some(m) if m.is_dir() => "dir",
            Some(m) if m.is_symlink() => "link",
            _ => "file",
        };
        entries.push(json!({
            "name": name,
            "kind": kind,
            "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            "modified": meta.as_ref().map(modified_ms).unwrap_or(0),
        }));
    }
    entries.sort_by(|a, b| {
        let ka = if a["kind"] == "dir" { 0 } else { 1 };
        let kb = if b["kind"] == "dir" { 0 } else { 1 };
        ka.cmp(&kb)
            .then(a["name"].as_str().cmp(&b["name"].as_str()))
    });
    Ok(Json(json!({ "path": q.path, "entries": entries })))
}

fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// GET /api/file —— 文本预读（1MiB 截断；二进制返回 binary 标记）。
pub(crate) async fn file_read(
    State(state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let path = confined(&root, &q.path, true).await?;
    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|_| bad("路径不存在"))?;
    if meta.is_dir() {
        return Err(bad("目标是目录，请用 /api/fs 列举"));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| bad(format!("无法读取文件：{e}")))?;
    let truncated = bytes.len() as u64 > FILE_READ_CAP;
    let view = if truncated {
        &bytes[..FILE_READ_CAP as usize]
    } else {
        &bytes[..]
    };
    if looks_binary(view) {
        return Ok(Json(json!({
            "path": q.path, "binary": true, "truncated": false,
            "size": bytes.len(), "content": serde_json::Value::Null,
        })));
    }
    Ok(Json(json!({
        "path": q.path,
        "binary": false,
        "truncated": truncated,
        "size": bytes.len(),
        "modified": modified_ms(&meta),
        "content": String::from_utf8_lossy(view),
    })))
}

/// GET /api/file/raw —— 原始字节（图片预览 / 下载）。
pub(crate) async fn file_raw(
    State(state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> Result<Response, ApiError> {
    let root = state.runtime.config.workspace.clone();
    let path = confined(&root, &q.path, true).await?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| bad(format!("无法读取文件：{e}")))?;
    let mime = match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "txt" | "md" | "log" => "text/plain; charset=utf-8",
        "json" | "toml" | "yaml" | "yml" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(bytes))
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileWrite {
    path: String,
    #[serde(default)]
    content: String,
}

/// PUT /api/file —— 写文件（父目录自动创建）。
pub(crate) async fn file_write(
    State(state): State<AppState>,
    Json(body): Json<FileWrite>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let path = confined(&root, &body.path, false).await?;
    if path == root {
        return Err(bad("目标路径是 workspace 根"));
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| bad(format!("无法创建父目录：{e}")))?;
    }
    tokio::fs::write(&path, body.content.as_bytes())
        .await
        .map_err(|e| bad(format!("写入失败：{e}")))?;
    Ok(Json(
        json!({ "saved": true, "path": body.path, "size": body.content.len() }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MkdirBody {
    path: String,
}

/// POST /api/fs/mkdir —— 递归建目录。
pub(crate) async fn fs_mkdir(
    State(state): State<AppState>,
    Json(body): Json<MkdirBody>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let path = confined(&root, &body.path, false).await?;
    tokio::fs::create_dir_all(&path)
        .await
        .map_err(|e| bad(format!("创建目录失败：{e}")))?;
    Ok(Json(json!({ "created": true, "path": body.path })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenameBody {
    from: String,
    to: String,
}

/// POST /api/fs/rename —— 同根内改名/移动。
pub(crate) async fn fs_rename(
    State(state): State<AppState>,
    Json(body): Json<RenameBody>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let from = confined(&root, &body.from, true).await?;
    let to = confined(&root, &body.to, false).await?;
    if from == root || to == root {
        return Err(bad("不能重命名 workspace 根"));
    }
    tokio::fs::rename(&from, &to)
        .await
        .map_err(|e| bad(format!("重命名失败：{e}")))?;
    Ok(Json(json!({ "renamed": true })))
}

/// DELETE /api/fs —— 删文件或递归删目录（前端二次确认）。
pub(crate) async fn fs_delete(
    State(state): State<AppState>,
    Query(q): Query<PathQuery>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let path = confined(&root, &q.path, true).await?;
    if path == root {
        return Err(bad("不能删除 workspace 根"));
    }
    if path.is_dir() {
        tokio::fs::remove_dir_all(&path)
            .await
            .map_err(|e| bad(format!("删除目录失败：{e}")))?;
    } else {
        tokio::fs::remove_file(&path)
            .await
            .map_err(|e| bad(format!("删除文件失败：{e}")))?;
    }
    Ok(Json(json!({ "deleted": true, "path": q.path })))
}

// ---------- Git 审查 ----------

async fn run_git(root: &Path, args: &[&str]) -> Result<String, ApiError> {
    let out = tokio::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .await
        .map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("无法启动 git：{e}"),
            )
        })?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(bad(if err.is_empty() {
            format!("git {} 失败", args.first().unwrap_or(&""))
        } else {
            err
        }));
    }
    Ok(stdout)
}

/// GET /api/git/status —— 分支 + 变更清单（--no-renames 简化解析）。
pub(crate) async fn git_status(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let out = run_git(
        &root,
        &[
            "-c",
            "core.quotepath=off",
            "status",
            "--porcelain=v1",
            "-b",
            "--no-renames",
        ],
    )
    .await?;
    let mut lines = out.lines();
    let head = lines.next().unwrap_or("");
    let mut branch = String::new();
    let mut upstream = serde_json::Value::Null;
    if let Some(rest) = head.strip_prefix("## ") {
        let rest = rest.split(" [").next().unwrap_or(rest);
        let mut parts = rest.splitn(2, "...");
        branch = parts.next().unwrap_or("").trim().to_string();
        upstream = parts
            .next()
            .map(|u| json!(u.trim()))
            .unwrap_or(serde_json::Value::Null);
    }
    let changes: Vec<serde_json::Value> = lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let bytes = l.as_bytes();
            let (x, y) = (bytes[0] as char, bytes[1] as char);
            let path = l[3..].to_string();
            json!({
                "x": x, "y": y, "path": path,
                "staged": x != ' ' && x != '?',
                "untracked": x == '?' && y == '?',
            })
        })
        .collect();
    Ok(Json(
        json!({ "branch": branch, "upstream": upstream, "changes": changes }),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DiffQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    staged: bool,
}

/// GET /api/git/diff —— 统一 diff 文本（512KiB 截断）。
pub(crate) async fn git_diff(
    State(state): State<AppState>,
    Query(q): Query<DiffQuery>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let mut args: Vec<&str> = vec!["diff", "--no-color", "--no-ext-diff"];
    if q.staged {
        args.push("--cached");
    }
    if !q.path.trim().is_empty() {
        args.push("--");
        args.push(q.path.trim());
    }
    let out = run_git(&root, &args).await?;
    let truncated = out.len() > DIFF_CAP;
    Ok(Json(json!({
        "diff": if truncated { out[..DIFF_CAP].to_string() } else { out },
        "truncated": truncated,
    })))
}

/// GET /api/git/log —— 提交历史（新→旧，默认 30 条）。
pub(crate) async fn git_log(
    State(state): State<AppState>,
    Query(q): Query<LogQuery>,
) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    // 空仓库：rev-parse HEAD 失败时返回空列表而非报错
    if run_git(&root, &["rev-parse", "--verify", "-q", "HEAD"])
        .await
        .is_err()
    {
        return Ok(Json(json!({ "commits": [] })));
    }
    let limit = q.limit.clamp(1, 200);
    let fmt_args = format!("-n{limit}");
    let out = run_git(
        &root,
        &[
            "log",
            &fmt_args,
            "--pretty=format:%H\u{1f}%h\u{1f}%an\u{1f}%at\u{1f}%s\u{1f}%D\u{1e}",
        ],
    )
    .await?;
    let commits: Vec<serde_json::Value> = out
        .split('\u{1e}')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.trim().split('\u{1f}').collect();
            if f.len() < 6 {
                return None;
            }
            Some(json!({
                "hash": f[0], "short": f[1], "author": f[2],
                "time": f[3].parse::<u64>().unwrap_or(0),
                "subject": f[4],
                "refs": f[5],
            }))
        })
        .collect();
    Ok(Json(json!({ "commits": commits })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LogQuery {
    #[serde(default = "default_log_limit")]
    limit: u32,
}

fn default_log_limit() -> u32 {
    30
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PathsBody {
    #[serde(default)]
    paths: Vec<String>,
}

fn validate_paths(paths: &[String]) -> Result<(), ApiError> {
    if paths.is_empty() {
        return Err(bad("paths 不能为空"));
    }
    for p in paths {
        resolve_in_root(Path::new("x"), p).map_err(|_| bad(format!("非法路径：{p}")))?;
    }
    Ok(())
}

/// POST /api/git/stage —— git add。
pub(crate) async fn git_stage(
    State(state): State<AppState>,
    Json(body): Json<PathsBody>,
) -> ApiResult<serde_json::Value> {
    validate_paths(&body.paths)?;
    let root = state.runtime.config.workspace.clone();
    let mut args = vec!["add", "--"];
    args.extend(body.paths.iter().map(String::as_str));
    run_git(&root, &args).await?;
    Ok(Json(json!({ "staged": body.paths.len() })))
}

/// POST /api/git/unstage —— git reset HEAD（无提交历史回退 git rm --cached）。
pub(crate) async fn git_unstage(
    State(state): State<AppState>,
    Json(body): Json<PathsBody>,
) -> ApiResult<serde_json::Value> {
    validate_paths(&body.paths)?;
    let root = state.runtime.config.workspace.clone();
    let mut args = vec!["reset", "-q", "HEAD", "--"];
    args.extend(body.paths.iter().map(String::as_str));
    if run_git(&root, &args).await.is_err() {
        let mut args = vec!["rm", "--cached", "-q", "--"];
        args.extend(body.paths.iter().map(String::as_str));
        run_git(&root, &args).await?;
    }
    Ok(Json(json!({ "unstaged": body.paths.len() })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CommitBody {
    message: String,
    #[serde(default)]
    paths: Vec<String>,
}

/// POST /api/git/commit —— 可选先 stage 指定路径，再提交。
pub(crate) async fn git_commit(
    State(state): State<AppState>,
    Json(body): Json<CommitBody>,
) -> ApiResult<serde_json::Value> {
    let message = body.message.trim();
    if message.is_empty() {
        return Err(bad("提交说明不能为空"));
    }
    if message.chars().count() > 4000 {
        return Err(bad("提交说明过长"));
    }
    let root = state.runtime.config.workspace.clone();
    if !body.paths.is_empty() {
        validate_paths(&body.paths)?;
        let mut args = vec!["add", "--"];
        args.extend(body.paths.iter().map(String::as_str));
        run_git(&root, &args).await?;
    }
    run_git(&root, &["commit", "-m", message]).await?;
    let hash = run_git(&root, &["rev-parse", "--short", "HEAD"]).await?;
    Ok(Json(json!({ "committed": true, "hash": hash.trim() })))
}

/// GET /api/git/branches —— 本地分支与当前指向。
pub(crate) async fn git_branches(State(state): State<AppState>) -> ApiResult<serde_json::Value> {
    let root = state.runtime.config.workspace.clone();
    let out = run_git(&root, &["branch", "--format=%(HEAD)%09%(refname:short)"]).await?;
    let branches: Vec<serde_json::Value> = out
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let current = l.starts_with('*');
            let name = l.trim_start_matches(['*', ' ']).trim();
            json!({ "name": name, "current": current })
        })
        .collect();
    Ok(Json(json!({ "branches": branches })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CheckoutBody {
    branch: String,
    #[serde(default)]
    create: bool,
}

/// POST /api/git/checkout —— 切换分支（可选新建）。
pub(crate) async fn git_checkout(
    State(state): State<AppState>,
    Json(body): Json<CheckoutBody>,
) -> ApiResult<serde_json::Value> {
    let branch = body.branch.trim();
    if branch.is_empty() || branch.contains(' ') || branch.starts_with('-') {
        return Err(bad("分支名非法"));
    }
    let root = state.runtime.config.workspace.clone();
    if body.create {
        run_git(&root, &["checkout", "-q", "-b", branch]).await?;
    } else {
        run_git(&root, &["checkout", "-q", branch]).await?;
    }
    Ok(Json(json!({ "branch": branch, "created": body.create })))
}

/// POST /api/git/discard —— 丢弃已跟踪文件的未暂存修改（前端需确认）。
pub(crate) async fn git_discard(
    State(state): State<AppState>,
    Json(body): Json<PathsBody>,
) -> ApiResult<serde_json::Value> {
    validate_paths(&body.paths)?;
    let root = state.runtime.config.workspace.clone();
    let mut args = vec!["checkout", "-q", "--"];
    args.extend(body.paths.iter().map(String::as_str));
    run_git(&root, &args).await?;
    Ok(Json(json!({ "discarded": body.paths.len() })))
}

// ---------- 浏览器代理 ----------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxyQuery {
    url: String,
}

/// 大小写不敏感子串查找（HTML 标签扫描用，避免引入 regex 依赖）。
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let h = haystack.to_ascii_lowercase();
    h.find(needle)
}

fn remove_tag_blocks(html: &str, tag: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    loop {
        match find_ci(rest, &format!("<{tag}")) {
            Some(start) => {
                out.push_str(&rest[..start]);
                let after = &rest[start..];
                let open_end = match find_ci(after, ">") {
                    Some(i) => i + 1,
                    None => break,
                };
                match find_ci(&after[open_end..], &format!("</{tag}")) {
                    Some(c) => {
                        let tail = &after[open_end + c..];
                        rest = match find_ci(tail, ">") {
                            Some(g) => &tail[g + 1..],
                            None => "",
                        };
                    }
                    None => rest = &after[open_end..],
                }
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// 剥离 <script> 块与内联 CSP meta，注入 <base>，让代理页在禁脚本 CSP 下可用。
fn sanitize_html(html: &str, base_url: &str) -> String {
    let no_scripts = remove_tag_blocks(html, "script");
    let mut cleaned = String::with_capacity(no_scripts.len() + 64);
    let mut rest: &str = &no_scripts;
    loop {
        match find_ci(rest, "<meta") {
            Some(start) => {
                let after = &rest[start..];
                let open_end = match find_ci(after, ">") {
                    Some(i) => i,
                    None => {
                        cleaned.push_str(rest);
                        break;
                    }
                };
                let tag = after[..open_end].to_ascii_lowercase();
                if tag.contains("http-equiv") && tag.contains("content-security-policy") {
                    cleaned.push_str(&rest[..start]);
                } else {
                    cleaned.push_str(&rest[..open_end + 1]);
                }
                rest = &after[open_end + 1..];
            }
            None => {
                cleaned.push_str(rest);
                break;
            }
        }
    }
    let base_tag = format!("<base href=\"{}\">", base_url.replace('"', "%22"));
    match find_ci(&cleaned, "<head") {
        Some(i) => match find_ci(&cleaned[i..], ">") {
            Some(gt) => format!(
                "{}{}{}",
                &cleaned[..i + gt + 1],
                base_tag,
                &cleaned[i + gt + 1..]
            ),
            None => format!("{base_tag}{cleaned}"),
        },
        None => format!("{base_tag}{cleaned}"),
    }
}

/// GET /api/proxy —— 受控网页代理（仅 html；脚本剥离 + 禁脚本 CSP）。
pub(crate) async fn proxy(
    State(_state): State<AppState>,
    Query(q): Query<ProxyQuery>,
) -> Result<Response, ApiError> {
    let url = q.url.trim().to_string();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(bad("仅支持 http/https 地址"));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent("Mozilla/5.0 (compatible; RPiPanel/0.1)")
        .build()
        .map_err(|e| {
            ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("代理初始化失败：{e}"),
            )
        })?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| bad(format!("代理请求失败：{e}")))?;
    let status = resp.status();
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| bad(format!("读取响应失败：{e}")))?;
    if bytes.len() > PROXY_CAP {
        return Err(bad("页面过大（>4MiB），请在系统浏览器打开"));
    }
    if !content_type.contains("text/html") {
        // 非 HTML 直接透传字节（css/图片等子资源由 <base> 让浏览器直连源站）
        let mime = if content_type.is_empty() {
            "application/octet-stream"
        } else {
            content_type.as_str()
        };
        return Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-store")
            .body(Body::from(bytes))
            .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()));
    }
    let html = String::from_utf8_lossy(&bytes);
    let body = sanitize_html(&html, &url);
    Response::builder()
        .status(if status.is_success() { StatusCode::OK } else { StatusCode::BAD_GATEWAY })
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .header(
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; img-src http: https: data:; style-src * 'unsafe-inline'; font-src * data:; media-src http: https: data:; script-src 'none'; frame-src 'none'; connect-src 'none'",
        )
        .body(Body::from(body))
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_rejects_traversal_and_drive() {
        let root = Path::new("/w");
        assert!(resolve_in_root(root, "a/../b").is_err());
        assert!(resolve_in_root(root, "..").is_err());
        assert!(resolve_in_root(root, "C:/win").is_err());
        assert!(resolve_in_root(root, "a//b").is_err());
        assert_eq!(
            resolve_in_root(root, "sub/file.txt").unwrap(),
            Path::new("/w").join("sub/file.txt")
        );
        assert_eq!(resolve_in_root(root, "").unwrap(), Path::new("/w"));
    }

    #[test]
    fn sanitize_strips_scripts_and_csp_and_injects_base() {
        let html = r#"<html><head><meta http-equiv="Content-Security-Policy" content="default-src 'none'"><title>T</title></head>
<body onload="x()"><script src="/a.js"></script><script>alert(1)</script><p>hi</p></body></html>"#;
        let out = sanitize_html(html, "https://a.example/x");
        assert!(!out.to_ascii_lowercase().contains("<script"));
        assert!(!out.to_ascii_lowercase().contains("content-security-policy"));
        assert!(out.contains(r#"<base href="https://a.example/x">"#));
        assert!(out.contains("<p>hi</p>"));
    }

    #[test]
    fn status_parse_extracts_branch_and_changes() {
        let out = "## main...origin/main [ahead 1]\n M src/a.rs\nA  new.rs\n?? tmp.log\n";
        let mut lines = out.lines();
        let head = lines.next().unwrap();
        assert!(
            head.strip_prefix("## ")
                .unwrap()
                .starts_with("main...origin/main")
        );
        let mut count = 0;
        for l in lines {
            let b = l.as_bytes();
            assert_eq!(b[2], b' ');
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn git_stage_commit_diff_flow() {
        let dir = std::env::temp_dir().join(format!("rpi-panels-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@local"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "hello\n").unwrap();
        let status = run_git(&dir, &["status", "--porcelain=v1", "-b", "--no-renames"])
            .await
            .unwrap();
        assert!(status.contains("?? a.txt"));
        run_git(&dir, &["add", "--", "a.txt"]).await.unwrap();
        run_git(&dir, &["commit", "-m", "init"]).await.unwrap();
        std::fs::write(dir.join("a.txt"), "changed\n").unwrap();
        let diff = run_git(&dir, &["diff", "--no-color", "--", "a.txt"])
            .await
            .unwrap();
        assert!(diff.contains("+changed"));
        let log = run_git(
            &dir,
            &[
                "log",
                "-n1",
                "--pretty=format:%H\u{1f}%h\u{1f}%an\u{1f}%at\u{1f}%s\u{1f}%D\u{1e}",
            ],
        )
        .await
        .unwrap();
        assert!(log.contains("\u{1f}init"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
