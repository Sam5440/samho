//! TUI：`samho tui --url` 连接运行中的 `samho serve`（纯 HTTP/SSE 客户端）。
//!
//! 模块划分：`api` HTTP/SSE 封装、`sse` SSE 解析器、`markdown` Markdown
//! 渲染器、`widgets` 共享小助手、`views` 视图与浮层。数据契约与 web 前端
//! 一致：进度为 TaskProgress 累计快照（按 revision 整体替换），轨迹为
//! TraceEvent。M2 起支持项目侧栏、工具卡、任务看板与模态浮层。

mod api;
mod markdown;
mod sse;
mod views;
mod widgets;

use std::collections::{BTreeSet, VecDeque};
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
    widgets::{Block, Borders, Paragraph},
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

/// 顶层视图（M4 起扩展文件/Git/终端/浏览器）。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum View {
    #[default]
    Main,
    Tasks,
}

/// 工具卡状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolStatus {
    Running,
    Ok,
    Error,
}

/// 由 tool.start/tool.result 按 call_id 配对得到的工具卡片。
struct ToolCard {
    call_id: String,
    name: String,
    args: String,
    status: ToolStatus,
    latency_ms: Option<u64>,
    result: Option<String>,
}

/// 浮层动作（菜单项确认后分发）。
#[derive(Clone)]
enum Action {
    OpenSession(String),
    RenameSession(String),
    ForkSession(String),
    ArchiveSession(String, bool),
    MoveSession(String),
    AssignSession(String, Option<String>),
    DeleteSession(String),
    DeleteSessionNow(String),
    RenameProject(String),
    ToggleProject(String, bool),
    DeleteProject(String),
    DeleteProjectNow(String),
    PickModel(usize),
}

/// 输入浮层的提交动作。
#[derive(Clone)]
enum PromptAction {
    NewProject,
    RenameSession(String),
    RenameProject(String),
}

/// 模态浮层（打开时独占键位）。
enum Overlay {
    /// 单行输入
    Prompt {
        title: String,
        input: String,
        action: PromptAction,
    },
    /// 确认（Enter 执行 accept）
    Confirm { title: String, accept: Action },
    /// 单选菜单
    Menu {
        title: String,
        items: Vec<(String, Action)>,
        cursor: usize,
    },
    /// 技能多选
    Skills { cursor: usize },
}

#[derive(Default)]
struct App {
    input: String,
    messages: Vec<(String, String)>,
    status: String,
    models: Vec<Value>,
    sessions: Vec<Value>,
    projects: Vec<Value>,
    tasks: Vec<Value>,
    objects: Vec<Value>,
    skills: Vec<Value>,
    events: VecDeque<Value>,
    state: Value,
    active: Option<String>,
    submitting: bool,
    session_id: Option<String>,
    selected_model: usize,
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
    /// (宽度, 原文, 渲染结果) —— 流式思考正文（muted 着色）
    reasoning_md: Option<(u16, String, Vec<Line<'static>>)>,
    // M2：视图/焦点/侧栏/看板/浮层/工具卡/composer
    view: View,
    focus_composer: bool,
    sidebar_cursor: usize,
    sidebar_filter: String,
    sidebar_search: bool,
    show_archived: bool,
    tasks_cursor: usize,
    pending_project_id: Option<String>,
    selected_skills: BTreeSet<String>,
    followup_mode: bool,
    complexity: u32,
    reasoning_collapsed: bool,
    overlay: Option<Overlay>,
    tool_cards: Vec<ToolCard>,
    ui_tick: u64,
}

/// 资源轮询快照（字段较多，装箱避免 Update 枚举尺寸失衡）。
struct ResourceSnapshot {
    state: Value,
    models: Vec<Value>,
    sessions: Vec<Value>,
    projects: Vec<Value>,
    tasks: Vec<Value>,
    objects: Vec<Value>,
    skills: Vec<Value>,
    ui: Value,
    workspaces: Vec<Value>,
    network: Value,
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
    Resources(Box<ResourceSnapshot>),
    Session {
        value: Value,
        version: u64,
    },
}

/// 键位处理所需的请求上下文（client/url/回程通道），由事件循环临时借用。
struct Ctx<'a> {
    client: &'a reqwest::Client,
    url: &'a str,
    sender: &'a mpsc::Sender<Update>,
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
    app.pending_project_id = None;
    app.tool_cards.clear();
    app.status = "新会话".into();
}

/// 通用控制请求：POST/PUT/PATCH/DELETE。`session_version` 给定时把响应
/// 当作会话内容应用（fork 场景），否则只弹通知。
fn spawn_request(
    ctx: &Ctx,
    method: &str,
    path: &str,
    body: Option<Value>,
    session_version: Option<u64>,
) {
    let client = ctx.client.clone();
    let url = ctx.url.to_owned();
    let sender = ctx.sender.clone();
    let method = method.to_ascii_uppercase();
    let path = path.to_owned();
    tokio::spawn(async move {
        let result = async {
            let base = format!("{url}{path}");
            let request = match method.as_str() {
                "PUT" => client.put(&base),
                "PATCH" => client.patch(&base),
                "DELETE" => client.delete(&base),
                _ => client.post(&base),
            };
            let request = if let Some(body) = &body {
                request.json(body)
            } else {
                request
            };
            let response = request.send().await?;
            let status = response.status();
            let value: Value = response.json().await.unwrap_or(Value::Null);
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

/// 取消运行中任务（Ctrl+C / 任务看板 s 共用）。
fn cancel_task(ctx: &Ctx, task_id: &str) {
    spawn_request(
        ctx,
        "POST",
        &format!("/api/tasks/{task_id}/cancel"),
        Some(json!({})),
        None,
    );
}

/// 打开一个会话：拉取转录并切回主视图；清空进度/工具卡等运行态。
fn open_session(app: &mut App, ctx: &Ctx, session_id: &str) {
    app.transcript_version += 1;
    let version = app.transcript_version;
    app.session_id = None; // 等响应到达再设置，避免陈旧转录
    app.progress = Value::Null;
    app.progress_md = None;
    app.reasoning_md = None;
    app.tool_cards.clear();
    app.pending_project_id = None;
    app.view = View::Main;
    app.focus_composer = true;
    app.scroll = 0;
    app.status = "正在加载会话…".into();
    let client = ctx.client.clone();
    let url = ctx.url.to_owned();
    let sender = ctx.sender.clone();
    let path = format!("/api/sessions/{session_id}");
    tokio::spawn(async move {
        let update = match api::fetch(&client, &url, &path).await {
            Ok(value) => Update::Session { value, version },
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
            "/help" => app.messages.push(("system".into(),"命令：/steer 内容；/follow-up 内容；/fork [消息截止数] [标题]；/theme dark|light|contrast；/bind submit|new|cancel 快捷键；/auto 自动层级；/complexity 0-10 复杂度。快捷键：Ctrl+M 模型、F6 技能多选、Ctrl+R 思考折叠、Ctrl+F 引导模式、Shift+Tab 侧栏/输入、Alt+2 任务看板。".into())),
            "/steer"|"/follow-up" => {
                if let Some(id)=&app.active {
                    if argument.trim().is_empty(){app.status="引导内容不能为空".into();return;}
                    let action=command.trim_start_matches('/');
                    let ctx=Ctx{client,url,sender};
                    spawn_request(&ctx,"POST",&format!("/api/tasks/{id}/{action}"),Some(json!({"prompt":argument})),None);
                    app.status="引导已提交".into();
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
                    let ctx=Ctx{client,url,sender};
                    spawn_request(&ctx,"POST",&format!("/api/sessions/{id}/fork"),Some(body),Some(app.transcript_version));
                }else{app.status="请先选择已有会话".into();}
            }
            "/theme" => {
                if !["dark","light","contrast"].contains(&argument.trim()){app.status="主题可选 dark、light、contrast".into();return;}
                let mut ui=app.ui.clone();ui["theme"]=json!(argument.trim());
                let ctx=Ctx{client,url,sender};
                spawn_request(&ctx,"PUT","/api/ui",Some(ui),None);
            }
            "/bind" => {
                let Some((action,key))=argument.split_once(' ') else{app.status="格式：/bind submit|new|cancel Ctrl+Enter".into();return;};
                let field=match action{"submit"=>"submit_key","new"=>"new_session_key","cancel"=>"cancel_key",_=>{app.status="快捷键动作可选 submit/new/cancel".into();return;}};
                if parse_binding(key.trim()).is_none(){app.status="快捷键格式无效".into();return;}
                let mut ui=app.ui.clone();ui[field]=json!(key.trim());
                let ctx=Ctx{client,url,sender};
                spawn_request(&ctx,"PUT","/api/ui",Some(ui),None);
            }
            "/auto" => {
                app.tier = 0;
                app.status = "已切换为自动层级（auto）".into();
            }
            "/complexity" => {
                match argument.trim().parse::<u32>() {
                    Ok(n) if n <= 10 => {
                        app.complexity = n;
                        app.status = format!("复杂度已设为 {n}");
                    }
                    _ => app.status = "复杂度取值 0-10".into(),
                }
            }
            _=>{app.status="未知命令，使用 /help 查看帮助".into();app.input=prompt;}
        }
        return;
    }
    if let Some(active) = app.active.clone() {
        // 运行中：输入即引导（Ctrl+F 切换 steer/follow-up 语义）
        let action = if app.followup_mode {
            "follow-up"
        } else {
            "steer"
        };
        let ctx = Ctx {
            client,
            url,
            sender,
        };
        spawn_request(
            &ctx,
            "POST",
            &format!("/api/tasks/{active}/{action}"),
            Some(json!({"prompt": prompt})),
            None,
        );
        app.status = if app.followup_mode {
            "引导已排队（follow-up，任务结束后执行）".into()
        } else {
            "引导已注入（steer）".into()
        };
        return;
    }
    if app.submitting {
        app.input = prompt;
        app.status = "正在提交任务，请稍候".into();
        return;
    }
    app.transcript_version += 1;
    app.messages.push(("user".into(), prompt.clone()));
    app.submitting = true;
    app.status = "正在提交任务…".into();
    app.tool_cards.clear();
    let model = app
        .selected_model
        .checked_sub(1)
        .and_then(|i| app.models.get(i))
        .and_then(|m| m["id"].as_str());
    let skills: Vec<String> = app.selected_skills.iter().cloned().collect();
    let tiers = tier_names(app);
    let tier = if app.tier == 0 {
        None
    } else {
        tiers.get(app.tier).map(String::as_str)
    };
    let complexity = app.complexity;
    let session_id = app.session_id.clone();
    let request = json!({"session_id":session_id,"prompt":prompt,"profile":{"complexity":complexity,"intelligence_tier":tier,"model":model},"skills":skills});
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
                let (
                    state,
                    models,
                    sessions,
                    projects,
                    tasks,
                    objects,
                    skills,
                    ui,
                    workspaces,
                    network,
                ) = tokio::try_join!(
                    api::fetch(&resource_client, &resource_url, "/api/state"),
                    api::fetch(&resource_client, &resource_url, "/api/models"),
                    api::fetch(&resource_client, &resource_url, "/api/sessions"),
                    api::fetch(&resource_client, &resource_url, "/api/projects"),
                    api::fetch(&resource_client, &resource_url, "/api/tasks"),
                    api::fetch(&resource_client, &resource_url, "/api/objects"),
                    api::fetch(&resource_client, &resource_url, "/api/skills"),
                    api::fetch(&resource_client, &resource_url, "/api/ui"),
                    api::fetch(&resource_client, &resource_url, "/api/workspaces"),
                    api::fetch(&resource_client, &resource_url, "/api/network"),
                )?;
                Ok::<_, anyhow::Error>(Update::Resources(Box::new(ResourceSnapshot {
                    state,
                    models: array(models),
                    sessions: array(sessions),
                    projects: array(projects),
                    tasks: tasks["tasks"].as_array().cloned().unwrap_or_default(),
                    objects: array(objects),
                    skills: array(skills),
                    ui,
                    workspaces: array(workspaces),
                    network,
                })))
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
        focus_composer: true,
        complexity: 4,
        ..App::default()
    };
    let mut streams = spawn_streams(&client, &url, &sender);
    let mut tick = tokio::time::interval(Duration::from_millis(33));
    let result = async {
        loop {
            tick.tick().await;
            app.ui_tick = app.ui_tick.wrapping_add(1);
            while let Ok(update) = receiver.try_recv() {
                let ctx = Ctx {
                    client: &client,
                    url: &url,
                    sender: &sender,
                };
                apply_update(&mut app, update, Some(&ctx));
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
                let alt = key.modifiers.contains(KeyModifiers::ALT);
                if ctrl && key.code == KeyCode::Char('q') {
                    return Ok(());
                }
                // 浮层打开时独占键位
                if app.overlay.is_some() {
                    let ctx = Ctx {
                        client: &client,
                        url: &url,
                        sender: &sender,
                    };
                    views::overlays::handle_key(&mut app, &ctx, key);
                    continue;
                }
                // 全局：视图切换、工作区切换、取消/退出
                match key.code {
                    KeyCode::Char('1') if alt && !ctrl => {
                        app.view = View::Main;
                        continue;
                    }
                    KeyCode::Char('2') if alt && !ctrl => {
                        app.view = View::Tasks;
                        continue;
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
                                focus_composer: true,
                                complexity: 4,
                                ..Default::default()
                            };
                            streams = spawn_streams(&client, &url, &sender);
                        }
                        continue;
                    }
                    KeyCode::Char('c') if ctrl => {
                        if let Some(active) = app.active.clone() {
                            let ctx = Ctx {
                                client: &client,
                                url: &url,
                                sender: &sender,
                            };
                            cancel_task(&ctx, &active);
                            app.status = "已请求取消，等待任务结束".into();
                        } else if !app.submitting {
                            return Ok(());
                        }
                        continue;
                    }
                    _ => {}
                }
                let ctx = Ctx {
                    client: &client,
                    url: &url,
                    sender: &sender,
                };
                match app.view {
                    View::Main => handle_main_key(&mut app, &ctx, key),
                    View::Tasks => views::tasks::handle_key(&mut app, &ctx, key),
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

/// 主视图键位：提交/新建绑定、视图内全局键（Tab 检查面板、F2/F3/F4/F5/F6、
/// Ctrl+R/M/F）、Shift+Tab 焦点切换，其余按键按焦点分发给侧栏或 composer。
fn handle_main_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let submit_binding = binding(app, "submit_key", "Ctrl+Enter").to_owned();
    let plain_enter_submit = app.focus_composer
        && key.code == KeyCode::Enter
        && key.modifiers == KeyModifiers::NONE
        && submit_binding == "Ctrl+Enter";
    if (app.focus_composer && key_matches(key, &submit_binding)) || plain_enter_submit {
        submit_input(app, ctx.client, ctx.url, ctx.sender);
        return;
    }
    if key_matches(key, binding(app, "new_session_key", "Alt+N")) {
        if !app.submitting && app.active.is_none() {
            reset_draft(app);
        }
        return;
    }
    let key = if key_matches(key, binding(app, "cancel_key", "Ctrl+K")) {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    } else {
        key
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        // Shift+Tab（部分终端上报 BackTab）在侧栏/输入间切换焦点
        KeyCode::BackTab | KeyCode::Tab
            if key.modifiers.contains(KeyModifiers::SHIFT) && !ctrl && !alt =>
        {
            app.focus_composer = !app.focus_composer;
        }
        KeyCode::Tab if !ctrl && !alt => app.panel = (app.panel + 1) % PANELS.len(),
        KeyCode::F(2) => app.tier = (app.tier + 1) % tier_names(app).len(),
        KeyCode::F(3) => app.selected_model = (app.selected_model + 1) % (app.models.len() + 1),
        KeyCode::F(4) if !app.submitting && app.active.is_none() => {
            let next = app
                .sessions
                .iter()
                .position(|s| s["id"].as_str() == app.session_id.as_deref())
                .map_or(0, |index| (index + 1) % app.sessions.len().max(1));
            if let Some(session_id) = app
                .sessions
                .get(next)
                .and_then(|s| s["id"].as_str())
                .map(str::to_owned)
            {
                open_session(app, ctx, &session_id);
            }
        }
        KeyCode::F(5) if !app.submitting && app.active.is_none() => reset_draft(app),
        KeyCode::F(6) => {
            if app.overlay.is_none() {
                app.overlay = Some(Overlay::Skills { cursor: 0 });
            }
        }
        KeyCode::Char('r') if ctrl && !alt => app.reasoning_collapsed = !app.reasoning_collapsed,
        KeyCode::Char('f') if ctrl && !alt => {
            app.followup_mode = !app.followup_mode;
            app.status = if app.followup_mode {
                "引导模式：follow-up（排队，任务结束后执行）"
            } else {
                "引导模式：steer（即时注入当前任务）"
            }
            .into();
        }
        KeyCode::Char('m') if ctrl && !alt => {
            if app.overlay.is_none() {
                app.overlay = Some(views::overlays::model_menu(app));
            }
        }
        KeyCode::PageUp => app.scroll = app.scroll.saturating_add(8),
        KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(8),
        _ => {
            if app.focus_composer {
                match key.code {
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        app.input.push('\n')
                    }
                    KeyCode::Backspace => {
                        app.input.pop();
                    }
                    KeyCode::Char(character) if !ctrl && !alt => app.input.push(character),
                    _ => {}
                }
            } else {
                views::sidebar::handle_key(app, ctx, key);
            }
        }
    }
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
    let muted = palette(app.ui["theme"].as_str().unwrap_or("dark")).1;
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
    if let Some(text) = app.progress["reasoning"].as_str() {
        let rebuild = app
            .reasoning_md
            .as_ref()
            .is_none_or(|(cached_width, cached_text, _)| {
                *cached_width != width || cached_text != text
            });
        if rebuild {
            // 思考正文整体降为 muted（保留加粗/斜体等结构修饰）
            let lines = markdown::render(text, &md, width)
                .into_iter()
                .map(|line| {
                    Line::from(
                        line.spans
                            .into_iter()
                            .map(|span| Span::styled(span.content, span.style.fg(muted)))
                            .collect::<Vec<_>>(),
                    )
                })
                .collect();
            app.reasoning_md = Some((width, text.to_owned(), lines));
        }
    } else {
        app.reasoning_md = None;
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

/// tool.start/tool.result → 按 call_id 配对维护工具卡；只记录当前任务
/// （轨迹匹配）或当前会话相关的事件，结果事件仅在卡片已存在时更新。
fn upsert_tool_card(app: &mut App, event: &Value) {
    let kind = event["kind"].as_str().unwrap_or("");
    if kind != "tool.start" && kind != "tool.result" {
        return;
    }
    let payload = &event["payload"];
    let Some(call_id) = payload["call_id"].as_str().map(str::to_owned) else {
        return;
    };
    if kind == "tool.result" {
        if let Some(card) = app
            .tool_cards
            .iter_mut()
            .find(|card| card.call_id == call_id)
        {
            card.status = if payload["is_error"].as_bool().unwrap_or(false) {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            };
            card.latency_ms = payload["latency_ms"].as_u64();
            card.result = Some(widgets::arg_summary(&payload["result"], 120));
        }
        return;
    }
    let relevant = if let Some(trace) = app.progress["trace_id"].as_str() {
        event["trace_id"].as_str() == Some(trace)
    } else if !app.progress.is_null() {
        true
    } else {
        app.session_id
            .as_deref()
            .is_some_and(|sid| event["session_id"].as_str() == Some(sid))
    };
    if !relevant {
        return;
    }
    let name = payload["name"].as_str().unwrap_or("工具").to_owned();
    let args = widgets::arg_summary(&payload["arguments"], 100);
    if let Some(card) = app
        .tool_cards
        .iter_mut()
        .find(|card| card.call_id == call_id)
    {
        card.name = name;
        card.args = args;
        card.status = ToolStatus::Running;
    } else {
        app.tool_cards.push(ToolCard {
            call_id,
            name,
            args,
            status: ToolStatus::Running,
            latency_ms: None,
            result: None,
        });
        if app.tool_cards.len() > 40 {
            app.tool_cards.remove(0);
        }
    }
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

fn apply_update(app: &mut App, update: Update, ctx: Option<&Ctx>) {
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
                // 草稿期选择的项目：任务完成后把新会话归入
                if let Some(ctx) = ctx
                    && let Some(project) = app.pending_project_id.take()
                {
                    spawn_request(
                        ctx,
                        "PATCH",
                        &format!("/api/sessions/{id}"),
                        Some(json!({"project_id": project})),
                        None,
                    );
                }
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
            upsert_tool_card(app, &event);
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
        Update::Resources(payload) => {
            let ResourceSnapshot {
                state,
                models,
                sessions,
                projects,
                tasks,
                objects,
                skills,
                ui,
                workspaces,
                network,
            } = *payload;
            app.ui = ui;
            app.workspaces = workspaces;
            app.state = state;
            if app.tier >= tier_names(app).len() {
                app.tier = 0;
            }
            app.models = models;
            app.sessions = sessions;
            app.projects = projects;
            app.tasks = tasks;
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
    let view_label = match app.view {
        View::Main => "对话",
        View::Tasks => "任务",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Samho / 中文对象工作台 ",
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                "  视图:{view_label}  层级:{}  模型:{model}  复杂度:{}  {}",
                tier_names(app)
                    .get(app.tier)
                    .map(String::as_str)
                    .unwrap_or("auto"),
                app.complexity,
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
    match app.view {
        View::Main => views::main::render(frame, app, rows[1], rows[2]),
        View::Tasks => views::tasks::render(frame, app, rows[1], rows[2]),
    }
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
    let hints = match app.view {
        View::Main => format!(
            "Ctrl+Q 退出  Shift+Tab 侧栏/输入  Alt+1 对话 Alt+2 任务  Tab 检查  F2 层级 Ctrl+M 模型 F6 技能  Ctrl+R 思考  Ctrl+F 引导  {} 提交  {} 新建",
            binding(app, "submit_key", "Ctrl+Enter"),
            binding(app, "new_session_key", "Alt+N")
        ),
        View::Tasks => {
            "Ctrl+Q 退出  ↑↓ 选择  Enter 打开会话  s 停止  Alt+1/Esc 返回对话".to_owned()
        }
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("流 "),
            dot(app.live_sse),
            dot(app.trace_sse),
            Span::raw(format!("  LAN {}  ", app.lan)),
            Span::styled(hints, Style::default().fg(muted)),
        ])),
        rows[4],
    );
    // composer 光标（主视图 + 输入焦点 + 无浮层）
    if app.view == View::Main
        && app.focus_composer
        && app.overlay.is_none()
        && rows[2].width > 2
        && rows[2].height > 2
    {
        let input_width = usize::from(rows[2].width - 2);
        let cursor = Line::raw(app.input.lines().last().unwrap_or(""))
            .width()
            .min(input_width.saturating_sub(1)) as u16;
        frame.set_cursor_position((rows[2].x + 1 + cursor, rows[2].y + 1));
    }
    // 浮层最后渲染（盖在主内容之上），必要时自带光标
    views::overlays::render(frame, app, area);
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
            None,
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
            None,
        );
        apply_update(
            &mut app,
            Update::Error("temporary resource connection failure".into()),
            None,
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
            None,
        );
        apply_update(
            &mut app,
            Update::Progress(json!({"text":"第一段第二段","revision":2})),
            None,
        );
        assert_eq!(app.progress["text"], "第一段第二段");
        apply_update(
            &mut app,
            Update::Finished(
                json!({"status":"complete","result":{"output":"最终答复","session_id":"s"}}),
            ),
            None,
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
                None,
            );
        }
        assert_eq!(app.events.len(), 3);
        assert_eq!(app.last_event_seq, 3);
        apply_update(
            &mut app,
            Update::Resources(Box::new(ResourceSnapshot {
                state: json!({}),
                models: vec![],
                sessions: vec![],
                projects: vec![],
                tasks: vec![],
                objects: vec![],
                skills: vec![],
                ui: json!({}),
                workspaces: vec![],
                network: json!({"addresses":["192.168.1.8","10.0.0.2"],"port":7432}),
            })),
            None,
        );
        assert_eq!(app.lan, "192.168.1.8:7432");
        apply_update(
            &mut app,
            Update::Sse {
                live: Some(true),
                trace: Some(false),
            },
            None,
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
        app.progress = json!({"text":"**加粗**流式","reasoning":"思考 **要点**","revision":3});
        refresh_markdown_cache(&mut app, 60);
        let progress = app.progress_md.as_ref().unwrap();
        assert!(progress.2.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.contains("加粗")
                    && span.style.has_modifier(ratatui::style::Modifier::BOLD)
            })
        }));
        // 思考缓存整体 muted（不保留强调色）
        let reasoning = app.reasoning_md.as_ref().unwrap();
        assert_eq!(reasoning.1, "思考 **要点**");
        assert!(
            reasoning
                .2
                .iter()
                .all(|line| line.spans.iter().all(|span| span.style.fg == Some(MUTED)))
        );
    }

    #[test]
    fn sidebar_groups_projects_days_and_archive() {
        let now = widgets::now_ms();
        let app = App {
            sessions: vec![
                json!({"id":"a","title":"今天的会话","updated_at":now,"project_id":null,"archived":false}),
                json!({"id":"b","title":"项目会话","updated_at":now-3_600_000,"project_id":"p1","archived":false}),
                json!({"id":"c","title":"三天前的会话","updated_at":now-3*86_400_000,"project_id":null,"archived":false}),
                json!({"id":"d","title":"归档会话","updated_at":now,"project_id":null,"archived":true}),
            ],
            projects: vec![json!({"id":"p1","name":"样例项目","color":"#ff0000","open":true})],
            session_id: Some("a".into()),
            ..App::default()
        };
        let model = views::sidebar::model(&app);
        let labels: Vec<String> = model
            .rows
            .iter()
            .map(|row| match row {
                views::sidebar::SideRow::Group(name, _) => name.clone(),
                views::sidebar::SideRow::Project { name, .. } => name.clone(),
                views::sidebar::SideRow::Session { title, .. } => title.clone(),
            })
            .collect();
        for expected in [
            "样例项目",
            "项目会话",
            "今天",
            "今天的会话",
            "三天前的会话",
            "归档",
        ] {
            assert!(
                labels.iter().any(|label| label.contains(expected)),
                "missing {expected} in {labels:?}"
            );
        }
        // 归档行默认折叠；可选行 = 项目 1 + 未归档会话 3
        assert!(!labels.iter().any(|label| label.contains("归档会话")));
        assert_eq!(model.sel.len(), 4);
    }

    #[test]
    fn sidebar_filter_narrows_rows_and_keys_move_cursor() {
        let (tx, _rx) = mpsc::channel(4);
        let client = reqwest::Client::new();
        let ctx = Ctx {
            client: &client,
            url: "http://127.0.0.1:1",
            sender: &tx,
        };
        let mut app = App {
            sessions: vec![
                json!({"id":"a","title":"写周报","updated_at":1}),
                json!({"id":"b","title":"修 bug","updated_at":2}),
            ],
            focus_composer: false,
            ..App::default()
        };
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.sidebar_cursor, 1);
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        assert!(app.sidebar_search);
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE),
        );
        assert_eq!(app.sidebar_filter, "bu");
        // 过滤后只剩「修 bug」一条可选行，光标被夹取回 0
        let model = views::sidebar::model(&app);
        assert_eq!(model.sel.len(), 1);
        assert_eq!(app.sidebar_cursor, 0);
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert!(!app.sidebar_search);
        assert_eq!(app.sidebar_filter, "bu"); // 第一次 Esc 只退出搜索
        views::sidebar::handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(app.sidebar_filter, "");
    }

    #[test]
    fn tool_cards_pair_start_and_result_by_call_id() {
        let mut app = App {
            progress: json!({"trace_id":"t1","revision":1}),
            ..App::default()
        };
        apply_update(
            &mut app,
            Update::TraceEvent(
                json!({"sequence":1,"trace_id":"t1","session_id":"s","kind":"tool.start",
                "payload":{"call_id":"c1","name":"write_file","arguments":{"path":"a.rs"}}}),
            ),
            None,
        );
        assert_eq!(app.tool_cards.len(), 1);
        assert_eq!(app.tool_cards[0].status, ToolStatus::Running);
        assert!(app.tool_cards[0].args.contains("path"));
        // 其它轨迹的事件不建卡
        apply_update(
            &mut app,
            Update::TraceEvent(
                json!({"sequence":2,"trace_id":"t2","session_id":"s","kind":"tool.start",
                "payload":{"call_id":"c2","name":"shell"}}),
            ),
            None,
        );
        assert_eq!(app.tool_cards.len(), 1);
        // 结果事件配对更新状态与耗时
        apply_update(
            &mut app,
            Update::TraceEvent(
                json!({"sequence":3,"trace_id":"t1","session_id":"s","kind":"tool.result",
                "payload":{"call_id":"c1","is_error":false,"latency_ms":123,"result":"已写入"}}),
            ),
            None,
        );
        assert_eq!(app.tool_cards[0].status, ToolStatus::Ok);
        assert_eq!(app.tool_cards[0].latency_ms, Some(123));
        assert_eq!(app.tool_cards[0].result.as_deref(), Some("已写入"));
        apply_update(
            &mut app,
            Update::TraceEvent(
                json!({"sequence":4,"trace_id":"t1","session_id":"s","kind":"tool.result",
                "payload":{"call_id":"c1","is_error":true}}),
            ),
            None,
        );
        assert_eq!(app.tool_cards[0].status, ToolStatus::Error);
    }

    #[test]
    fn tasks_sections_group_and_cursor_keys() {
        let app = App {
            tasks: vec![
                json!({"task_id":"1","status":"running","progress":{"session_id":"s1"}}),
                json!({"task_id":"2","status":"failed","error":"boom"}),
                json!({"task_id":"3","status":"completed"}),
                json!({"task_id":"4","status":"cancelled"}),
            ],
            ..App::default()
        };
        let sections = views::tasks::sections(&app);
        assert_eq!(sections[0].1.len(), 1);
        assert_eq!(sections[1].1.len(), 1);
        assert_eq!(sections[2].1.len(), 1);
        assert_eq!(sections[3].1.len(), 1);
        assert_eq!(views::tasks::flat(&app).len(), 4);
        // 渲染不 panic
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                views::tasks::render(
                    frame,
                    &app,
                    ratatui::layout::Rect::new(0, 0, 100, 24),
                    ratatui::layout::Rect::new(0, 24, 100, 4),
                )
            })
            .unwrap();
    }

    #[test]
    fn complexity_and_auto_commands_update_profile_without_network() {
        let client = reqwest::Client::new();
        let (tx, _rx) = mpsc::channel(4);
        let mut app = App {
            tier: 2,
            ..App::default()
        };
        app.input = "/complexity 7".into();
        submit_input(&mut app, &client, "http://127.0.0.1:1", &tx);
        assert_eq!(app.complexity, 7);
        app.input = "/complexity 99".into();
        submit_input(&mut app, &client, "http://127.0.0.1:1", &tx);
        assert_eq!(app.complexity, 7); // 越界被拒
        app.input = "/auto".into();
        submit_input(&mut app, &client, "http://127.0.0.1:1", &tx);
        assert_eq!(app.tier, 0);
    }

    #[test]
    fn finished_keeps_pending_project_without_ctx_and_assigns_session() {
        let mut app = App {
            pending_project_id: Some("p1".into()),
            ..App::default()
        };
        apply_update(
            &mut app,
            Update::Finished(
                json!({"status":"completed","result":{"output":"done","session_id":"s9"}}),
            ),
            None,
        );
        assert_eq!(app.session_id.as_deref(), Some("s9"));
        assert_eq!(app.pending_project_id.as_deref(), Some("p1")); // 无 ctx 时不丢失
        assert_eq!(app.messages[0].1, "done");
    }

    #[test]
    fn overlay_model_menu_lists_auto_and_models() {
        let app = App {
            models: vec![json!({"id":"m-a"}), json!({"id":"m-b"})],
            selected_model: 1,
            ..App::default()
        };
        let overlay = views::overlays::model_menu(&app);
        let Overlay::Menu { items, cursor, .. } = overlay else {
            panic!("expected menu overlay");
        };
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].0, "自动（按层级路由）");
        assert_eq!(cursor, 1);
    }
}
