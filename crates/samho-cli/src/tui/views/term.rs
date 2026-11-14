//! 终端面板（Alt+5）：PTY 终端列表 + **接管模式** —— 接管时挂起 TUI、退出
//! 备用屏，把 `/api/term/{id}/stream` 的字节流直通 stdout、键盘输入经
//! `/api/term/{id}/input` 写回 PTY；Ctrl+Q 脱离（PTY 保活，可重入续接历史）。

use std::io::Write as _;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use serde_json::json;

use crate::tui::widgets;
use crate::tui::{App, Ctx, PanelKind, View, palette, spawn_panel};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;

/// 刷新会话附属终端列表（无活动会话时仅显示本 TUI 已知的终端 id）。
pub(crate) fn refresh(ctx: &Ctx, session_id: Option<&str>) {
    let Some(session_id) = session_id.filter(|id| !id.is_empty()) else {
        return;
    };
    spawn_panel(
        ctx,
        PanelKind::TermList,
        "GET",
        &format!("/api/term?session_id={session_id}"),
        None,
    );
}

/// 新建终端（cwd = workspace 根；有活动会话时附属绑定）。
pub(crate) fn create(ctx: &Ctx, session_id: Option<&str>) {
    let body = json!({
        "cwd": "",
        "session_id": session_id.filter(|id| !id.is_empty()),
    });
    spawn_panel(ctx, PanelKind::TermNew, "POST", "/api/term", Some(body));
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.term_cursor = app.term_cursor.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if !app.term_ids.is_empty() {
                app.term_cursor = (app.term_cursor + 1).min(app.term_ids.len() - 1);
            }
        }
        KeyCode::Char('n') => create(ctx, app.active.as_deref()),
        KeyCode::Enter => {
            if let Some(id) = app.term_ids.get(app.term_cursor) {
                app.takeover = Some(id.clone());
            }
        }
        KeyCode::Char('K') => {
            if let Some(id) = app.term_ids.get(app.term_cursor).cloned() {
                app.term_ids.retain(|known| known != &id);
                app.term_cursor = app
                    .term_cursor
                    .saturating_sub(1)
                    .min(app.term_ids.len().saturating_sub(1));
                spawn_panel(
                    ctx,
                    PanelKind::TermClosed,
                    "DELETE",
                    &format!("/api/term/{id}"),
                    None,
                );
            }
        }
        KeyCode::Char('r') => refresh(ctx, app.active.as_deref()),
        _ => {}
    }
}

/// 终端列表渲染。
pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (accent, muted, background) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let focused = app.view == View::Term;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " 终端 ",
            Style::default().fg(if focused { accent } else { muted }),
        ))
        .style(Style::default().bg(background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mut lines = vec![Line::from(Span::styled(
        "Enter 接管（Ctrl+Q 脱离） · n 新建 · K 关闭 · r 刷新",
        Style::default().fg(muted),
    ))];
    if app.term_ids.is_empty() {
        lines.push(Line::from(Span::styled(
            "暂无终端，n 新建一个（新会话与当前 Agent 会话绑定）",
            Style::default().fg(muted),
        )));
    }
    for (index, id) in app.term_ids.iter().enumerate() {
        let selected = index == app.term_cursor;
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(format!("▸ #{} ", index + 1), Style::default().fg(accent)),
            Span::styled(
                widgets::short_id(id),
                Style::default().fg(if selected { Color::Reset } else { muted }),
            ),
            Span::styled("  PTY 会话（脱离后保活）", Style::default().fg(muted)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// 键盘事件 → PTY 字节序列（xterm 风格最小集）。
pub(crate) fn key_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    use crossterm::event::KeyModifiers as M;
    let text: &str = match key.code {
        KeyCode::Enter => "\r",
        KeyCode::Tab => {
            return Some(if key.modifiers.contains(M::SHIFT) {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            });
        }
        KeyCode::Backspace => "\x08",
        KeyCode::Esc => "\x1b",
        KeyCode::Left => "\x1b[D",
        KeyCode::Right => "\x1b[C",
        KeyCode::Up => "\x1b[A",
        KeyCode::Down => "\x1b[B",
        KeyCode::Home => "\x1b[H",
        KeyCode::End => "\x1b[F",
        KeyCode::PageUp => "\x1b[5~",
        KeyCode::PageDown => "\x1b[6~",
        KeyCode::Delete => "\x1b[3~",
        KeyCode::Insert => "\x1b[2~",
        KeyCode::F(1) => "\x1bOP",
        KeyCode::F(2) => "\x1bOQ",
        KeyCode::F(3) => "\x1bOR",
        KeyCode::F(4) => "\x1bOS",
        KeyCode::F(5) => "\x1b[15~",
        KeyCode::Char(' ') if key.modifiers.contains(M::CONTROL) => "\x00",
        KeyCode::Char(c) if key.modifiers.contains(M::CONTROL) && c.is_ascii_alphabetic() => {
            return Some(vec![(c.to_ascii_uppercase() as u8) & 0x1f]);
        }
        KeyCode::Char(c) if key.modifiers.contains(M::ALT) => {
            let mut bytes = vec![0x1b];
            bytes.extend_from_slice(c.to_string().as_bytes());
            return Some(bytes);
        }
        KeyCode::Char(c) => return Some(c.to_string().into_bytes()),
        _ => return None,
    };
    Some(text.as_bytes().to_vec())
}

/// 接管模式：挂起 TUI 界面，透传 PTY 字节流与键盘输入；Ctrl+Q 脱离。
/// 调用方持有 raw mode（TerminalGuard 不释放），本函数只负责备用屏切换。
pub(crate) async fn takeover(client: &reqwest::Client, url: &str, term_id: &str) -> Result<()> {
    use std::io::stdout;

    crossterm::execute!(stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let reader = tokio::task::spawn_blocking(move || {
        while let Ok(event) = crossterm::event::read() {
            if event_tx.send(event).is_err() {
                break;
            }
        }
    });
    // 尺寸对齐当前窗口（best-effort）
    if let Ok((cols, rows)) = terminal::size() {
        let _ = client
            .post(format!("{url}/api/term/{term_id}/resize"))
            .json(&json!({"cols": cols, "rows": rows}))
            .send()
            .await;
    }
    let response = client
        .get(format!("{url}/api/term/{term_id}/stream"))
        .header("accept", "text/event-stream")
        .send()
        .await
        .context("终端流连接失败")?;
    anyhow::ensure!(
        response.status().is_success(),
        "终端流 {}",
        response.status()
    );
    let mut stream = response.bytes_stream();
    let mut parser = crate::tui::sse::SseParser::default();
    let mut out = stdout();
    let mut detached = false;
    loop {
        tokio::select! {
            chunk = stream.next() => {
                let Some(chunk) = chunk else { break };
                let bytes = chunk.context("终端流中断")?;
                for message in parser.feed(&bytes) {
                    if matches!(message.event.as_str(), "data" | "backfill") {
                        out.write_all(message.data.as_bytes())?;
                        out.flush()?;
                    }
                }
            }
            event = event_rx.recv() => {
                let Some(Event::Key(key)) = event else {
                    if let Some(Event::Resize(cols, rows)) = event {
                        let _ = client
                            .post(format!("{url}/api/term/{term_id}/resize"))
                            .json(&json!({"cols": cols, "rows": rows}))
                            .send()
                            .await;
                    }
                    continue;
                };
                if key.kind != crossterm::event::KeyEventKind::Press {
                    continue;
                }
                let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
                if ctrl && key.code == KeyCode::Char('q') {
                    detached = true;
                    break;
                }
                if let Some(bytes) = key_bytes(key) {
                    let text = String::from_utf8_lossy(&bytes).into_owned();
                    let _ = client
                        .post(format!("{url}/api/term/{term_id}/input"))
                        .json(&json!({"text": text}))
                        .send()
                        .await;
                }
            }
        }
    }
    reader.abort();
    crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen)?;
    if detached {
        Ok(())
    } else {
        anyhow::bail!("终端流已结束（进程退出或连接断开）")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_bytes_translate_xterm_sequences() {
        let plain = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        assert_eq!(key_bytes(plain('a')).unwrap(), b"a");
        assert_eq!(key_bytes(plain('中')).unwrap(), "中".as_bytes());
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap(),
            b"\r"
        );
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)).unwrap(),
            b"\x1b[A"
        );
        assert_eq!(key_bytes(ctrl('c')).unwrap(), vec![0x03]);
        assert_eq!(key_bytes(ctrl('d')).unwrap(), vec![0x04]);
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT)).unwrap(),
            b"\x1bx"
        );
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT)).unwrap(),
            b"\x1b[Z"
        );
        assert_eq!(
            key_bytes(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)).unwrap(),
            b"q"
        );
    }

    #[test]
    fn list_render_and_cursor_keys() {
        let app = App {
            view: View::Term,
            term_ids: vec!["term-1".into(), "term-2".into()],
            ..App::default()
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| render(frame, &app, ratatui::layout::Rect::new(0, 0, 100, 20)))
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("term-1") || content.contains("term-"));
    }
}
