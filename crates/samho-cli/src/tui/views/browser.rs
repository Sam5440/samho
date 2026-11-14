//! 浏览器面板（Alt+6）：URL 栏 + 系统浏览器直开；文本模式经 `/api/proxy`
//! 取回净化 HTML 后剥标签，在终端内滚动阅读（终端不渲染网页，属设计内降级）。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde_json::Value;

use crate::tui::{App, Ctx, View, palette, spawn_browser_text, widgets};
use crossterm::event::{KeyCode, KeyEvent};

pub(crate) struct BrowserPanel {
    pub(crate) url: String,
    /// 文本模式正文（剥标签后的纯文本行）
    pub(crate) lines: Vec<String>,
    pub(crate) scroll: u16,
}

/// 系统浏览器直接打开当前 URL。
pub(crate) fn open_external(app: &mut App, url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") {
        if webbrowser::open(url).is_ok() {
            app.status = format!("已在系统浏览器打开 {url}");
        } else {
            app.status = "无法启动系统浏览器".into();
        }
    } else {
        app.status = "URL 需要 http:// 或 https:// 前缀".into();
    }
}

pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    if app.browser.is_none() {
        // 尚未加载过页面：o/u 直接弹 URL 输入
        if matches!(key.code, KeyCode::Char('o') | KeyCode::Char('u')) {
            prompt_url(app);
        }
        return;
    }
    // 滚动类键直接改 scroll（借用短生命周期，逐臂取用避免长借用冲突）
    let scroll_delta: i32 = match key.code {
        KeyCode::Up | KeyCode::Char('k') => -1,
        KeyCode::Down | KeyCode::Char('j') => 1,
        KeyCode::PageUp => -10,
        KeyCode::PageDown => 10,
        KeyCode::Home => {
            if let Some(browser) = app.browser.as_mut() {
                browser.scroll = 0;
            }
            return;
        }
        _ => 0,
    };
    if scroll_delta != 0 {
        if let Some(browser) = app.browser.as_mut() {
            browser.scroll = if scroll_delta < 0 {
                browser.scroll.saturating_sub((-scroll_delta) as u16)
            } else {
                browser.scroll.saturating_add(scroll_delta as u16)
            };
        }
        return;
    }
    match key.code {
        KeyCode::Char('o') => {
            let url = app
                .browser
                .as_ref()
                .map_or(String::new(), |b| b.url.clone());
            open_external(app, &url);
        }
        KeyCode::Char('u') => prompt_url(app),
        KeyCode::Char('t') => {
            let url = app
                .browser
                .as_ref()
                .map_or(String::new(), |b| b.url.clone());
            if !url.is_empty() {
                if let Some(browser) = app.browser.as_mut() {
                    browser.scroll = 0;
                }
                app.status = format!("正在以文本模式读取 {url}…");
                spawn_browser_text(ctx, &url);
            }
        }
        _ => {}
    }
}

fn prompt_url(app: &mut App) {
    app.overlay = Some(crate::tui::Overlay::Prompt {
        title: "输入网址（http/https）".into(),
        input: app
            .browser
            .as_ref()
            .map_or(String::new(), |b| b.url.clone()),
        action: crate::tui::PromptAction::BrowserUrl,
    });
}

/// 剥 HTML 标签为可读文本：去 script/style 块，块级标签换行，解码常见实体。
pub(crate) fn html_to_text(html: &str) -> String {
    let without_blocks = strip_block(html, "script").replace("<!--", "");
    let without_blocks = strip_block(&without_blocks, "style");
    let mut out = String::with_capacity(without_blocks.len());
    let mut rest: &str = &without_blocks;
    // 块级标签 → 换行；其它标签直接剥离
    const BLOCK_TAGS: [&str; 9] = [
        "<p>", "</p>", "<br", "<div", "</div>", "<h1", "<h2", "<h3", "<h4",
    ];
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        if BLOCK_TAGS.iter().any(|tag| {
            // get(..n) 在多字节字符边界处返回 None，避免中文内容里越界切片
            after
                .get(..tag.len())
                .is_some_and(|head| head.eq_ignore_ascii_case(tag))
        }) {
            out.push('\n');
        }
        match after.find('>') {
            Some(end) => rest = &after[end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    decode_entities(&out)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 删除 `<tag …>…</tag>` 整块（大小写不敏感，不含闭合时剥到结尾）。
fn strip_block(html: &str, tag: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    let open_lower = format!("<{tag}");
    let close_lower = format!("</{tag}");
    while let Some(start) = rest.to_ascii_lowercase().find(&open_lower) {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(open_end) = after.find('>') else {
            rest = "";
            break;
        };
        let lower = after.to_ascii_lowercase();
        match lower[open_end..].find(&close_lower) {
            Some(close) => {
                let tail = &after[open_end + close..];
                rest = tail.find('>').map_or("", |gt| &tail[gt + 1..]);
            }
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&copy;", "©")
        .replace("&#169;", "©")
}

/// 浏览器面板渲染：URL 栏 + 文本正文。
pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (accent, muted, background) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let focused = app.view == View::Browser;
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " 浏览器 ",
            Style::default().fg(if focused { accent } else { muted }),
        ))
        .style(Style::default().bg(background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(browser) = &app.browser else {
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled("尚未加载页面", Style::default().fg(muted))),
                Line::raw(""),
                Line::from(Span::styled(
                    "o/u 输入网址（Enter 系统浏览器打开，t 文本模式阅读）",
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                )),
            ]),
            inner,
        );
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled("URL ", Style::default().fg(muted)),
            Span::styled(
                widgets::truncate(&browser.url, inner.width.saturating_sub(6) as usize),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "o 外开 · u 换址 · t 文本模式 · ↑↓ 滚动",
            Style::default().fg(muted),
        )),
        Line::raw(""),
    ];
    if browser.lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "（按 t 以文本模式读取；终端内不渲染网页）",
            Style::default().fg(muted),
        )));
    } else {
        for text in &browser.lines {
            lines.push(Line::from(Span::styled(
                text.clone(),
                Style::default().fg(Color::Reset),
            )));
        }
    }
    let visible = usize::from(inner.height.saturating_sub(3));
    let max_scroll = browser.lines.len().saturating_sub(visible.max(1));
    let scroll = browser
        .scroll
        .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

/// 供 mod.rs 的 PanelKind::BrowserText 分支调用：填充文本行。
pub(crate) fn apply_text(app: &mut App, value: &Value) {
    let url = value["url"].as_str().unwrap_or_default().to_owned();
    let text = value["text"].as_str().unwrap_or_default().to_owned();
    let lines: Vec<String> = text.lines().map(str::to_owned).collect();
    app.browser = Some(BrowserPanel {
        url,
        lines,
        scroll: 0,
    });
    app.status = "文本模式已加载".into();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn html_to_text_strips_tags_and_entities() {
        let html = "<html><head><script>var x = 1 < 2;</script><style>p{color:red}</style></head>\
            <body><h1>标题</h1><p>第一段 &amp; 第二段</p><div>块级</div><span>行内</span></body></html>";
        let text = html_to_text(html);
        assert!(text.contains("标题"), "应保留标题文本：{text}");
        assert!(text.contains("第一段 & 第二段"), "应解码实体：{text}");
        assert!(text.contains("块级"));
        assert!(text.contains("行内"));
        assert!(!text.contains("var x"), "script 内容应删除：{text}");
        assert!(!text.contains("color"), "style 内容应删除：{text}");
        assert!(!text.contains('<'), "不应残留标签：{text}");
    }

    #[test]
    fn render_with_and_without_page() {
        let mut app = App {
            view: View::Browser,
            ..App::default()
        };
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 20)).unwrap();
        let area = ratatui::layout::Rect::new(0, 0, 90, 20);
        terminal.draw(|frame| render(frame, &app, area)).unwrap();
        apply_text(
            &mut app,
            &json!({"url": "https://example.com", "text": "第一行\n第二行\n第三行"}),
        );
        assert_eq!(app.browser.as_ref().unwrap().lines.len(), 3);
        terminal.draw(|frame| render(frame, &app, area)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
            .replace(' ', "");
        assert!(content.contains("https://example.com"));
        assert!(content.contains("第二行"), "buffer: {content}");
    }
}
