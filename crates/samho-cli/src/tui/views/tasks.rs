//! 任务看板（Alt+2）：/api/tasks 四段分组（执行中/已完成/失败/已取消），
//! 卡片含模型/轮次/尝试/最近文本，s 停止、Enter 打开会话、Esc 返回对话。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::{Value, json};

use crate::tui::{App, Ctx, View, palette, widgets};

/// 四段固定顺序分组（空段也展示，便于一眼看出分布）。
pub(crate) fn sections(app: &App) -> Vec<(&'static str, Vec<&Value>)> {
    let mut running = Vec::new();
    let mut completed = Vec::new();
    let mut failed = Vec::new();
    let mut cancelled = Vec::new();
    for task in &app.tasks {
        match task["status"].as_str().unwrap_or("") {
            "running" | "queued" => running.push(task),
            "failed" => failed.push(task),
            "cancelled" => cancelled.push(task),
            _ => completed.push(task),
        }
    }
    vec![
        ("执行中", running),
        ("已完成", completed),
        ("失败", failed),
        ("已取消", cancelled),
    ]
}

/// 光标用的扁平任务列表（执行中在前）。
pub(crate) fn flat(app: &App) -> Vec<&Value> {
    sections(app)
        .into_iter()
        .flat_map(|(_, list)| list)
        .collect()
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let count = flat(app).len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') if !ctrl && !alt => {
            if count > 0 {
                app.tasks_cursor = app.tasks_cursor.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !ctrl && !alt => {
            if count > 0 {
                app.tasks_cursor = (app.tasks_cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home if !ctrl && count > 0 => app.tasks_cursor = 0,
        KeyCode::End if !ctrl && count > 0 => app.tasks_cursor = count - 1,
        KeyCode::Enter if !ctrl => {
            let target = flat(app).get(app.tasks_cursor).map(|task| {
                task["result"]["session_id"]
                    .as_str()
                    .or_else(|| task["progress"]["session_id"].as_str())
                    .map(str::to_owned)
            });
            match target.flatten() {
                Some(session_id) => super::super::open_session(app, ctx, &session_id),
                None => app.status = "该任务尚未产生会话".into(),
            }
        }
        KeyCode::Char('s') if !ctrl && !alt => {
            let target = flat(app).get(app.tasks_cursor).map(|task| {
                (
                    task["task_id"].as_str().map(str::to_owned),
                    task["status"].as_str().map(str::to_owned),
                )
            });
            if let Some((Some(id), Some(status))) = target {
                if matches!(status.as_str(), "running" | "queued") {
                    super::super::spawn_request(
                        ctx,
                        "POST",
                        &format!("/api/tasks/{id}/cancel"),
                        Some(json!({})),
                        None,
                    );
                    app.status = "已请求停止任务".into();
                } else {
                    app.status = "任务已结束，无需停止".into();
                }
            }
        }
        KeyCode::Esc if !ctrl => app.view = View::Main,
        _ => {}
    }
}

pub(crate) fn render(frame: &mut Frame, app: &App, body: Rect, composer: Rect) {
    let (accent, muted, _) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let flat_list = flat(app);
    let cursor = if flat_list.is_empty() {
        0
    } else {
        app.tasks_cursor.min(flat_list.len() - 1)
    };
    let mut lines = vec![
        Line::styled(
            "↑↓ 选择 · Enter 打开会话 · s 停止 · Esc 返回对话",
            Style::default().fg(muted),
        ),
        Line::raw(""),
    ];
    let mut index = 0usize;
    for (label, list) in sections(app) {
        lines.push(Line::styled(
            format!("── {label} ({})", list.len()),
            Style::default().fg(muted).add_modifier(Modifier::BOLD),
        ));
        if list.is_empty() {
            lines.push(Line::styled("（无）", Style::default().fg(muted)));
        }
        for task in list {
            let selected = index == cursor;
            index += 1;
            lines.extend(card_lines(task, selected, cursor, accent, muted));
        }
        lines.push(Line::raw(""));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" 任务看板（Alt+2） ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(accent)),
            )
            .wrap(Wrap { trim: false }),
        body,
    );
    frame.render_widget(
        Paragraph::new("任务在后台由运行时调度；此处可随时停止执行中任务或回到对应会话。")
            .block(
                Block::default()
                    .title(" 任务操作 ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(muted)),
            )
            .wrap(Wrap { trim: false }),
        composer,
    );
}

fn card_lines(
    task: &Value,
    selected: bool,
    cursor: usize,
    accent: Color,
    muted: Color,
) -> Vec<Line<'static>> {
    let status = task["status"].as_str().unwrap_or("unknown");
    let progress = &task["progress"];
    let model = progress["model_id"].as_str().unwrap_or("—");
    let turn = progress["turn"].as_u64().unwrap_or(0);
    let attempt = progress["attempt"].as_u64().unwrap_or(1);
    let session_id = task["result"]["session_id"]
        .as_str()
        .or_else(|| progress["session_id"].as_str())
        .unwrap_or("");
    let snippet = task["progress"]["text"]
        .as_str()
        .or_else(|| task["error"].as_str())
        .or_else(|| task["result"]["output"].as_str())
        .unwrap_or("—");
    vec![
        Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled("●", Style::default().fg(widgets::status_color(status))),
            Span::raw(format!(
                " {} ",
                widgets::short_id(task["task_id"].as_str().unwrap_or("?"))
            )),
            Span::styled(status.to_owned(), Style::default().fg(muted)),
            Span::raw(format!(" {model} 第{turn}轮/尝试{attempt}")),
            Span::styled(
                if session_id.is_empty() {
                    String::new()
                } else {
                    format!(" · 会话 {}", widgets::short_id(session_id))
                },
                Style::default().fg(muted),
            ),
            Span::styled(
                if selected {
                    format!(" (第{}项)", cursor + 1)
                } else {
                    String::new()
                },
                Style::default().fg(accent),
            ),
        ]),
        Line::styled(
            format!("    {}", widgets::truncate(&snippet.replace('\n', " "), 80)),
            Style::default().fg(muted),
        ),
    ]
}
