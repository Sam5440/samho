//! 侧栏：项目分组（色点/计数/展开态）+ 未分类按 今天/昨天/更早 分组 +
//! 归档区 + 搜索过滤 + 行菜单。数据与 web 前端同源（/api/projects、/api/sessions）。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::{Value, json};

use crate::tui::{
    Action, App, Ctx, Overlay, PromptAction, palette,
    widgets::{self, now_ms},
};

/// 侧栏行（Group 行不可选中；Project/Session 行进入 sel 供光标定位）。
#[derive(Clone)]
pub(crate) enum SideRow {
    Group(String, usize),
    Project {
        name: String,
        color: Option<String>,
        open: bool,
        count: usize,
        selected: bool,
    },
    Session {
        title: String,
        current: bool,
        archived: bool,
        depth: usize,
        updated_at: u64,
        selected: bool,
    },
}

/// 可选中的行（光标只落在这些行上）。
#[derive(Clone)]
pub(crate) enum Sel {
    Project(String),
    Session(String),
}

pub(crate) struct SideModel {
    pub rows: Vec<SideRow>,
    pub sel: Vec<Sel>,
}

/// 由当前 App 状态构建侧栏行模型（渲染与键位处理共用同一份派生逻辑）。
pub(crate) fn model(app: &App) -> SideModel {
    let filter = app.sidebar_filter.trim().to_lowercase();
    let now = now_ms();
    let mut sessions: Vec<&Value> = app.sessions.iter().collect();
    sessions.sort_by(|a, b| {
        b["updated_at"]
            .as_u64()
            .unwrap_or(0)
            .cmp(&a["updated_at"].as_u64().unwrap_or(0))
    });
    let title_of = |s: &Value| {
        s["title"]
            .as_str()
            .or_else(|| s["id"].as_str())
            .unwrap_or("未命名会话")
            .to_owned()
    };
    let archived_of = |s: &Value| s["archived"].as_bool().unwrap_or(false);
    let project_of = |s: &Value| s["project_id"].as_str().map(str::to_owned);
    let visible = |s: &Value| filter.is_empty() || title_of(s).to_lowercase().contains(&filter);

    let mut rows: Vec<SideRow> = Vec::new();
    let mut sel: Vec<Sel> = Vec::new();

    // 项目分组：成员标题匹配过滤；搜索时空项目隐藏，展开态非搜索时强制展示
    for p in &app.projects {
        let pid = p["id"].as_str().unwrap_or_default().to_owned();
        let members: Vec<&Value> = sessions
            .iter()
            .copied()
            .filter(|s| {
                !archived_of(s) && project_of(s).as_deref() == Some(pid.as_str()) && visible(s)
            })
            .collect();
        if members.is_empty() && !filter.is_empty() {
            continue;
        }
        let open = p["open"].as_bool().unwrap_or(true);
        rows.push(SideRow::Project {
            name: p["name"].as_str().unwrap_or("未命名项目").to_owned(),
            color: p["color"].as_str().map(str::to_owned),
            open,
            count: members.len(),
            selected: false,
        });
        sel.push(Sel::Project(pid));
        if open || !filter.is_empty() {
            for s in members {
                push_session(&mut rows, &mut sel, s, app.session_id.as_deref());
            }
        }
    }

    // 未分类：按日分组
    let loose: Vec<&Value> = sessions
        .iter()
        .copied()
        .filter(|s| !archived_of(s) && project_of(s).is_none() && visible(s))
        .collect();
    if !loose.is_empty() {
        for label in ["今天", "昨天", "更早"] {
            let part: Vec<&Value> = loose
                .iter()
                .copied()
                .filter(|s| widgets::day_group(s["updated_at"].as_u64().unwrap_or(0), now) == label)
                .collect();
            if part.is_empty() {
                continue;
            }
            rows.push(SideRow::Group(label.to_owned(), part.len()));
            for s in part {
                push_session(&mut rows, &mut sel, s, app.session_id.as_deref());
            }
        }
    }

    // 归档：默认折叠，按 a 展开
    let archived: Vec<&Value> = sessions
        .iter()
        .copied()
        .filter(|s| archived_of(s) && visible(s))
        .collect();
    if !archived.is_empty() {
        rows.push(SideRow::Group(
            format!(
                "归档{}",
                if app.show_archived {
                    ""
                } else {
                    "（a 展开）"
                }
            ),
            archived.len(),
        ));
        if app.show_archived {
            for s in archived {
                push_session(&mut rows, &mut sel, s, app.session_id.as_deref());
            }
        }
    }

    // 标注选中行（光标按可选行数夹取，避免重建模型时越界）
    let cursor = if sel.is_empty() {
        0
    } else {
        app.sidebar_cursor.min(sel.len() - 1)
    };
    let mut index = 0usize;
    for row in &mut rows {
        let is_selectable = matches!(row, SideRow::Project { .. } | SideRow::Session { .. });
        match row {
            SideRow::Project { selected, .. } | SideRow::Session { selected, .. } => {
                *selected = is_selectable && index == cursor;
            }
            SideRow::Group(..) => {}
        }
        if is_selectable {
            index += 1;
        }
    }
    SideModel { rows, sel }
}

fn push_session(rows: &mut Vec<SideRow>, sel: &mut Vec<Sel>, s: &Value, current: Option<&str>) {
    let sid = s["id"].as_str().unwrap_or_default().to_owned();
    let title = s["title"]
        .as_str()
        .or_else(|| s["id"].as_str())
        .unwrap_or("未命名会话")
        .to_owned();
    rows.push(SideRow::Session {
        current: current == Some(sid.as_str()),
        title,
        archived: s["archived"].as_bool().unwrap_or(false),
        depth: usize::from(s["parent_id"].as_str().is_some_and(|p| !p.is_empty())),
        updated_at: s["updated_at"].as_u64().unwrap_or(0),
        selected: false,
    });
    sel.push(Sel::Session(sid));
}

/// 当前光标所在行对应的项目 id（项目行取自身；会话行取其归属）。
fn selected_project(app: &App) -> Option<String> {
    let model = model(app);
    let selection = model.sel.get(app.sidebar_cursor)?;
    match selection {
        Sel::Project(pid) => Some(pid.clone()),
        Sel::Session(sid) => app
            .sessions
            .iter()
            .find(|s| s["id"].as_str() == Some(sid.as_str()))
            .and_then(|s| s["project_id"].as_str())
            .map(str::to_owned),
    }
}

/// 侧栏键位：↑↓/jk 移动、Enter 激活（展开项目/打开会话）、/ 搜索、
/// n 新会话草稿、P 新建项目、m 行菜单、a 展开归档、Esc 清空过滤。
pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if app.sidebar_search {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.sidebar_search = false;
                app.sidebar_cursor = 0;
            }
            KeyCode::Backspace => {
                app.sidebar_filter.pop();
                app.sidebar_cursor = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                app.sidebar_filter.push(c);
                app.sidebar_cursor = 0;
            }
            _ => {}
        }
        return;
    }
    let count = model(app).sel.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') if !ctrl && !alt => {
            if count > 0 {
                app.sidebar_cursor = app.sidebar_cursor.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !ctrl && !alt => {
            if count > 0 {
                app.sidebar_cursor = (app.sidebar_cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home if !ctrl && count > 0 => app.sidebar_cursor = 0,
        KeyCode::End if !ctrl && count > 0 => app.sidebar_cursor = count - 1,
        KeyCode::Char('/') if !ctrl && !alt => app.sidebar_search = true,
        KeyCode::Esc if !ctrl => {
            app.sidebar_filter.clear();
            app.sidebar_cursor = 0;
        }
        KeyCode::Char('a') if !ctrl && !alt => app.show_archived = !app.show_archived,
        KeyCode::Char('n') if !ctrl && !alt => new_session_draft(app),
        KeyCode::Char('p' | 'P') if !ctrl && !alt => {
            app.overlay = Some(Overlay::Prompt {
                title: "新建项目".into(),
                input: String::new(),
                action: PromptAction::NewProject,
            });
        }
        KeyCode::Char('m') if !ctrl && !alt => open_row_menu(app),
        KeyCode::Enter if !ctrl => activate(app, ctx),
        _ => {}
    }
}

/// n：新会话草稿；若光标停在项目（或项目内会话）上，任务完成后自动归入该项目。
fn new_session_draft(app: &mut App) {
    let project = selected_project(app);
    super::super::reset_draft(app);
    app.pending_project_id = project;
    app.focus = crate::tui::Focus::Composer;
    app.status = if app.pending_project_id.is_some() {
        "新会话草稿（任务完成后归入所选项目）".into()
    } else {
        "新会话草稿".into()
    };
}

fn activate(app: &mut App, ctx: &Ctx) {
    let selection = {
        let model = model(app);
        app.sidebar_cursor = app.sidebar_cursor.min(model.sel.len().saturating_sub(1));
        model.sel.get(app.sidebar_cursor).cloned()
    };
    match selection {
        Some(Sel::Project(pid)) => {
            let open = app
                .projects
                .iter()
                .find(|p| p["id"].as_str() == Some(pid.as_str()))
                .and_then(|p| p["open"].as_bool())
                .unwrap_or(true);
            let new_open = !open;
            if let Some(p) = app
                .projects
                .iter_mut()
                .find(|p| p["id"].as_str() == Some(pid.as_str()))
            {
                p["open"] = json!(new_open);
            }
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/projects/{pid}"),
                Some(json!({"open": new_open})),
                None,
            );
        }
        Some(Sel::Session(sid)) => super::super::open_session(app, ctx, &sid),
        None => {}
    }
}

/// m：按行类型弹出会话/项目操作菜单。
fn open_row_menu(app: &mut App) {
    let selection = {
        let model = model(app);
        app.sidebar_cursor = app.sidebar_cursor.min(model.sel.len().saturating_sub(1));
        model.sel.get(app.sidebar_cursor).cloned()
    };
    match selection {
        Some(Sel::Session(sid)) => {
            let archived = app
                .sessions
                .iter()
                .find(|s| s["id"].as_str() == Some(sid.as_str()))
                .and_then(|s| s["archived"].as_bool())
                .unwrap_or(false);
            app.overlay = Some(Overlay::Menu {
                title: "会话操作".into(),
                items: vec![
                    ("打开会话".into(), Action::OpenSession(sid.clone())),
                    ("重命名…".into(), Action::RenameSession(sid.clone())),
                    ("创建分支（fork）…".into(), Action::ForkSession(sid.clone())),
                    (
                        if archived {
                            "取消归档".into()
                        } else {
                            "归档".into()
                        },
                        Action::ArchiveSession(sid.clone(), !archived),
                    ),
                    ("移动到项目…".into(), Action::MoveSession(sid.clone())),
                    ("删除会话…".into(), Action::DeleteSession(sid)),
                ],
                cursor: 0,
            });
        }
        Some(Sel::Project(pid)) => {
            let open = app
                .projects
                .iter()
                .find(|p| p["id"].as_str() == Some(pid.as_str()))
                .and_then(|p| p["open"].as_bool())
                .unwrap_or(true);
            app.overlay = Some(Overlay::Menu {
                title: "项目操作".into(),
                items: vec![
                    (
                        if open {
                            "收起".into()
                        } else {
                            "展开".into()
                        },
                        Action::ToggleProject(pid.clone(), !open),
                    ),
                    ("重命名…".into(), Action::RenameProject(pid.clone())),
                    (
                        "删除项目（会话回到未分类）…".into(),
                        Action::DeleteProject(pid),
                    ),
                ],
                cursor: 0,
            });
        }
        None => app.status = "侧栏为空".into(),
    }
}

pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let (accent, muted, _) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let model = model(app);
    let now = now_ms();
    let mut lines: Vec<Line<'static>> = Vec::new();
    for row in &model.rows {
        match row {
            SideRow::Group(name, count) => {
                lines.push(Line::styled(
                    format!("── {name} ({count})"),
                    Style::default().fg(muted).add_modifier(Modifier::BOLD),
                ));
            }
            SideRow::Project {
                name,
                color,
                open,
                count,
                selected,
                ..
            } => {
                let dot_color = color
                    .as_deref()
                    .and_then(widgets::hex_color)
                    .unwrap_or(muted);
                let name_style = if *selected {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        if *selected { "› " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::styled(if *open { "▾" } else { "▸" }, Style::default().fg(muted)),
                    Span::raw(" "),
                    Span::styled("●", Style::default().fg(dot_color)),
                    Span::raw(" "),
                    Span::styled(name.clone(), name_style),
                    Span::styled(format!(" ({count})"), Style::default().fg(muted)),
                ]));
            }
            SideRow::Session {
                title,
                current,
                archived,
                depth,
                updated_at,
                selected,
                ..
            } => {
                let indent = "  ".repeat(1 + depth);
                let title_style = if *current {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else if *archived {
                    Style::default().fg(muted)
                } else {
                    Style::default()
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        if *selected { "› " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::raw(indent),
                    Span::styled(
                        if *current { "▸ " } else { "  " },
                        Style::default().fg(accent),
                    ),
                    Span::styled(title.clone(), title_style),
                    Span::styled(
                        format!(" {}", widgets::relative_time(*updated_at, now)),
                        Style::default().fg(muted),
                    ),
                ]));
            }
        }
    }
    if model.rows.is_empty() {
        lines.push(Line::styled(
            "暂无会话 · n 新建",
            Style::default().fg(muted),
        ));
    }
    let title = if app.sidebar_search {
        format!(" 搜索:{}▏ ", app.sidebar_filter)
    } else {
        " 项目与会话 ".into()
    };
    let border = if focused { accent } else { muted };
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border)),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}
