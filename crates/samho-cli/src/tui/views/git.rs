//! Git 面板（Alt+4）：状态清单（暂存/未暂存/未跟踪着色）、diff 红绿着色、
//! 提交历史、分支切换，全部走 `/api/git/*`。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::tui::{App, Ctx, PanelKind, View, palette, spawn_panel, widgets};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 面板内子页（状态 / diff / 历史 / 分支）。
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum GitPane {
    #[default]
    Status,
    Diff,
    Log,
    Branches,
}

#[derive(Clone)]
pub(crate) struct GitPanel {
    pub(crate) pane: GitPane,
    /// GET /api/git/status 原始响应（branch/upstream/changes）
    pub(crate) status: Value,
    /// 当前 diff 文本与目标（staged 与否）
    pub(crate) diff: String,
    pub(crate) diff_path: String,
    pub(crate) diff_staged: bool,
    pub(crate) log: Vec<Value>,
    pub(crate) branches: Vec<Value>,
    pub(crate) cursor: usize,
}

impl Default for GitPanel {
    fn default() -> Self {
        Self {
            pane: GitPane::Status,
            status: Value::Null,
            diff: String::new(),
            diff_path: String::new(),
            diff_staged: false,
            log: Vec::new(),
            branches: Vec::new(),
            cursor: 0,
        }
    }
}

impl GitPanel {
    pub(crate) fn changes(&self) -> Vec<Value> {
        self.status["changes"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

fn refresh(ctx: &Ctx) {
    spawn_panel(ctx, PanelKind::GitStatus, "GET", "/api/git/status", None);
}

fn load_diff(ctx: &Ctx, path: &str, staged: bool) {
    let query = format!(
        "/api/git/diff?path={}&staged={}",
        super::super::urlencode_query(path),
        staged
    );
    spawn_panel(ctx, PanelKind::GitDiff, "GET", &query, None);
}

/// 当前光标指向的变更路径。
fn selected_path(panel: &GitPanel) -> Option<String> {
    let changes = panel.changes();
    let change = changes.get(panel.cursor)?;
    change["path"].as_str().map(str::to_owned)
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let pane = app.git.pane;
    match pane {
        GitPane::Status => handle_status(app, ctx, key),
        GitPane::Diff => match key.code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => {
                app.git.pane = GitPane::Status;
                app.git.cursor = 0;
            }
            _ => {}
        },
        GitPane::Log => handle_log(app, ctx, key),
        GitPane::Branches => handle_branches(app, ctx, key, ctrl),
    }
}

fn handle_status(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let count = app.git.changes().len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.git.cursor = app.git.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                app.git.cursor = (app.git.cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home => app.git.cursor = 0,
        KeyCode::End => app.git.cursor = count.saturating_sub(1),
        KeyCode::Enter => {
            // 查看该路径 diff（有暂存内容时优先看 staged）
            let Some(path) = selected_path(&app.git) else {
                return;
            };
            let staged = app.git.changes()[app.git.cursor]["staged"]
                .as_bool()
                .unwrap_or(false);
            app.git.diff_staged = staged;
            app.git.diff_path = path.clone();
            app.git.pane = GitPane::Diff;
            app.git.diff.clear();
            load_diff(ctx, &path, staged);
        }
        KeyCode::Char('s') => {
            if let Some(path) = selected_path(&app.git) {
                spawn_panel(
                    ctx,
                    PanelKind::GitMut,
                    "POST",
                    "/api/git/stage",
                    Some(json!({"paths":[path]})),
                );
            }
        }
        KeyCode::Char('S') => {
            // 暂存全部
            let paths: Vec<String> = app
                .git
                .changes()
                .iter()
                .filter_map(|change| change["path"].as_str().map(str::to_owned))
                .collect();
            if !paths.is_empty() {
                spawn_panel(
                    ctx,
                    PanelKind::GitMut,
                    "POST",
                    "/api/git/stage",
                    Some(json!({"paths": paths})),
                );
            }
        }
        KeyCode::Char('u') => {
            if let Some(path) = selected_path(&app.git) {
                spawn_panel(
                    ctx,
                    PanelKind::GitMut,
                    "POST",
                    "/api/git/unstage",
                    Some(json!({"paths":[path]})),
                );
            }
        }
        KeyCode::Char('D') => {
            // 丢弃未暂存修改（已跟踪文件）
            if let Some(path) = selected_path(&app.git) {
                app.overlay = Some(crate::tui::Overlay::Confirm {
                    title: format!("丢弃 {} 的未暂存修改？", widgets::truncate(&path, 40)),
                    accept: crate::tui::Action::GitDiscard(vec![path]),
                });
            }
        }
        KeyCode::Char('c') => {
            app.overlay = Some(crate::tui::Overlay::Prompt {
                title: "提交说明".into(),
                input: String::new(),
                action: crate::tui::PromptAction::GitCommit,
            });
        }
        KeyCode::Char('l') => {
            app.git.pane = GitPane::Log;
            spawn_panel(ctx, PanelKind::GitLog, "GET", "/api/git/log?limit=30", None);
        }
        KeyCode::Char('b') => {
            app.git.pane = GitPane::Branches;
            spawn_panel(
                ctx,
                PanelKind::GitBranches,
                "GET",
                "/api/git/branches",
                None,
            );
        }
        KeyCode::Char('r') | KeyCode::F(5) => refresh(ctx),
        _ => {}
    }
}

fn handle_log(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let count = app.git.log.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.git.cursor = app.git.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                app.git.cursor = (app.git.cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home => app.git.cursor = 0,
        KeyCode::End => app.git.cursor = count.saturating_sub(1),
        KeyCode::Esc | KeyCode::Backspace => {
            app.git.pane = GitPane::Status;
            app.git.cursor = 0;
        }
        KeyCode::Char('b') => {
            app.git.pane = GitPane::Branches;
            spawn_panel(
                ctx,
                PanelKind::GitBranches,
                "GET",
                "/api/git/branches",
                None,
            );
        }
        _ => {}
    }
}

fn handle_branches(app: &mut App, _ctx: &Ctx, key: KeyEvent, _ctrl: bool) {
    let count = app.git.branches.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => app.git.cursor = app.git.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                app.git.cursor = (app.git.cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home => app.git.cursor = 0,
        KeyCode::End => app.git.cursor = count.saturating_sub(1),
        KeyCode::Enter => {
            if let Some(branch) = app.git.branches.get(app.git.cursor) {
                let name = branch["name"].as_str().unwrap_or_default().to_owned();
                let current = branch["current"].as_bool().unwrap_or(false);
                if !name.is_empty() && !current {
                    app.overlay = Some(crate::tui::Overlay::Confirm {
                        title: format!("切换到分支 {name}？"),
                        accept: crate::tui::Action::GitCheckout(name),
                    });
                }
            }
        }
        KeyCode::Char('n') => {
            app.overlay = Some(crate::tui::Overlay::Prompt {
                title: "新建分支并切换".into(),
                input: String::new(),
                action: crate::tui::PromptAction::NewBranch,
            });
        }
        KeyCode::Esc | KeyCode::Backspace => {
            app.git.pane = GitPane::Status;
            app.git.cursor = 0;
        }
        _ => {}
    }
}

/// Git 面板渲染。
pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (accent, muted, background) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let focused = app.view == View::Git;
    let pane_label = match app.git.pane {
        GitPane::Status => "状态",
        GitPane::Diff => "差异",
        GitPane::Log => "历史",
        GitPane::Branches => "分支",
    };
    let branch = app.git.status["branch"].as_str().unwrap_or("");
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            format!(" Git · {pane_label} · {branch} "),
            Style::default().fg(if focused { accent } else { muted }),
        ))
        .style(Style::default().bg(background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match app.git.pane {
        GitPane::Status => render_status(frame, app, inner, accent, muted),
        GitPane::Diff => render_diff(frame, app, inner, accent, muted),
        GitPane::Log => render_log(frame, app, inner, accent, muted),
        GitPane::Branches => render_branches(frame, app, inner, accent, muted),
    }
}

fn render_status(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let changes = app.git.changes();
    let count = changes.len();
    let cursor = if count == 0 {
        0
    } else {
        app.git.cursor.min(count - 1)
    };
    let mut lines = vec![Line::from(Span::styled(
        "Enter 看 diff · s 暂存 · S 全部暂存 · u 取消暂存 · D 丢弃 · c 提交 · l 历史 · b 分支 · r 刷新",
        Style::default().fg(muted),
    ))];
    if changes.is_empty() {
        lines.push(Line::from(Span::styled(
            "工作区干净 ✓",
            Style::default().fg(Color::Green),
        )));
    }
    for (index, change) in changes.iter().enumerate() {
        let x = change["x"].as_str().unwrap_or(" ");
        let y = change["y"].as_str().unwrap_or(" ");
        let path = change["path"].as_str().unwrap_or("");
        let untracked = change["untracked"].as_bool().unwrap_or(false);
        let staged = change["staged"].as_bool().unwrap_or(false);
        let color = if untracked {
            Color::Yellow
        } else if staged {
            Color::Green
        } else {
            accent
        };
        lines.push(Line::from(vec![
            Span::styled(
                if index == cursor { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(format!("{x}{y} "), Style::default().fg(color)),
            Span::styled(
                widgets::truncate(path, area.width.saturating_sub(8) as usize),
                Style::default().fg(color).add_modifier(if index == cursor {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
        ]));
    }
    let upstream = app.git.status["upstream"].as_str();
    if let Some(upstream) = upstream {
        lines.push(Line::from(Span::styled(
            format!("上游 {upstream}"),
            Style::default().fg(muted),
        )));
    }
    let scroll = cursor.saturating_sub(usize::from(area.height.saturating_sub(2)));
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        area,
    );
}

fn render_diff(frame: &mut Frame, app: &App, area: Rect, _accent: Color, muted: Color) {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            "Esc 返回 · {}（{}）",
            widgets::truncate(&app.git.diff_path, area.width.saturating_sub(20) as usize),
            if app.git.diff_staged {
                "已暂存"
            } else {
                "未暂存"
            }
        ),
        Style::default().fg(muted),
    ))];
    if app.git.diff.is_empty() {
        lines.push(Line::from(Span::styled(
            "（无差异）",
            Style::default().fg(muted),
        )));
    }
    for line in app.git.diff.lines() {
        let style = if line.starts_with("+++")
            || line.starts_with("---")
            || line.starts_with("diff ")
            || line.starts_with("index ")
        {
            Style::default().fg(muted)
        } else if line.starts_with("@@") {
            Style::default().fg(Color::Cyan)
        } else if line.starts_with('+') {
            Style::default().fg(Color::Green)
        } else if line.starts_with('-') {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Reset)
        };
        lines.push(Line::from(Span::styled(
            widgets::truncate(line, area.width as usize),
            style,
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_log(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let count = app.git.log.len();
    let cursor = if count == 0 {
        0
    } else {
        app.git.cursor.min(count - 1)
    };
    let mut lines = vec![Line::from(Span::styled(
        "↑↓ 浏览 · Esc 返回状态 · b 分支",
        Style::default().fg(muted),
    ))];
    if app.git.log.is_empty() {
        lines.push(Line::from(Span::styled(
            "（空仓库，还没有提交）",
            Style::default().fg(muted),
        )));
    }
    for (index, commit) in app.git.log.iter().enumerate() {
        let short = commit["short"].as_str().unwrap_or("");
        let author = commit["author"].as_str().unwrap_or("");
        let time = commit["time"].as_u64().unwrap_or(0);
        let subject = commit["subject"].as_str().unwrap_or("");
        let refs = commit["refs"].as_str().unwrap_or("");
        let selected = index == cursor;
        let first = format!("{} {short}", if selected { "›" } else { " " });
        let mut spans = vec![
            Span::styled(
                first,
                Style::default()
                    .fg(if selected { accent } else { muted })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" {author} "), Style::default().fg(muted)),
            Span::styled(
                widgets::relative_time(time, widgets::now_ms()),
                Style::default().fg(muted),
            ),
        ];
        if !refs.is_empty() {
            spans.push(Span::styled(
                format!(" ⭐{refs}"),
                Style::default().fg(Color::Cyan),
            ));
        }
        lines.push(Line::from(spans));
        lines.push(Line::from(Span::styled(
            format!(
                "    {}",
                widgets::truncate(subject, area.width.saturating_sub(6) as usize)
            ),
            Style::default().fg(if selected { Color::Reset } else { muted }),
        )));
    }
    let scroll = cursor.saturating_sub(usize::from(area.height.saturating_sub(3)) / 2);
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll * 2).unwrap_or(u16::MAX), 0)),
        area,
    );
}

fn render_branches(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let count = app.git.branches.len();
    let cursor = if count == 0 {
        0
    } else {
        app.git.cursor.min(count - 1)
    };
    let mut lines = vec![Line::from(Span::styled(
        "Enter 切换 · n 新建分支 · Esc 返回状态",
        Style::default().fg(muted),
    ))];
    if app.git.branches.is_empty() {
        lines.push(Line::from(Span::styled(
            "（无分支）",
            Style::default().fg(muted),
        )));
    }
    for (index, branch) in app.git.branches.iter().enumerate() {
        let name = branch["name"].as_str().unwrap_or("");
        let current = branch["current"].as_bool().unwrap_or(false);
        lines.push(Line::from(vec![
            Span::styled(
                if index == cursor { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(
                if current { "● " } else { "  " },
                Style::default().fg(Color::Green),
            ),
            Span::styled(
                name.to_owned(),
                Style::default()
                    .fg(if current { Color::Green } else { Color::Reset })
                    .add_modifier(if index == cursor {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ]));
    }
    let scroll = cursor.saturating_sub(usize::from(area.height.saturating_sub(2)));
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    fn fixture_app() -> App {
        App {
            view: View::Git,
            git: GitPanel {
                status: json!({
                    "branch": "master",
                    "upstream": "origin/master",
                    "changes": [
                        {"x":"?","y":"?","path":"new.rs","staged":false,"untracked":true},
                        {"x":"M","y":" ","path":"mod.rs","staged":true,"untracked":false},
                        {"x":" ","y":"M","path":"api.rs","staged":false,"untracked":false}
                    ]
                }),
                log: vec![
                    json!({"short":"abc1234","author":"sam","time":1_700_000_000,"subject":"初始提交","refs":"HEAD -> master"}),
                ],
                branches: vec![
                    json!({"name":"master","current":true}),
                    json!({"name":"feature","current":false}),
                ],
                ..GitPanel::default()
            },
            ..App::default()
        }
    }

    #[tokio::test]
    async fn status_navigation_stage_and_diff_switch() {
        let client = reqwest::Client::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let ctx = Ctx {
            client: &client,
            url: "http://127.0.0.1:1",
            sender: &tx,
        };
        let mut app = fixture_app();
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(app.git.cursor, 1);
        // Enter → diff 子页
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert_eq!(app.git.pane, GitPane::Diff);
        assert_eq!(app.git.diff_path, "mod.rs");
        assert!(app.git.diff_staged);
        // Esc → 状态
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(app.git.pane, GitPane::Status);
        // l → 历史，b → 分支
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert_eq!(app.git.pane, GitPane::Log);
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        );
        assert_eq!(app.git.pane, GitPane::Branches);
        // 光标在非当前分支上按 Enter → 弹确认浮层
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(app.overlay.is_some(), "切换分支应弹确认");
    }

    #[test]
    fn panes_render_without_panic() {
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        let area = ratatui::layout::Rect::new(0, 0, 100, 26);
        for pane in [
            GitPane::Status,
            GitPane::Diff,
            GitPane::Log,
            GitPane::Branches,
        ] {
            let mut app = fixture_app();
            app.git.pane = pane;
            if pane == GitPane::Diff {
                app.git.diff =
                    "diff --git a/mod.rs b/mod.rs\n@@ -1 +1 @@\n-old line\n+new line".into();
            }
            terminal.draw(|frame| render(frame, &app, area)).unwrap();
        }
    }
}
