//! 检查面板（右列，Tab 循环 6 个）：轨迹调用卡（可展开/搜索/E 导出）、
//! 预算汇总、对象/技能/MCP 卡片化、模型角色分配编辑。仅当焦点处于
//! 检查面板时响应键位，避免与 composer 输入冲突。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::tui::{
    Action, App, Ctx, ModelCall, Overlay, PANELS, ToolCard, ToolStatus, palette, widgets,
};

/// 轨迹面板的一张卡片（模型调用或工具调用，按事件序号倒序展示）。
pub(crate) enum TrajCard<'a> {
    Model(&'a ModelCall),
    Tool(&'a ToolCard),
}

impl TrajCard<'_> {
    fn key(&self) -> &str {
        match self {
            TrajCard::Model(call) => &call.key,
            TrajCard::Tool(card) => &card.call_id,
        }
    }
    fn sequence(&self) -> u64 {
        match self {
            TrajCard::Model(call) => call.sequence,
            TrajCard::Tool(card) => card.sequence,
        }
    }
    fn matches_filter(&self, filter: &str) -> bool {
        if filter.is_empty() {
            return true;
        }
        let haystack = match self {
            TrajCard::Model(call) => format!(
                "model {} {} {} {}",
                call.model_id,
                call.finish_reason.as_deref().unwrap_or(""),
                call.error.as_deref().unwrap_or(""),
                call.message.as_deref().unwrap_or("")
            ),
            TrajCard::Tool(card) => {
                format!(
                    "tool {} {} {}",
                    card.name,
                    card.args,
                    card.result.as_deref().unwrap_or("")
                )
            }
        };
        haystack.to_lowercase().contains(filter)
    }
}

/// 当前检查面板下的可选行数（光标夹取依据）。
pub(crate) fn panel_len(app: &App) -> usize {
    match app.panel {
        0 => trajectory(app).len(),
        3 => app.skills.len(),
        4 => app
            .objects
            .iter()
            .filter(|o| o["kind"].as_str().is_some_and(|k| k.contains("mcp")))
            .count(),
        5 => app.roles["roles"]
            .as_object()
            .map_or(0, |roles| roles.len()),
        _ => 0,
    }
}

/// 轨迹卡片合并列表：模型调用 + 工具调用，按事件序号倒序（最新在前），
/// 再按搜索过滤。
pub(crate) fn trajectory(app: &App) -> Vec<TrajCard<'_>> {
    let filter = app.inspector_filter.trim().to_lowercase();
    let mut cards: Vec<TrajCard<'_>> = Vec::new();
    cards.extend(app.model_calls.iter().map(TrajCard::Model));
    cards.extend(app.tool_cards.iter().map(TrajCard::Tool));
    cards.sort_by_key(|card| std::cmp::Reverse(card.sequence()));
    cards.retain(|card| card.matches_filter(&filter));
    cards
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    if app.inspector_search {
        match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                app.inspector_search = false;
                app.inspector_cursor = 0;
            }
            KeyCode::Backspace => {
                app.inspector_filter.pop();
                app.inspector_cursor = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                app.inspector_filter.push(c);
                app.inspector_cursor = 0;
            }
            _ => {}
        }
        return;
    }
    let count = panel_len(app);
    match key.code {
        KeyCode::Up | KeyCode::Char('k') if !ctrl && !alt => {
            if count > 0 {
                app.inspector_cursor = app.inspector_cursor.saturating_sub(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') if !ctrl && !alt => {
            if count > 0 {
                app.inspector_cursor = (app.inspector_cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home if !ctrl && count > 0 => app.inspector_cursor = 0,
        KeyCode::End if !ctrl && count > 0 => app.inspector_cursor = count - 1,
        KeyCode::Enter if !ctrl => match app.panel {
            0 => toggle_expand(app),
            5 => open_assign_menu(app),
            _ => {}
        },
        KeyCode::Char('/') if !ctrl && !alt && app.panel == 0 => app.inspector_search = true,
        KeyCode::Esc if !ctrl => {
            app.inspector_filter.clear();
            app.inspector_cursor = 0;
        }
        KeyCode::Char('e' | 'E') if !ctrl && !alt && app.panel == 0 => {
            if app.last_trace_id.is_empty() {
                app.status = "还没有可导出的轨迹".into();
            } else {
                let trace_id = app.last_trace_id.clone();
                super::super::export_trace(ctx, &trace_id);
            }
        }
        KeyCode::Char('r' | 'R') if !ctrl && !alt => match app.panel {
            3 => {
                super::super::spawn_request(
                    ctx,
                    "POST",
                    "/api/skills/refresh",
                    Some(serde_json::json!({})),
                    None,
                );
                app.status = "已请求刷新技能清单".into();
            }
            5 => reset_roles_to_default(app, ctx),
            _ => {}
        },
        _ => {}
    }
}

fn toggle_expand(app: &mut App) {
    let cards = trajectory(app);
    let Some(card) = cards.get(app.inspector_cursor) else {
        return;
    };
    let key = card.key().to_owned();
    if !app.inspector_expanded.remove(&key) {
        app.inspector_expanded.insert(key);
    }
}

/// Enter：为当前光标角色改派模型（浮层菜单，含「跟随自动路由」）。
fn open_assign_menu(app: &mut App) {
    let roles = app.roles["roles"]
        .as_object()
        .map(|map| map.keys().map(|key| key.to_owned()).collect::<Vec<_>>());
    let Some(roles) = roles else {
        app.status = "角色清单不可用".into();
        return;
    };
    let Some(tier) = roles.get(app.inspector_cursor.min(roles.len().saturating_sub(1))) else {
        return;
    };
    let tier = tier.clone();
    let mut items = vec![(
        "（跟随自动路由）".to_owned(),
        Action::AssignRole(tier.clone(), String::new()),
    )];
    for model in &app.roles["models"].as_array().cloned().unwrap_or_default() {
        if let Some(id) = model["id"].as_str() {
            items.push((
                id.to_owned(),
                Action::AssignRole(tier.clone(), id.to_owned()),
            ));
        }
    }
    app.overlay = Some(Overlay::Menu {
        title: format!("改派 {tier}"),
        items,
        cursor: 0,
    });
}

/// R：全部任务类型恢复默认模型（default_model；缺失则跟随自动路由）。
fn reset_roles_to_default(app: &mut App, ctx: &Ctx) {
    let default = app.roles["default_model"].as_str().unwrap_or("").to_owned();
    let mut roles = serde_json::Map::new();
    for tier in app.roles["roles"]
        .as_object()
        .map(|map| map.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default()
    {
        roles.insert(tier, serde_json::Value::String(default.clone()));
    }
    super::super::spawn_request(
        ctx,
        "PUT",
        "/api/model-roles",
        Some(serde_json::json!({"roles": roles})),
        None,
    );
    app.status = format!(
        "已请求恢复默认模型 {}",
        if default.is_empty() {
            "（跟随自动）"
        } else {
            &default
        }
    );
}

pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect, focused: bool) {
    let (accent, muted, _) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let title = if app.inspector_search {
        format!(" {} · 搜索:{}▏ ", PANELS[app.panel], app.inspector_filter)
    } else {
        format!(" {} · Tab ", PANELS[app.panel])
    };
    let border = if focused { accent } else { muted };
    let lines = match app.panel {
        0 => trajectory_lines(app, accent, muted),
        1 => budget_lines(app, accent, muted),
        2 => object_lines(app, muted),
        3 => skill_lines(app, muted),
        4 => mcp_lines(app, muted),
        _ => roles_lines(app, accent, muted),
    };
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

fn trajectory_lines(app: &App, accent: Color, muted: Color) -> Vec<Line<'static>> {
    let cards = trajectory(app);
    let cursor = if cards.is_empty() {
        0
    } else {
        app.inspector_cursor.min(cards.len() - 1)
    };
    let mut lines = vec![
        Line::styled(
            format!(
                "log_content:{} · ↑↓ 选择 · Enter 展开 · / 搜索 · E 导出",
                if app.state["log_content"].as_bool().unwrap_or(false) {
                    "开"
                } else {
                    "关（内容脱敏）"
                }
            ),
            Style::default().fg(muted),
        ),
        Line::raw(""),
    ];
    if cards.is_empty() {
        lines.push(Line::styled("暂无调用记录", Style::default().fg(muted)));
        return lines;
    }
    // 卡片总数有上限（模型/工具各 ≤40），直接全量渲染，光标行高亮
    for (index, card) in cards.iter().enumerate() {
        let selected = index == cursor;
        let expanded = app.inspector_expanded.contains(card.key());
        lines.extend(card_lines(card, selected, expanded, accent, muted));
    }
    lines
}

fn card_lines(
    card: &TrajCard<'_>,
    selected: bool,
    expanded: bool,
    accent: Color,
    muted: Color,
) -> Vec<Line<'static>> {
    let marker = |color: Color| {
        vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled("●", Style::default().fg(color)),
        ]
    };
    match card {
        TrajCard::Model(call) => {
            let (color, status) = match call.status {
                ToolStatus::Running => (accent, "…"),
                ToolStatus::Ok => (Color::Green, "✓"),
                ToolStatus::Error => (Color::Red, "✗"),
            };
            let mut first = marker(color);
            first.push(Span::styled(
                format!("model {status} "),
                Style::default().fg(if call.status == ToolStatus::Error {
                    Color::Red
                } else {
                    accent
                }),
            ));
            first.push(Span::styled(call.model_id.clone(), Style::default()));
            if call.turn > 0 {
                first.push(Span::styled(
                    if call.attempt > 1 {
                        format!(" t{}·r{}", call.turn, call.attempt)
                    } else {
                        format!(" t{}", call.turn)
                    },
                    Style::default().fg(muted),
                ));
            }
            if let Some(latency) = call.latency_ms {
                first.push(Span::styled(
                    format!(" {latency}ms"),
                    Style::default().fg(muted),
                ));
            }
            first.push(Span::styled(
                format!(
                    " ↑{} ↓{}",
                    call.input_tokens.unwrap_or(0),
                    call.output_tokens.unwrap_or(0)
                ),
                Style::default().fg(muted),
            ));
            if let Some(cost) = call.cost_usd {
                first.push(Span::styled(
                    format!(" ${cost:.4}"),
                    Style::default().fg(muted),
                ));
            }
            let mut lines = vec![Line::from(first)];
            if expanded {
                if let Some(reason) = &call.finish_reason {
                    lines.push(Line::styled(
                        format!("    finish: {reason}"),
                        Style::default().fg(muted),
                    ));
                }
                if let Some(reserved) = call.reserved_usd {
                    lines.push(Line::styled(
                        format!("    预算预留: ${reserved:.4}"),
                        Style::default().fg(muted),
                    ));
                }
                if let Some(error) = &call.error {
                    lines.push(Line::styled(
                        format!("    错误: {}", widgets::truncate(error, 60)),
                        Style::default().fg(Color::Red),
                    ));
                }
                if let Some(message) = &call.message {
                    for line in message.lines().take(4) {
                        lines.push(Line::styled(
                            format!("    {}", widgets::truncate(line, 70)),
                            Style::default().fg(muted),
                        ));
                    }
                }
                lines.push(Line::raw(""));
            }
            lines
        }
        TrajCard::Tool(card) => {
            let (color, status) = match card.status {
                ToolStatus::Running => (accent, "…"),
                ToolStatus::Ok => (Color::Green, "✓"),
                ToolStatus::Error => (Color::Red, "✗"),
            };
            let mut first = marker(color);
            first.push(Span::styled(
                format!("tool {status} "),
                Style::default().fg(if card.status == ToolStatus::Error {
                    Color::Red
                } else {
                    accent
                }),
            ));
            first.push(Span::styled(card.name.clone(), Style::default()));
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
            if expanded {
                if let Some(result) = &card.result {
                    for line in result.lines().take(4) {
                        lines.push(Line::styled(
                            format!("    {}", widgets::truncate(line, 70)),
                            Style::default().fg(muted),
                        ));
                    }
                }
                lines.push(Line::raw(""));
            }
            lines
        }
    }
}

fn budget_lines(app: &App, accent: Color, muted: Color) -> Vec<Line<'static>> {
    let budget = &app.state["budget"];
    let tokens = budget["tokens"].as_u64().unwrap_or(0);
    let usd = budget["usd"].as_f64().unwrap_or(0.0);
    let usage = &app.progress["usage"];
    let turn = app.progress["turn"].as_u64().unwrap_or(0);
    let max_turns = app.state["limits"]["max_turns"].as_u64().unwrap_or(0);
    let max_concurrent = app.state["limits"]["max_concurrent_tasks"]
        .as_u64()
        .unwrap_or(0);
    vec![
        Line::styled(
            "服务累计",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            Span::raw("  tokens "),
            Span::styled(format!("{tokens}"), Style::default().fg(accent)),
            Span::raw("  ·  累计费用 "),
            Span::styled(format!("${usd:.4}"), Style::default().fg(accent)),
        ]),
        Line::raw(""),
        Line::styled(
            "当前任务",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            Span::raw("  ↑"),
            Span::raw(usage["input_tokens"].as_u64().unwrap_or(0).to_string()),
            Span::raw(" ↓"),
            Span::raw(usage["output_tokens"].as_u64().unwrap_or(0).to_string()),
            Span::raw(" tokens"),
        ]),
        Line::styled(
            if turn > 0 {
                format!(
                    "  第 {turn}{} 轮 · 尝试 {} · 任务费用 ${:.4}",
                    if max_turns > 0 {
                        format!("/{max_turns}")
                    } else {
                        String::new()
                    },
                    app.progress["attempt"].as_u64().unwrap_or(1),
                    app.task_cost_usd
                )
            } else {
                "  （无运行中任务）".to_owned()
            },
            Style::default().fg(muted),
        ),
        Line::raw(""),
        Line::styled(
            format!(
                "服务上限 · 轮次 {} · 并发 {}",
                if max_turns > 0 {
                    max_turns.to_string()
                } else {
                    "∞".into()
                },
                if max_concurrent > 0 {
                    max_concurrent.to_string()
                } else {
                    "∞".into()
                }
            ),
            Style::default().fg(muted),
        ),
    ]
}

fn object_lines(app: &App, muted: Color) -> Vec<Line<'static>> {
    carded_lines(
        app.objects.iter().map(|o| {
            (
                o["kind"].as_str().unwrap_or("object").to_owned(),
                o["id"].as_str().unwrap_or("").to_owned(),
                o["description"].as_str().unwrap_or("").to_owned(),
            )
        }),
        muted,
        "暂无注册对象",
    )
}

fn skill_lines(app: &App, muted: Color) -> Vec<Line<'static>> {
    let mut lines = carded_lines(
        app.skills.iter().map(|s| {
            (
                "skill".to_owned(),
                s["name"]
                    .as_str()
                    .or_else(|| s["id"].as_str())
                    .unwrap_or("")
                    .to_owned(),
                s["description"].as_str().unwrap_or("").to_owned(),
            )
        }),
        muted,
        "暂无技能 · R 刷新",
    );
    lines.push(Line::raw(""));
    lines.push(Line::styled("R 刷新技能清单", Style::default().fg(muted)));
    lines
}

fn mcp_lines(app: &App, muted: Color) -> Vec<Line<'static>> {
    carded_lines(
        app.objects
            .iter()
            .filter(|o| o["kind"].as_str().is_some_and(|k| k.contains("mcp")))
            .map(|o| {
                (
                    o["kind"].as_str().unwrap_or("mcp").to_owned(),
                    o["id"].as_str().unwrap_or("").to_owned(),
                    o["description"].as_str().unwrap_or("").to_owned(),
                )
            }),
        muted,
        "暂无 MCP 对象",
    )
}

/// 对象/技能/MCP 的通用卡片：`● kind 短id` + 描述行。
fn carded_lines(
    items: impl Iterator<Item = (String, String, String)>,
    muted: Color,
    empty: &'static str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut count = 0usize;
    for (kind, id, description) in items {
        count += 1;
        lines.push(Line::from(vec![
            Span::styled("● ", Style::default().fg(muted)),
            Span::styled(kind, Style::default()),
            Span::styled(
                format!(" {}", widgets::short_id(&id)),
                Style::default().fg(muted),
            ),
        ]));
        if !description.is_empty() {
            lines.push(Line::styled(
                format!("  {}", widgets::truncate(&description, 70)),
                Style::default().fg(muted),
            ));
        }
    }
    if count == 0 {
        lines.push(Line::styled(empty, Style::default().fg(muted)));
    }
    lines
}

fn roles_lines(app: &App, accent: Color, muted: Color) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            format!(
                "默认模型: {} · ↑↓ 选择 · Enter 改派 · R 恢复默认",
                app.roles["default_model"].as_str().unwrap_or("自动")
            ),
            Style::default().fg(muted),
        ),
        Line::raw(""),
    ];
    let roles = app.roles["roles"].as_object().cloned().unwrap_or_default();
    if roles.is_empty() {
        lines.push(Line::styled("暂无任务类型", Style::default().fg(muted)));
        return lines;
    }
    let cursor = app.inspector_cursor.min(roles.len().saturating_sub(1));
    for (index, (tier, model)) in roles.iter().enumerate() {
        let selected = index == cursor;
        let model_label = model.as_str().unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(
                tier.clone(),
                if selected {
                    Style::default().fg(accent).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            ),
            Span::raw(" → "),
            Span::styled(
                if model_label.is_empty() {
                    "跟随自动路由".to_owned()
                } else {
                    model_label.to_owned()
                },
                Style::default().fg(if selected { accent } else { muted }),
            ),
        ]));
    }
    lines
}
