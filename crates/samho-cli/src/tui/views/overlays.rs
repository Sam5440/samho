//! 模态浮层：单行输入（新建项目/重命名）、确认（删除）、单选菜单（行操作/
//! 移动项目/模型选择）与技能多选。浮层打开时独占键位，Esc 关闭。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use crate::tui::{Action, App, Ctx, Overlay, PromptAction, palette};

/// 技能名列表（去 "skill:" 前缀；与提交时的 skills 字段一致）。
pub(crate) fn skill_names(app: &App) -> Vec<String> {
    app.skills
        .iter()
        .filter_map(|s| s["name"].as_str().or_else(|| s["id"].as_str()))
        .map(|name| name.trim_start_matches("skill:").to_owned())
        .collect()
}

/// Ctrl+M 模型选择菜单（自动 + 具体模型，选中态对齐 web model-picker）。
pub(crate) fn model_menu(app: &App) -> Overlay {
    let mut items = vec![("自动（按层级路由）".to_owned(), Action::PickModel(0))];
    for (index, model) in app.models.iter().enumerate() {
        if let Some(id) = model["id"].as_str() {
            items.push((id.to_owned(), Action::PickModel(index + 1)));
        }
    }
    let cursor = app.selected_model.min(items.len().saturating_sub(1));
    Overlay::Menu {
        title: "选择模型".into(),
        items,
        cursor,
    }
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let Some(overlay) = app.overlay.take() else {
        return;
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match overlay {
        Overlay::Prompt {
            title,
            mut input,
            action,
        } => match key.code {
            KeyCode::Enter if !ctrl => {
                let text = std::mem::take(&mut input);
                apply_prompt(app, ctx, action, text);
            }
            KeyCode::Esc if !ctrl => {} // 关闭
            KeyCode::Backspace => {
                input.pop();
                app.overlay = Some(Overlay::Prompt {
                    title,
                    input,
                    action,
                });
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                input.push(c);
                app.overlay = Some(Overlay::Prompt {
                    title,
                    input,
                    action,
                });
            }
            _ => {
                app.overlay = Some(Overlay::Prompt {
                    title,
                    input,
                    action,
                })
            }
        },
        Overlay::Confirm { title, accept } => match key.code {
            KeyCode::Enter => perform(app, ctx, accept),
            KeyCode::Esc | KeyCode::Char('n' | 'N') if !ctrl && !alt => {} // 关闭
            _ => app.overlay = Some(Overlay::Confirm { title, accept }),
        },
        Overlay::Menu {
            title,
            items,
            mut cursor,
        } => {
            let count = items.len();
            match key.code {
                KeyCode::Up | KeyCode::Char('k') if !ctrl && !alt && count > 0 => {
                    cursor = cursor.saturating_sub(1);
                    app.overlay = Some(Overlay::Menu {
                        title,
                        items,
                        cursor,
                    });
                }
                KeyCode::Down | KeyCode::Char('j') if !ctrl && !alt && count > 0 => {
                    cursor = (cursor + 1).min(count - 1);
                    app.overlay = Some(Overlay::Menu {
                        title,
                        items,
                        cursor,
                    });
                }
                KeyCode::Home if !ctrl && count > 0 => {
                    app.overlay = Some(Overlay::Menu {
                        title,
                        items,
                        cursor: 0,
                    });
                }
                KeyCode::End if !ctrl && count > 0 => {
                    app.overlay = Some(Overlay::Menu {
                        title,
                        items,
                        cursor: count - 1,
                    });
                }
                KeyCode::Enter if !ctrl => {
                    if let Some((_, action)) = items.into_iter().nth(cursor) {
                        perform(app, ctx, action);
                    }
                }
                KeyCode::Esc if !ctrl => {} // 关闭
                _ => {
                    app.overlay = Some(Overlay::Menu {
                        title,
                        items,
                        cursor,
                    })
                }
            }
        }
        Overlay::Skills { mut cursor } => {
            let names = skill_names(app);
            let count = names.len();
            match key.code {
                KeyCode::Up | KeyCode::Char('k') if !ctrl && !alt && count > 0 => {
                    cursor = cursor.saturating_sub(1);
                    app.overlay = Some(Overlay::Skills { cursor });
                }
                KeyCode::Down | KeyCode::Char('j') if !ctrl && !alt && count > 0 => {
                    cursor = (cursor + 1).min(count - 1);
                    app.overlay = Some(Overlay::Skills { cursor });
                }
                KeyCode::Char(' ') if !ctrl => {
                    if let Some(name) = names.get(cursor)
                        && !app.selected_skills.remove(name)
                    {
                        app.selected_skills.insert(name.clone());
                    }
                    app.overlay = Some(Overlay::Skills { cursor });
                }
                KeyCode::Enter | KeyCode::Esc if !ctrl => {} // 关闭，勾选已生效
                _ => app.overlay = Some(Overlay::Skills { cursor }),
            }
        }
    }
}

/// 输入浮层提交：空内容拒绝；成功后关闭浮层并发起对应请求。
fn apply_prompt(app: &mut App, ctx: &Ctx, action: PromptAction, text: String) {
    let value = text.trim().to_owned();
    if value.is_empty() {
        app.status = "内容不能为空".into();
        return;
    }
    app.overlay = None;
    match action {
        PromptAction::NewProject => {
            super::super::spawn_request(
                ctx,
                "POST",
                "/api/projects",
                Some(json!({"name": value})),
                None,
            );
            app.status = "已请求创建项目".into();
        }
        PromptAction::RenameSession(id) => {
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/sessions/{id}"),
                Some(json!({"title": value})),
                None,
            );
            app.status = "已请求重命名会话".into();
        }
        PromptAction::RenameProject(id) => {
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/projects/{id}"),
                Some(json!({"name": value})),
                None,
            );
            app.status = "已请求重命名项目".into();
        }
    }
}

/// 菜单/确认动作分发：网络动作直接发起，二次动作（重命名、删除）先打开
/// 下级浮层再确认。
fn perform(app: &mut App, ctx: &Ctx, action: Action) {
    app.overlay = None;
    match action {
        Action::OpenSession(id) => super::super::open_session(app, ctx, &id),
        Action::RenameSession(id) => {
            let title = app
                .sessions
                .iter()
                .find(|s| s["id"].as_str() == Some(id.as_str()))
                .and_then(|s| s["title"].as_str())
                .unwrap_or("")
                .to_owned();
            app.overlay = Some(Overlay::Prompt {
                title: "重命名会话".into(),
                input: title,
                action: PromptAction::RenameSession(id),
            });
        }
        Action::ForkSession(id) => {
            app.transcript_version += 1;
            super::super::spawn_request(
                ctx,
                "POST",
                &format!("/api/sessions/{id}/fork"),
                Some(json!({})),
                Some(app.transcript_version),
            );
            app.status = "已请求创建分支".into();
        }
        Action::ArchiveSession(id, flag) => {
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/sessions/{id}"),
                Some(json!({"archived": flag})),
                None,
            );
            app.status = if flag {
                "已请求归档会话"
            } else {
                "已请求取消归档"
            }
            .into();
        }
        Action::MoveSession(id) => {
            let mut items = vec![("未分类".to_owned(), Action::AssignSession(id.clone(), None))];
            for p in &app.projects {
                if let (Some(pid), Some(name)) = (p["id"].as_str(), p["name"].as_str()) {
                    items.push((
                        name.to_owned(),
                        Action::AssignSession(id.clone(), Some(pid.to_owned())),
                    ));
                }
            }
            app.overlay = Some(Overlay::Menu {
                title: "移动到项目".into(),
                items,
                cursor: 0,
            });
        }
        Action::AssignSession(id, target) => {
            // None 序列化为 null → 服务端把会话移出项目（与 web 一致）
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/sessions/{id}"),
                Some(json!({"project_id": target})),
                None,
            );
            app.status = "已更新会话归属".into();
        }
        Action::DeleteSession(id) => {
            app.overlay = Some(Overlay::Confirm {
                title: format!("删除会话 {}？不可恢复", crate::tui::widgets::short_id(&id)),
                accept: Action::DeleteSessionNow(id),
            });
        }
        Action::DeleteSessionNow(id) => {
            super::super::spawn_request(ctx, "DELETE", &format!("/api/sessions/{id}"), None, None);
            if app.session_id.as_deref() == Some(id.as_str()) {
                super::super::reset_draft(app);
            }
            app.status = "已请求删除会话".into();
        }
        Action::RenameProject(id) => {
            let name = app
                .projects
                .iter()
                .find(|p| p["id"].as_str() == Some(id.as_str()))
                .and_then(|p| p["name"].as_str())
                .unwrap_or("")
                .to_owned();
            app.overlay = Some(Overlay::Prompt {
                title: "重命名项目".into(),
                input: name,
                action: PromptAction::RenameProject(id),
            });
        }
        Action::ToggleProject(id, open) => {
            if let Some(p) = app
                .projects
                .iter_mut()
                .find(|p| p["id"].as_str() == Some(id.as_str()))
            {
                p["open"] = json!(open);
            }
            super::super::spawn_request(
                ctx,
                "PATCH",
                &format!("/api/projects/{id}"),
                Some(json!({"open": open})),
                None,
            );
        }
        Action::DeleteProject(id) => {
            app.overlay = Some(Overlay::Confirm {
                title: "删除项目？其中会话将回到未分类".into(),
                accept: Action::DeleteProjectNow(id),
            });
        }
        Action::DeleteProjectNow(id) => {
            if app.pending_project_id.as_deref() == Some(id.as_str()) {
                app.pending_project_id = None;
            }
            super::super::spawn_request(ctx, "DELETE", &format!("/api/projects/{id}"), None, None);
            app.status = "已请求删除项目".into();
        }
        Action::AssignRole(tier, model) => {
            // 以当前快照为基础只改派一个角色，其余保持不变
            let mut roles = app.roles["roles"].as_object().cloned().unwrap_or_default();
            roles.insert(tier.clone(), serde_json::Value::String(model));
            super::super::spawn_request(
                ctx,
                "PUT",
                "/api/model-roles",
                Some(json!({"roles": roles})),
                None,
            );
            app.status = format!("已请求改派任务类型 {tier}");
        }
        Action::PickModel(index) => {
            app.selected_model = index;
            let label = app
                .models
                .get(index.saturating_sub(1))
                .and_then(|m| m["id"].as_str())
                .unwrap_or("自动（按层级路由）");
            app.status = format!("已选择模型 {label}");
        }
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(12);
    let height = height.min(area.height.saturating_sub(2)).max(3);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let Some(overlay) = app.overlay.as_ref() else {
        return;
    };
    let (accent, muted, _) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    match overlay {
        Overlay::Prompt {
            title,
            input,
            action: _,
        } => {
            let rect = centered(area, 56, 5);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::raw(input.clone()),
                    Line::raw(""),
                    Line::styled("Enter 确认 · Esc 取消", Style::default().fg(muted)),
                ])
                .block(
                    Block::default()
                        .title(format!(" {title} "))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(accent)),
                ),
                rect,
            );
            let cursor_x = rect.x
                + 1
                + u16::try_from(input.width())
                    .unwrap_or(u16::MAX)
                    .min(rect.width.saturating_sub(2));
            frame.set_cursor_position((cursor_x, rect.y + 1));
        }
        Overlay::Confirm { title, accept: _ } => {
            let rect = centered(area, 56, 5);
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(title.clone(), Style::default().fg(Color::Red)),
                    Line::raw(""),
                    Line::styled("Enter 确认 · n/Esc 取消", Style::default().fg(muted)),
                ])
                .block(
                    Block::default()
                        .title(" 确认操作 ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Red)),
                ),
                rect,
            );
        }
        Overlay::Menu {
            title,
            items,
            cursor,
        } => {
            let visible = usize::from(area.height.saturating_sub(4)).max(1);
            let offset = if *cursor >= visible {
                *cursor + 1 - visible
            } else {
                0
            };
            let lines: Vec<Line<'static>> = items
                .iter()
                .skip(offset)
                .take(visible)
                .enumerate()
                .map(|(index, (label, _))| {
                    let selected = offset + index == *cursor;
                    Line::from(vec![
                        Span::styled(
                            if selected { "▸ " } else { "  " },
                            Style::default().fg(accent),
                        ),
                        Span::styled(
                            label.clone(),
                            if selected {
                                Style::default().fg(accent).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                    ])
                })
                .chain(std::iter::once(Line::styled(
                    "↑↓ 选择 · Enter 执行 · Esc 关闭",
                    Style::default().fg(muted),
                )))
                .collect();
            let height = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            let rect = centered(area, 60, height);
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .title(format!(" {title} "))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(accent)),
                ),
                rect,
            );
        }
        Overlay::Skills { cursor } => {
            let names = skill_names(app);
            let visible = usize::from(area.height.saturating_sub(5)).max(1);
            let offset = if *cursor >= visible {
                *cursor + 1 - visible
            } else {
                0
            };
            let mut lines: Vec<Line<'static>> = names
                .iter()
                .skip(offset)
                .take(visible)
                .enumerate()
                .map(|(index, name)| {
                    let selected = offset + index == *cursor;
                    let checked = app.selected_skills.contains(name);
                    let style = if selected {
                        Style::default().fg(accent).add_modifier(Modifier::BOLD)
                    } else if checked {
                        Style::default().fg(accent)
                    } else {
                        Style::default()
                    };
                    Line::from(vec![
                        Span::styled(
                            if selected { "▸ " } else { "  " },
                            Style::default().fg(accent),
                        ),
                        Span::styled(if checked { "☑ " } else { "☐ " }, style),
                        Span::styled(name.clone(), style),
                    ])
                })
                .collect();
            if names.is_empty() {
                lines.push(Line::styled("（无可用技能）", Style::default().fg(muted)));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "Space 勾选 · Enter 完成 · Esc 关闭",
                Style::default().fg(muted),
            ));
            let height = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_add(2);
            let rect = centered(area, 52, height);
            frame.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .title(format!(" 技能多选（已选 {}） ", app.selected_skills.len()))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(accent)),
                ),
                rect,
            );
        }
    }
}
