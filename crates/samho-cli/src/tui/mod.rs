//! TUI：`samho tui --url` 连接运行中的 `samho serve`（纯 HTTP/SSE 客户端）。
//!
//! 模块划分：`api` HTTP/SSE 封装、`sse` SSE 解析器、`markdown` Markdown
//! 渲染器；视图自 M2 起按 views/ 子模块扩展。数据契约与 web 前端一致：
//! 进度为 TaskProgress 累计快照（按 revision 整体替换），轨迹为 TraceEvent。

mod api;
mod markdown;
mod sse;

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const TIERS: [&str; 7] = [
    "auto",
    "nano",
    "fast",
    "balanced",
    "reasoning",
    "frontier",
    "specialist",
];
const PANELS: [&str; 4] = ["轨迹", "对象", "技能", "MCP"];
const ACCENT: Color = Color::Rgb(188, 229, 87);
const MUTED: Color = Color::Rgb(128, 142, 151);
const BACKGROUND: Color = Color::Rgb(14, 20, 24);

#[derive(Default)]
struct App {
    input: String,
    messages: Vec<(String, String)>,
    status: String,
    models: Vec<Value>,
    sessions: Vec<Value>,
    objects: Vec<Value>,
    skills: Vec<Value>,
    events: VecDeque<Value>,
    state: Value,
    active: Option<String>,
    submitting: bool,
    session_id: Option<String>,
    selected_model: usize,
    selected_skill: usize,
    tier: usize,
    panel: usize,
    scroll: u16,
    transcript_version: u64,
    ui: Value,
    progress: Value,
    workspaces: Vec<Value>,
    message_count: usize,
    // SSE 连接态、局域网地址、事件游标与 Markdown 缓存
    live_sse: bool,
    trace_sse: bool,
    lan: String,
    last_event_seq: u64,
    /// (宽度, 原文, 渲染结果) —— 流式进度正文
    progress_md: Option<(u16, String, Vec<Line<'static>>)>,
    /// (宽度, 原文, 渲染结果) —— 最后一条助手消息
    assistant_md: Option<(u16, String, Vec<Line<'static>>)>,
}

enum Update {
    Submitted(String),
    Progress(Value),
    Notice(String),
    Finished(Value),
    Error(String),
    TaskError(String),
    TraceEvent(Value),
    Sse {
        live: Option<bool>,
        trace: Option<bool>,
    },
    Resources {
        state: Value,
        models: Vec<Value>,
        sessions: Vec<Value>,
        objects: Vec<Value>,
        skills: Vec<Value>,
        ui: Value,
        workspaces: Vec<Value>,
        network: Value,
    },
    Session {
        value: Value,
        version: u64,
    },
}

/// 三个后台流（资源轮询 + 两条 SSE），切换工作区/退出时统一终止。
struct Streams {
    poller: JoinHandle<()>,
    live: JoinHandle<()>,
    trace: JoinHandle<()>,
}

impl Streams {
    fn abort(&self) {
        self.poller.abort();
        self.live.abort();
        self.trace.abort();
    }
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

fn array(value: Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}
fn id(value: &Value) -> Option<&str> {
    value
        .get("id")
        .or_else(|| value.get("session_id"))
        .and_then(Value::as_str)
}

fn tier_names(app: &App) -> Vec<String> {
    let mut tiers = vec!["auto".into()];
    if let Some(definitions) = app.state["tiers"].as_array() {
        for tier in definitions {
            if let Some(name) = tier["id"].as_str().or_else(|| tier.as_str())
                && name != "auto"
                && !tiers.iter().any(|existing| existing == name)
            {
                tiers.push(name.into());
            }
        }
    }
    if tiers.len() == 1 {
        tiers.extend(TIERS[1..].iter().map(|name| (*name).to_owned()));
    }
    tiers
}
fn binding<'a>(app: &'a App, field: &str, default: &'a str) -> &'a str {
    app.ui[field].as_str().unwrap_or(default)
}
fn parse_binding(spec: &str) -> Option<(KeyCode, KeyModifiers)> {
    let mut modifiers = KeyModifiers::NONE;
    let mut pieces = spec.split('+').map(str::trim).collect::<Vec<_>>();
    let key = pieces.pop()?.to_ascii_lowercase();
    for modifier in pieces {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            _ => return None,
        };
    }
    let code = match key.as_str() {
        "enter" | "return" => KeyCode::Enter,
        "tab" => KeyCode::Tab,
        "esc" | "escape" => KeyCode::Esc,
        "space" => KeyCode::Char(' '),
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        _ if key.starts_with('f')
            && key[1..].parse::<u8>().is_ok_and(|n| (1..=24).contains(&n)) =>
        {
            KeyCode::F(key[1..].parse().ok()?)
        }
        _ if key.chars().count() == 1 => KeyCode::Char(key.chars().next()?),
        _ => return None,
    };
    Some((code, modifiers))
}
fn key_matches(event: KeyEvent, spec: &str) -> bool {
    let Some((code, modifiers)) = parse_binding(spec) else {
        return false;
    };
    let actual = match event.code {
        KeyCode::Char(character) => KeyCode::Char(character.to_ascii_lowercase()),
        other => other,
    };
    actual == code && event.modifiers == modifiers
}
fn palette(theme: &str) -> (Color, Color, Color) {
    match theme {
        "light" => (
            Color::Rgb(49, 91, 38),
            Color::Rgb(69, 80, 91),
            Color::Rgb(244, 246, 239),
        ),
        "contrast" => (Color::Yellow, Color::White, Color::Black),
        _ => (ACCENT, MUTED, BACKGROUND),
    }
}
fn role_label(role: &str) -> &str {
    match role {
        "user" => "用户",
        "assistant" => "助手",
        "system" => "系统",
        "tool" => "工具",
        "error" => "错误",
        _ => role,
    }
}
fn reset_draft(app: &mut App) {
    app.transcript_version += 1;
    app.session_id = None;
    app.messages.clear();
    app.progress = Value::Null;
    app.scroll = 0;
    app.message_count = 0;
    app.status = "新会话".into();
}
fn spawn_control(
    client: reqwest::Client,
    url: String,
    path: String,
    body: Value,
    put: bool,
    sender: mpsc::Sender<Update>,
    session_version: Option<u64>,
) {
    tokio::spawn(async move {
        let result = async {
            let request = if put {
                client.put(format!("{url}{path}"))
            } else {
                client.post(format!("{url}{path}"))
            };
            let response = request.json(&body).send().await?;
            let status = response.status();
            let value: Value = response.json().await?;
            anyhow::ensure!(
                status.is_success(),
                "{}",
                value["error"].as_str().unwrap_or("操作失败")
            );
            Ok::<Value, anyhow::Error>(value)
        }
        .await;
        let update = match result {
            Ok(value) => session_version.map_or_else(
                || Update::Notice("操作已提交".into()),
                |version| Update::Session { value, version },
            ),
            Err(error) => Update::Error(error.to_string()),
        };
        let _ = sender.send(update).await;
    });
}
fn submit_input(app: &mut App, client: &reqwest::Client, url: &str, sender: &mpsc::Sender<Update>) {
    if app.input.trim().is_empty() {
        return;
    }
    let prompt = std::mem::take(&mut app.input);
    if prompt.starts_with('/') {
        let (command, argument) = prompt.split_once(' ').unwrap_or((&prompt, ""));
        match command {
            "/help" => app.messages.push(("system".into(),"命令：/steer 内容；/follow-up 内容；/fork [消息截止数] [标题]；/theme dark|light|contrast；/bind submit|new|cancel 快捷键。F7 切换工作区。".into())),
            "/steer"|"/follow-up" => {
                if let Some(id)=&app.active {
                    if argument.trim().is_empty(){app.status="引导内容不能为空".into();return;}
                    let action=command.trim_start_matches('/');
                    spawn_control(client.clone(),url.into(),format!("/api/tasks/{id}/{action}"),json!({"prompt":argument}),false,sender.clone(),None);
                }else{app.status="没有正在运行的任务".into();}
            }
            "/fork" => {
                if app.active.is_some()||app.submitting{app.status="请等待当前任务结束后创建分支".into();return;}
                if let Some(id)=&app.session_id {
                    let mut body=json!({});let mut parts=argument.split_whitespace();
                    if let Some(cutoff)=parts.next(){
                        let Ok(cutoff)=cutoff.parse::<usize>() else{app.status="分支截止数必须为正整数".into();return;};
                        body["cutoff"]=json!(cutoff);let title=parts.collect::<Vec<_>>().join(" ");if !title.is_empty(){body["title"]=json!(title);}
                    }
                    app.transcript_version+=1;
                    spawn_control(client.clone(),url.into(),format!("/api/sessions/{id}/fork"),body,false,sender.clone(),Some(app.transcript_version));
                }else{app.status="请先选择已有会话".into();}
            }
            "/theme" => {
                if !["dark","light","contrast"].contains(&argument.trim()){app.status="主题可选 dark、light、contrast".into();return;}
                let mut ui=app.ui.clone();ui["theme"]=json!(argument.trim());
                spawn_control(client.clone(),url.into(),"/api/ui".into(),ui,true,sender.clone(),None);
            }
            "/bind" => {
                let Some((action,key))=argument.split_once(' ') else{app.status="格式：/bind submit|new|cancel Ctrl+Enter".into();return;};
                let field=match action{"submit"=>"submit_key","new"=>"new_session_key","cancel"=>"cancel_key",_=>{app.status="快捷键动作可选 submit/new/cancel".into();return;}};
                if parse_binding(key.trim()).is_none(){app.status="快捷键格式无效".into();return;}
                let mut ui=app.ui.clone();ui[field]=json!(key.trim());
                spawn_control(client.clone(),url.into(),"/api/ui".into(),ui,true,sender.clone(),None);
            }
            _=>{app.status="未知命令，使用 /help 查看帮助".into();app.input=prompt;}
        }
        return;
    }
    if app.active.is_some() || app.submitting {
        app.input = prompt;
        app.status = "任务正在执行，请使用 /steer 或 /follow-up".into();
        return;
    }
    app.transcript_version += 1;
    app.messages.push(("user".into(), prompt.clone()));
    app.submitting = true;
    app.status = "正在提交任务…".into();
    let model = app
        .selected_model
        .checked_sub(1)
        .and_then(|i| app.models.get(i))
        .and_then(|m| m["id"].as_str());
    let skills = app
        .selected_skill
        .checked_sub(1)
        .and_then(|i| app.skills.get(i))
        .and_then(|s| s["name"].as_str().or_else(|| s["id"].as_str()))
        .map(|s| vec![s.trim_start_matches("skill:").to_owned()])
        .unwrap_or_default();
    let tiers = tier_names(app);
    let tier = if app.tier == 0 {
        None
    } else {
        tiers.get(app.tier).map(String::as_str)
    };
    let request = json!({"session_id":app.session_id,"prompt":prompt,"profile":{"complexity":4,"intelligence_tier":tier,"model":model},"skills":skills});
    spawn_task(client.clone(), url.into(), request, sender.clone());
}

fn spawn_resources(
    client: &reqwest::Client,
    url: &str,
    sender: &mpsc::Sender<Update>,
) -> JoinHandle<()> {
    let resource_client = client.clone();
    let resource_url = url.to_owned();
    let resource_sender = sender.clone();
    tokio::spawn(async move {
        loop {
            let result = async {
                let (state, models, sessions, objects, skills, ui, workspaces, network) = tokio::try_join!(
                    api::fetch(&resource_client, &resource_url, "/api/state"),
                    api::fetch(&resource_client, &resource_url, "/api/models"),
                    api::fetch(&resource_client, &resource_url, "/api/sessions"),
                    api::fetch(&resource_client, &resource_url, "/api/objects"),
                    api::fetch(&resource_client, &resource_url, "/api/skills"),
                    api::fetch(&resource_client, &resource_url, "/api/ui"),
                    api::fetch(&resource_client, &resource_url, "/api/workspaces"),
                    api::fetch(&resource_client, &resource_url, "/api/network"),
                )?;
                Ok::<_, anyhow::Error>(Update::Resources {
                    state,
                    models: array(models),
                    sessions: array(sessions),
                    objects: array(objects),
                    skills: array(skills),
                    ui,
                    workspaces: array(workspaces),
                    network,
                })
            }
            .await;
            let update = match result {
                Ok(update) => update,
                Err(error) => Update::Error(format!("连接异常：{error}")),
            };
            if resource_sender.send(update).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
    })
}

fn spawn_streams(client: &reqwest::Client, url: &str, sender: &mpsc::Sender<Update>) -> Streams {
    let sse = api::sse_client().expect("SSE 客户端构建失败");
    Streams {
        poller: spawn_resources(client, url, sender),
        live: api::spawn_live_sse(sse.clone(), url.to_owned(), sender.clone()),
        trace: api::spawn_trace_sse(sse, url.to_owned(), sender.clone()),
    }
}

pub async fn run(url: &str) -> Result<()> {
    let mut url = url.trim_end_matches('/').to_owned();
    let root_url = url.clone();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    api::fetch(&client, &url, "/api/health")
        .await
        .context("无法连接运行时，请先启动 samho serve")?;
    enable_raw_mode()?;
    let guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    terminal.clear()?;
    let (sender, mut receiver) = mpsc::channel::<Update>(128);
    let mut app = App {
        status: format!("已连接 {url}"),
        ..App::default()
    };
    let mut streams = spawn_streams(&client, &url, &sender);
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    let result = async {
        loop {
            tick.tick().await;
            while let Ok(update) = receiver.try_recv() {
                apply_update(&mut app, update);
            }
            refresh_markdown_cache(&mut app, transcript_width(&terminal));
            terminal.draw(|frame| render(frame, &app))?;
            while event::poll(Duration::ZERO)? {
                let Event::Key(key) = event::read()? else {
                    continue;
                };
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('q') {
                    return Ok(());
                }
                if key_matches(key, binding(&app, "submit_key", "Ctrl+Enter"))
                    || (key.code == KeyCode::Enter
                        && key.modifiers == KeyModifiers::NONE
                        && binding(&app, "submit_key", "Ctrl+Enter") == "Ctrl+Enter")
                {
                    submit_input(&mut app, &client, &url, &sender);
                    continue;
                }
                if key_matches(key, binding(&app, "new_session_key", "Alt+N")) {
                    if !app.submitting && app.active.is_none() {
                        reset_draft(&mut app);
                    }
                    continue;
                }
                let key = if key_matches(key, binding(&app, "cancel_key", "Ctrl+K")) {
                    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
                } else {
                    key
                };
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                match key.code {
                    KeyCode::Char('q') if ctrl => return Ok::<_, anyhow::Error>(()),
                    KeyCode::Char('c') if ctrl => {
                        if let Some(active) = &app.active {
                            let client = client.clone();
                            let url = url.clone();
                            let id = active.clone();
                            let sender = sender.clone();
                            tokio::spawn(async move {
                                match client
                                    .post(format!("{url}/api/tasks/{id}/cancel"))
                                    .send()
                                    .await
                                {
                                    Ok(response) if response.status().is_success() => {}
                                    Ok(response) => {
                                        let _ = sender
                                            .send(Update::Error(format!(
                                                "取消失败：{}",
                                                response.status()
                                            )))
                                            .await;
                                    }
                                    Err(error) => {
                                        let _ = sender
                                            .send(Update::Error(format!("取消失败：{error}")))
                                            .await;
                                    }
                                }
                            });
                            app.status = "已请求取消，等待任务结束".into();
                        } else if !app.submitting {
                            return Ok(());
                        }
                    }
                    KeyCode::F(7) if !app.submitting && app.active.is_none() => {
                        let choices = app
                            .workspaces
                            .iter()
                            .filter_map(|w| {
                                w["url"].as_str().map(|candidate| {
                                    if candidate.starts_with("http://")
                                        || candidate.starts_with("https://")
                                    {
                                        candidate.trim_end_matches('/').to_owned()
                                    } else {
                                        format!("{}{}", root_url.trim_end_matches('/'), candidate)
                                    }
                                })
                            })
                            .collect::<Vec<_>>();
                        if !choices.is_empty() {
                            let next = choices
                                .iter()
                                .position(|candidate| candidate == &url)
                                .map_or(0, |i| (i + 1) % choices.len());
                            url = choices[next].clone();
                            streams.abort();
                            app = App {
                                status: format!("切换至 {url}"),
                                ..Default::default()
                            };
                            streams = spawn_streams(&client, &url, &sender);
                        }
                    }
                    KeyCode::F(2) => app.tier = (app.tier + 1) % tier_names(&app).len(),
                    KeyCode::F(3) => {
                        app.selected_model = (app.selected_model + 1) % (app.models.len() + 1)
                    }
                    KeyCode::F(6) => {
                        app.selected_skill = (app.selected_skill + 1) % (app.skills.len() + 1)
                    }
                    KeyCode::Tab => app.panel = (app.panel + 1) % PANELS.len(),
                    KeyCode::F(5) if !app.submitting && app.active.is_none() => {
                        reset_draft(&mut app)
                    }
                    KeyCode::F(4) if !app.submitting && app.active.is_none() => {
                        let next = app
                            .sessions
                            .iter()
                            .position(|session| id(session) == app.session_id.as_deref())
                            .map_or(0, |index| (index + 1) % app.sessions.len().max(1));
                        if let Some(session_id) =
                            app.sessions.get(next).and_then(id).map(str::to_owned)
                        {
                            app.transcript_version += 1;
                            let version = app.transcript_version;
                            let client = client.clone();
                            let url = url.clone();
                            let sender = sender.clone();
                            tokio::spawn(async move {
                                let update = match api::fetch(
                                    &client,
                                    &url,
                                    &format!("/api/sessions/{session_id}"),
                                )
                                .await
                                {
                                    Ok(value) => Update::Session { value, version },
                                    Err(error) => Update::Error(error.to_string()),
                                };
                                let _ = sender.send(update).await;
                            });
                        }
                    }
                    KeyCode::PageUp => app.scroll = app.scroll.saturating_add(8),
                    KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(8),
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        app.input.push('\n')
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Char(character)
                        if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        app.input.push(character)
                    }
                    _ => {}
                }
            }
        }
    }
    .await;
    streams.abort();
    drop(terminal);
    drop(guard);
    result
}

/// 对话中列的可用文本宽度（与 render 的布局约束保持一致）。
fn transcript_width(terminal: &Terminal<CrosstermBackend<io::Stdout>>) -> u16 {
    let size = terminal.size().unwrap_or_default();
    let area = Rect::new(0, 0, size.width, size.height);
    let columns = Layout::horizontal([
        Constraint::Percentage(22),
        Constraint::Percentage(51),
        Constraint::Percentage(27),
    ])
    .split(area);
    columns[1].width.saturating_sub(2).max(1)
}

/// 按当前宽度与原文重建 Markdown 缓存（原文未变则复用；流式增量整体重渲，
/// 与 web 前端「按 revision 整体替换」的语义一致）。
fn refresh_markdown_cache(app: &mut App, width: u16) {
    let md = markdown_palette(app);
    if let Some(text) = app.progress["text"].as_str() {
        let rebuild = app
            .progress_md
            .as_ref()
            .is_none_or(|(cached_width, cached_text, _)| {
                *cached_width != width || cached_text != text
            });
        if rebuild {
            let lines = markdown::render(text, &md, width);
            app.progress_md = Some((width, text.to_owned(), lines));
        }
    } else {
        app.progress_md = None;
    }
    if let Some((_, content)) = app
        .messages
        .iter()
        .rev()
        .find(|(role, _)| role == "assistant")
    {
        let rebuild = app
            .assistant_md
            .as_ref()
            .is_none_or(|(cached_width, cached_text, _)| {
                *cached_width != width || cached_text != content
            });
        if rebuild {
            let lines = markdown::render(content, &md, width);
            app.assistant_md = Some((width, content.clone(), lines));
        }
    } else {
        app.assistant_md = None;
    }
}

fn markdown_palette(app: &App) -> markdown::MarkdownPalette {
    markdown::MarkdownPalette::from_theme(app.ui["theme"].as_str().unwrap_or("dark"))
}

fn spawn_task(client: reqwest::Client, url: String, request: Value, sender: mpsc::Sender<Update>) {
    tokio::spawn(async move {
        let result = async {
            let response = client
                .post(format!("{url}/api/run"))
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            let value: Value = response.json().await?;
            anyhow::ensure!(
                status.is_success(),
                "{}",
                value["error"].as_str().unwrap_or("任务提交失败")
            );
            let id = value["task_id"].as_str().context("提交响应缺少 task_id")?;
            sender.send(Update::Submitted(id.to_owned())).await?;
            loop {
                // 兜底轮询：SSE 丢帧时仍能收尾（进度实时性由 /api/live/stream 保证）
                let response = match client.get(format!("{url}/api/tasks/{id}")).send().await {
                    Ok(response) => response,
                    Err(error) => {
                        sender
                            .send(Update::Error(format!("任务连接中断，正在重试：{error}")))
                            .await?;
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                let status = response.status();
                let value: Value = response.json().await?;
                anyhow::ensure!(
                    status.is_success(),
                    "{}",
                    value["error"].as_str().unwrap_or("任务查询失败")
                );
                if let Some(progress) = value.get("progress") {
                    sender.send(Update::Progress(progress.clone())).await?;
                }
                if !matches!(value["status"].as_str(), Some("running" | "queued")) {
                    sender.send(Update::Finished(value)).await?;
                    return Ok::<_, anyhow::Error>(());
                }
                tokio::time::sleep(Duration::from_millis(900)).await;
            }
        }
        .await;
        if let Err(error) = result {
            let _ = sender.send(Update::TaskError(error.to_string())).await;
        }
    });
}

fn apply_update(app: &mut App, update: Update) {
    match update {
        Update::Notice(text) => app.status = text,
        Update::Progress(value) => {
            app.progress = value;
        }
        Update::Submitted(id) => {
            app.submitting = false;
            app.active = Some(id);
            app.status = "Agent 正在执行，可使用配置的取消快捷键".into();
        }
        Update::Finished(value) => {
            app.progress = Value::Null;
            app.submitting = false;
            app.active = None;
            let result = &value["result"];
            if let Some(output) = result["output"].as_str() {
                app.messages.push(("assistant".into(), output.into()));
            }
            if let Some(id) = result["session_id"].as_str() {
                app.session_id = Some(id.into());
            }
            app.status = format!(
                "{} · 模型 {} · 轨迹 {}",
                value["status"].as_str().unwrap_or("已结束"),
                result["model_id"].as_str().unwrap_or("—"),
                result["trace_id"].as_str().unwrap_or("—")
            );
            if let Some(error) = value["error"].as_str() {
                app.messages.push(("error".into(), error.into()));
            }
        }
        Update::Error(error) => {
            app.status = error;
        }
        Update::TaskError(error) => {
            app.progress = Value::Null;
            app.submitting = false;
            app.active = None;
            app.status = error.clone();
            app.messages.push(("error".into(), error));
        }
        Update::TraceEvent(event) => {
            let sequence = event["sequence"].as_u64().unwrap_or(0);
            if sequence <= app.last_event_seq {
                return; // SSE 回放与轮询兜底的重复帧
            }
            app.last_event_seq = sequence;
            app.events.push_back(event);
            while app.events.len() > 300 {
                app.events.pop_front();
            }
        }
        Update::Sse { live, trace } => {
            if let Some(live) = live {
                app.live_sse = live;
            }
            if let Some(trace) = trace {
                app.trace_sse = trace;
            }
        }
        Update::Resources {
            state,
            models,
            sessions,
            objects,
            skills,
            ui,
            workspaces,
            network,
        } => {
            app.ui = ui;
            app.workspaces = workspaces;
            app.state = state;
            if app.tier >= tier_names(app).len() {
                app.tier = 0;
            }
            app.models = models;
            app.sessions = sessions;
            if let Some(id) = &app.session_id
                && let Some(session) = app
                    .sessions
                    .iter()
                    .find(|s| s["id"].as_str() == Some(id.as_str()))
            {
                app.message_count = session["message_count"].as_u64().unwrap_or(0) as usize;
            }
            app.objects = objects;
            app.skills = skills;
            let address = network["addresses"]
                .as_array()
                .and_then(|addresses| addresses.first())
                .and_then(Value::as_str)
                .unwrap_or("未检测到局域网地址");
            let port = network["port"].as_u64().unwrap_or(7432);
            app.lan = format!("{address}:{port}");
            if app.selected_model > app.models.len() {
                app.selected_model = 0;
            }
            if app.selected_skill > app.skills.len() {
                app.selected_skill = 0;
            }
        }
        Update::Session {
            value: session,
            version,
        } => {
            if version != app.transcript_version || app.submitting || app.active.is_some() {
                return;
            }
            app.session_id = id(&session).map(str::to_owned);
            app.message_count = session["messages"].as_array().map_or(0, Vec::len);
            app.messages = session["messages"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|message| {
                    let content = message["content"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| message["content"].to_string());
                    (
                        message["role"].as_str().unwrap_or("message").into(),
                        content,
                    )
                })
                .collect();
            app.scroll = 0;
            app.status = "已从共享服务恢复会话".into();
        }
    }
}

fn render(frame: &mut Frame, app: &App) {
    let (accent, muted, background) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(background)),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(area);
    let simulated = app.state["simulation"].as_bool().unwrap_or(false)
        || (!app.models.is_empty()
            && app
                .models
                .iter()
                .all(|model| model["provider"] == "demo" || model["provider"] == "mock"));
    let model = app
        .selected_model
        .checked_sub(1)
        .and_then(|index| app.models.get(index))
        .and_then(|model| model["id"].as_str())
        .unwrap_or("自动");
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Samho / 中文对象工作台 ",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  层级:{}  模型:{}  {}",
                tier_names(app)
                    .get(app.tier)
                    .map(String::as_str)
                    .unwrap_or("auto"),
                model,
                if simulated {
                    "模拟模式"
                } else {
                    "已配置模型服务"
                }
            )),
        ]))
        .block(Block::default().borders(Borders::BOTTOM)),
        rows[0],
    );
    let columns = Layout::horizontal([
        Constraint::Percentage(22),
        Constraint::Percentage(51),
        Constraint::Percentage(27),
    ])
    .split(rows[1]);
    let mut session_lines = vec![
        Line::styled("F4 切换 / F5 新建", Style::default().fg(muted)),
        Line::raw(""),
    ];
    for session in &app.sessions {
        let session_id = id(session).unwrap_or("unknown");
        let title = session
            .get("title")
            .or_else(|| session.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(session_id);
        let current = Some(session_id) == app.session_id.as_deref();
        session_lines.push(Line::styled(
            format!("{} {title}", if current { "▸" } else { " " }),
            Style::default().fg(if current { accent } else { muted }),
        ));
    }
    if app.sessions.is_empty() {
        session_lines.push(Line::raw("暂无会话"));
    }
    frame.render_widget(
        Paragraph::new(session_lines)
            .block(Block::default().title(" 会话 ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        columns[0],
    );
    let mut transcript = vec![
        Line::styled(
            "共享 Rust 服务 · SSE 流式 · Markdown 渲染",
            Style::default().fg(muted),
        ),
        Line::raw(""),
    ];
    for (role, content) in &app.messages {
        transcript.push(Line::styled(
            role_label(role),
            Style::default()
                .fg(if role == "user" {
                    accent
                } else if role == "error" {
                    Color::Red
                } else {
                    Color::Cyan
                })
                .add_modifier(Modifier::BOLD),
        ));
        // 助手消息走 Markdown 渲染；其余角色保持原文
        if role == "assistant" {
            match &app.assistant_md {
                Some((_, cached_text, lines)) if cached_text == content => {
                    transcript.extend(lines.iter().cloned());
                }
                _ => transcript.extend(content.lines().map(|line| Line::raw(line.to_owned()))),
            }
        } else {
            transcript.extend(content.lines().map(|line| Line::raw(line.to_owned())));
        }
        transcript.push(Line::raw(""));
    }
    if !app.progress.is_null() {
        transcript.push(Line::styled(
            "正在生成（累计脱敏快照）",
            Style::default().fg(accent),
        ));
        if let Some(reasoning) = app.progress["reasoning"].as_str() {
            transcript.extend(
                reasoning
                    .lines()
                    .map(|line| Line::styled(line.to_owned(), Style::default().fg(muted))),
            );
        }
        if let Some(text) = app.progress["text"].as_str() {
            match &app.progress_md {
                Some((_, cached_text, lines)) if cached_text == text => {
                    transcript.extend(lines.iter().cloned());
                }
                _ => transcript.extend(text.lines().map(|line| Line::raw(line.to_owned()))),
            }
        }
        if let Some(calls) = app.progress["tool_calls"].as_array()
            && !calls.is_empty()
        {
            transcript.push(Line::styled(
                format!("🔧 工具调用 ×{}", calls.len()),
                Style::default().fg(muted),
            ));
        }
    }
    if app.messages.is_empty() {
        transcript.push(Line::raw(
            "输入任务，使用 F2 选择层级、F3 固定模型。/help 查看命令。",
        ));
    }
    let width = usize::from(columns[1].width.saturating_sub(2)).max(1);
    let lines = transcript
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>();
    let scroll = lines
        .saturating_sub(usize::from(columns[1].height.saturating_sub(2)))
        .saturating_sub(usize::from(app.scroll))
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(
        Paragraph::new(transcript)
            .block(Block::default().title(" 对话 ").borders(Borders::ALL))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        columns[1],
    );
    let details = match app.panel {
        0 => Value::Array(app.events.iter().rev().take(16).cloned().collect()),
        1 => json!(app.objects),
        2 => json!(app.skills),
        _ => Value::Array(
            app.objects
                .iter()
                .filter(|object| {
                    object["kind"]
                        .as_str()
                        .is_some_and(|kind| kind.contains("mcp"))
                })
                .cloned()
                .collect(),
        ),
    };
    frame.render_widget(
        Paragraph::new(serde_json::to_string_pretty(&details).unwrap_or_default())
            .block(
                Block::default()
                    .title(format!(" {} · Tab ", PANELS[app.panel]))
                    .borders(Borders::ALL),
            )
            .style(Style::default().fg(muted))
            .wrap(Wrap { trim: false }),
        columns[2],
    );
    let skill = app
        .selected_skill
        .checked_sub(1)
        .and_then(|index| app.skills.get(index))
        .and_then(|skill| skill["name"].as_str().or_else(|| skill["id"].as_str()))
        .unwrap_or("无");
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(
                Block::default()
                    .title(format!(" 任务 · 提交见快捷键 · 技能:{skill} "))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(accent)),
            )
            .wrap(Wrap { trim: false }),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(app.status.as_str()).style(Style::default().fg(accent)),
        rows[3],
    );
    let dot = |on: bool| {
        Span::styled(
            if on { "●" } else { "○" },
            Style::default().fg(if on { Color::Green } else { Color::Red }),
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("流 "),
            dot(app.live_sse),
            dot(app.trace_sse),
            Span::raw(format!("  LAN {}  ", app.lan)),
            Span::styled(
                format!(
                    "Ctrl+Q 退出  {} 提交  {} 新建  {} 取消  F2 层级 F3 模型 F4 会话 F6 技能 F7 工作区 Tab 检查",
                    binding(app, "submit_key", "Ctrl+Enter"),
                    binding(app, "new_session_key", "Alt+N"),
                    binding(app, "cancel_key", "Ctrl+K")
                ),
                Style::default().fg(muted),
            ),
        ])),
        rows[4],
    );
    if rows[2].width > 2 && rows[2].height > 2 {
        let input_width = usize::from(rows[2].width - 2);
        let cursor = Line::raw(app.input.lines().last().unwrap_or(""))
            .width()
            .min(input_width.saturating_sub(1)) as u16;
        frame.set_cursor_position((rows[2].x + 1 + cursor, rows[2].y + 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn layout_displays_session_routing_simulation_and_trace() {
        let mut terminal = Terminal::new(TestBackend::new(140, 35)).unwrap();
        let app = App {
            messages: vec![
                ("user".into(), "Inspect the object graph".into()),
                ("assistant".into(), "Simulation only".into()),
            ],
            models: vec![json!({"id":"demo","provider":"mock"})],
            state: json!({"simulation":true}),
            status: "Connected".into(),
            ..App::default()
        };
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let content: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        for expected in [
            "中文对象工作台",
            "模拟模式",
            "会话",
            "对话",
            "轨迹",
            "Inspect the object graph",
            "Ctrl+Q",
        ] {
            assert!(
                content
                    .replace(' ', "")
                    .contains(&expected.replace(' ', "")),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn tiny_terminal_and_unicode_do_not_panic() {
        let mut terminal = Terminal::new(TestBackend::new(24, 8)).unwrap();
        let app = App {
            input: "中文 Agent 🦀".into(),
            ..App::default()
        };
        terminal.draw(|frame| render(frame, &app)).unwrap();
    }

    #[test]
    fn late_session_response_does_not_replace_new_draft_or_running_task() {
        let mut app = App {
            transcript_version: 2,
            messages: vec![("user".into(), "new draft".into())],
            ..App::default()
        };
        apply_update(
            &mut app,
            Update::Session {
                value: json!({"id":"old-session","messages":[{"role":"user","content":"old transcript"}]}),
                version: 1,
            },
        );
        assert!(app.session_id.is_none());
        assert_eq!(app.messages[0].1, "new draft");
        app.submitting = true;
        apply_update(
            &mut app,
            Update::Session {
                value: json!({"id":"old-session","messages":[]}),
                version: 2,
            },
        );
        apply_update(
            &mut app,
            Update::Error("temporary resource connection failure".into()),
        );
        assert!(app.submitting);
        assert_eq!(app.messages[0].1, "new draft");
    }

    #[test]
    fn configured_keybindings_and_dynamic_tiers_are_not_hardcoded() {
        assert!(key_matches(
            KeyEvent::new(
                KeyCode::Char('P'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT
            ),
            "Ctrl+Shift+P"
        ));
        assert!(key_matches(
            KeyEvent::new(KeyCode::F(8), KeyModifiers::ALT),
            "Alt+F8"
        ));
        assert!(!key_matches(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            "Ctrl+Enter"
        ));
        assert!(parse_binding("unknown+K").is_none());
        let app = App {
            state: json!({"tiers":[{"id":"tiny"},{"id":"architect"}]}),
            ui: json!({"submit_key":"Alt+P"}),
            ..Default::default()
        };
        assert_eq!(tier_names(&app), ["auto", "tiny", "architect"]);
        assert_eq!(binding(&app, "submit_key", "Ctrl+Enter"), "Alt+P");
        assert_ne!(palette("light").2, palette("dark").2);
    }
    #[test]
    fn cumulative_progress_replaces_previous_snapshot_and_clears_on_completion() {
        let mut app = App::default();
        apply_update(
            &mut app,
            Update::Progress(json!({"text":"第一段","revision":1})),
        );
        apply_update(
            &mut app,
            Update::Progress(json!({"text":"第一段第二段","revision":2})),
        );
        assert_eq!(app.progress["text"], "第一段第二段");
        apply_update(
            &mut app,
            Update::Finished(
                json!({"status":"complete","result":{"output":"最终答复","session_id":"s"}}),
            ),
        );
        assert!(app.progress.is_null());
        assert_eq!(app.messages.len(), 1);
    }
    #[test]
    fn cjk_combining_emoji_resize_and_light_theme_render_without_cell_overflow() {
        for (width, height) in [(12, 6), (24, 8), (80, 24), (160, 48)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let app = App {
                input: "中文e\u{301}🦀宽字符".into(),
                ui: json!({"theme":"light"}),
                progress: json!({"text":"正在生成中文 🦀"}),
                ..Default::default()
            };
            terminal.draw(|frame| render(frame, &app)).unwrap();
            assert_eq!(
                terminal.backend().buffer().content.len(),
                usize::from(width) * usize::from(height)
            );
        }
    }

    #[test]
    fn sse_trace_events_dedupe_by_sequence_and_resource_update_shows_lan() {
        let mut app = App::default();
        for sequence in [1u64, 2, 3, 2, 1] {
            apply_update(
                &mut app,
                Update::TraceEvent(json!({"sequence":sequence,"kind":"tool.start"})),
            );
        }
        assert_eq!(app.events.len(), 3);
        assert_eq!(app.last_event_seq, 3);
        apply_update(
            &mut app,
            Update::Resources {
                state: json!({}),
                models: vec![],
                sessions: vec![],
                objects: vec![],
                skills: vec![],
                ui: json!({}),
                workspaces: vec![],
                network: json!({"addresses":["192.168.1.8","10.0.0.2"],"port":7432}),
            },
        );
        assert_eq!(app.lan, "192.168.1.8:7432");
        apply_update(
            &mut app,
            Update::Sse {
                live: Some(true),
                trace: Some(false),
            },
        );
        assert!(app.live_sse);
        assert!(!app.trace_sse);
    }

    #[test]
    fn assistant_markdown_cache_rebuilds_on_content_or_width_change() {
        let mut app = App {
            messages: vec![("assistant".into(), "# 标题\n\n- 甲".into())],
            ..Default::default()
        };
        refresh_markdown_cache(&mut app, 60);
        let lines = app.assistant_md.as_ref().unwrap();
        assert_eq!(lines.0, 60);
        assert!(
            lines
                .2
                .iter()
                .any(|line| line.spans.iter().any(|span| span.content.contains("标题")))
        );
        refresh_markdown_cache(&mut app, 40);
        assert_eq!(app.assistant_md.as_ref().unwrap().0, 40);
        app.progress = json!({"text":"**加粗**流式","revision":3});
        refresh_markdown_cache(&mut app, 60);
        let progress = app.progress_md.as_ref().unwrap();
        assert!(progress.2.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.contains("加粗")
                    && span.style.has_modifier(ratatui::style::Modifier::BOLD)
            })
        }));
    }
}
