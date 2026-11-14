//! 主视图：左侧项目侧栏、中间对话转录（Markdown 渲染 + 工具卡 + 可折叠
//! 思考块 + 轮次进度条）、右侧检查面板、底部 composer（profile chip 常显）。

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};
use serde_json::{Value, json};

use crate::tui::{
    App, PANELS, ToolCard, ToolStatus, binding, palette, role_label, views::sidebar, widgets,
};

pub(crate) fn render(frame: &mut Frame, app: &App, body: Rect, composer: Rect) {
    let (accent, muted, _) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let columns = Layout::horizontal([
        Constraint::Percentage(22),
        Constraint::Percentage(51),
        Constraint::Percentage(27),
    ])
    .split(body);
    sidebar::render(frame, app, columns[0], !app.focus_composer);
    transcript(frame, app, columns[1], accent, muted);
    inspector(frame, app, columns[2], muted);
    composer_view(frame, app, composer, accent, muted);
}

/// 轮次进度条（turn/limits.max_turns；无上限时退化为纯文本）。
fn turn_bar(app: &App, turn: u64, accent: Color, muted: Color) -> Line<'static> {
    let max = app.state["limits"]["max_turns"].as_u64().unwrap_or(0);
    let attempt = app.progress["attempt"].as_u64().unwrap_or(1);
    let spin = widgets::spinner(app.ui_tick);
    if max == 0 {
        return Line::styled(
            format!("{spin} 第 {turn} 轮 · 尝试 {attempt}"),
            Style::default().fg(muted),
        );
    }
    let width = 10usize;
    let filled = turn
        .min(max)
        .checked_mul(width as u64)
        .and_then(|scaled| scaled.checked_div(max))
        .map_or(0, |value| value as usize);
    let bar = format!("{}{}", "█".repeat(filled), "░".repeat(width - filled));
    Line::from(vec![
        Span::styled(spin, Style::default().fg(accent)),
        Span::raw(format!(" 第 {turn}/{max} 轮 · 尝试 {attempt}  ")),
        Span::styled(bar, Style::default().fg(accent)),
    ])
}

fn tool_card_lines(card: &ToolCard, tick: u64, accent: Color, muted: Color) -> Vec<Line<'static>> {
    let (mark, color) = match card.status {
        ToolStatus::Running => (widgets::spinner(tick), accent),
        ToolStatus::Ok => ("✓", muted),
        ToolStatus::Error => ("✗", Color::Red),
    };
    let mut first = vec![
        Span::styled(mark.to_owned(), Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            card.name.clone(),
            Style::default().fg(if card.status == ToolStatus::Error {
                Color::Red
            } else {
                accent
            }),
        ),
    ];
    if let Some(latency) = card.latency_ms {
        first.push(Span::styled(
            format!(" {latency}ms"),
            Style::default().fg(muted),
        ));
    }
    first.push(Span::styled(
        format!("  {}", card.args),
        Style::default().fg(muted),
    ));
    let mut lines = vec![Line::from(first)];
    if let Some(result) = &card.result {
        lines.push(Line::styled(
            format!("    ↳ {result}"),
            Style::default().fg(muted),
        ));
    }
    lines
}

fn transcript(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let mut lines = vec![
        Line::styled(
            "共享 Rust 服务 · SSE 流式 · Markdown 渲染",
            Style::default().fg(muted),
        ),
        Line::raw(""),
    ];
    // 轮次进度
    if let Some(turn) = app.progress["turn"].as_u64()
        && turn > 0
    {
        lines.push(turn_bar(app, turn, accent, muted));
        lines.push(Line::raw(""));
    }
    // 工具卡（最近 6 条，旧条目留在检查面板）
    let start = app.tool_cards.len().saturating_sub(6);
    for card in &app.tool_cards[start..] {
        lines.extend(tool_card_lines(card, app.ui_tick, accent, muted));
    }
    if !app.tool_cards.is_empty() {
        if start > 0 {
            lines.push(Line::styled(
                format!("… 其余 {start} 次工具调用见轨迹面板"),
                Style::default().fg(muted),
            ));
        }
        lines.push(Line::raw(""));
    }
    // 历史消息
    for (role, content) in &app.messages {
        lines.push(Line::styled(
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
                Some((_, cached_text, cached)) if cached_text == content => {
                    lines.extend(cached.iter().cloned());
                }
                _ => lines.extend(content.lines().map(|l| Line::raw(l.to_owned()))),
            }
        } else {
            lines.extend(content.lines().map(|l| Line::raw(l.to_owned())));
        }
        lines.push(Line::raw(""));
    }
    // 思考块（可折叠，Markdown 渲染后统一 muted）
    if let Some(reasoning) = app.progress["reasoning"].as_str()
        && !reasoning.trim().is_empty()
    {
        if app.reasoning_collapsed {
            lines.push(Line::styled(
                format!("▸ 思考中（{} 行 · Ctrl+R 展开）", reasoning.lines().count()),
                Style::default().fg(muted),
            ));
        } else {
            lines.push(Line::styled(
                "▼ 思考（Ctrl+R 折叠）",
                Style::default().fg(accent),
            ));
            match &app.reasoning_md {
                Some((_, cached_text, cached)) if cached_text == reasoning => {
                    lines.extend(cached.iter().cloned());
                }
                _ => lines.extend(
                    reasoning
                        .lines()
                        .map(|l| Line::styled(l.to_owned(), Style::default().fg(muted))),
                ),
            }
        }
        lines.push(Line::raw(""));
    }
    // 流式正文（Markdown）
    if !app.progress.is_null() {
        lines.push(Line::styled(
            "正在生成（累计脱敏快照）",
            Style::default().fg(accent),
        ));
        if let Some(text) = app.progress["text"].as_str() {
            match &app.progress_md {
                Some((_, cached_text, cached)) if cached_text == text => {
                    lines.extend(cached.iter().cloned());
                }
                _ => lines.extend(text.lines().map(|l| Line::raw(l.to_owned()))),
            }
        }
        lines.push(Line::raw(""));
    }
    if app.messages.is_empty() && app.progress.is_null() {
        lines.push(Line::raw(
            "输入任务开始对话：F2 层级、Ctrl+M 模型、F6 技能多选；/help 查看命令。",
        ));
    }
    let width = usize::from(area.width.saturating_sub(2)).max(1);
    let total = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum::<usize>();
    let scroll = total
        .saturating_sub(usize::from(area.height.saturating_sub(2)))
        .saturating_sub(usize::from(app.scroll))
        .min(usize::from(u16::MAX)) as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(" 对话 ").borders(Borders::ALL))
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        area,
    );
}

fn inspector(frame: &mut Frame, app: &App, area: Rect, muted: Color) {
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
        area,
    );
}

fn composer_view(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let skills_label = if app.selected_skills.is_empty() {
        "无".to_owned()
    } else {
        format!("{} 项", app.selected_skills.len())
    };
    let guide = if app.followup_mode {
        "follow-up"
    } else {
        "steer"
    };
    let running = if app.active.is_some() {
        format!(" · 运行中回车即{guide}")
    } else {
        String::new()
    };
    let title = format!(
        " 任务 · 提交:{} · 技能:{skills_label} · 引导:{guide}{running} ",
        binding(app, "submit_key", "Ctrl+Enter")
    );
    frame.render_widget(
        Paragraph::new(app.input.as_str())
            .block(
                Block::default()
                    .title(title)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(if app.focus_composer {
                        accent
                    } else {
                        muted
                    })),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}
