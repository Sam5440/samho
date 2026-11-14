//! 文件面板（Alt+3）：`/api/fs` 懒加载目录列表 + 文本内容查看（Markdown 渲染
//! 切换）+ 自建多行编辑器（Ctrl+S 保存），支持新建/改名/删除/设根/系统浏览器
//! 预览 HTML 与图片。

use anyhow::Context as _;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use serde_json::{Value, json};

use crate::tui::{
    App, Ctx, PanelKind, View, markdown, palette, spawn_panel, urlencode_query, widgets,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 文件面板整体状态（列出 / 查看 / 编辑三种模式）。
pub(crate) struct FilesPanel {
    /// 树根（相对 workspace 的路径；"" = workspace 根）
    pub(crate) root: String,
    /// 当前目录（相对 root）
    pub(crate) cwd: String,
    pub(crate) entries: Vec<Value>,
    pub(crate) truncated: bool,
    pub(crate) cursor: usize,
    pub(crate) mode: FileMode,
}

/// 文件内容/编辑态（列表模式下为 None）。
pub(crate) enum FileMode {
    Listing,
    Content {
        path: String,
        content: String,
        markdown: bool,
        scroll: u16,
    },
    Editor {
        path: String,
        lines: Vec<String>,
        row: usize,
        col: usize,
    },
    /// 二进制/图片文件：仅提示（Enter/o 用系统浏览器打开 raw 地址）
    Binary {
        path: String,
    },
}

impl Default for FilesPanel {
    fn default() -> Self {
        Self {
            root: String::new(),
            cwd: String::new(),
            entries: Vec::new(),
            truncated: false,
            cursor: 0,
            mode: FileMode::Listing,
        }
    }
}

impl FilesPanel {
    fn selected(&self) -> Option<&Value> {
        self.entries.get(self.cursor)
    }

    fn cwd_join(&self, name: &str) -> String {
        if self.cwd.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{}", self.cwd, name)
        }
    }
}

/// 当前光标相对路径（列表模式）。目录以 `/` 结尾返回。
pub(crate) fn selected_path(files: &FilesPanel) -> Option<String> {
    let entry = files.selected()?;
    let name = entry["name"].as_str()?;
    let path = files.cwd_join(name);
    Some(if entry["kind"] == "dir" {
        format!("{path}/")
    } else {
        path
    })
}

fn join_relative(base: &str, name: &str) -> String {
    let name = name.trim().trim_matches('/');
    if name.is_empty() || name == "." {
        return base.to_owned();
    }
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    }
}

/// 重新加载当前目录列表。
pub(crate) fn refresh(ctx: &Ctx, root: &str, cwd: &str) {
    let path = if cwd.is_empty() {
        root.to_owned()
    } else {
        join_relative(root, cwd)
    };
    let query = if path.is_empty() {
        "/api/fs".to_owned()
    } else {
        format!("/api/fs?path={}", urlencode_query(&path))
    };
    spawn_panel(ctx, PanelKind::Files, "GET", &query, None);
}

/// 打开文件（文本读取；二进制标记交给 FileContent 分支处理）。
fn open_file(ctx: &Ctx, path: &str) {
    spawn_panel(
        ctx,
        PanelKind::FileContent,
        "GET",
        &format!("/api/file?path={}", urlencode_query(path)),
        None,
    );
}

/// 图片/二进制：直接把 raw 端点丢给系统浏览器。
fn open_raw_externally(app: &mut App, path: &str) {
    let raw = format!(
        "{}/api/file/raw?path={}",
        app.base_url.trim_end_matches('/'),
        urlencode_query(path)
    );
    if webbrowser::open(&raw).is_ok() {
        app.status = format!("已在系统浏览器打开 {path}");
    } else {
        app.status = "无法启动系统浏览器".into();
    }
}

/// HTML 预览：读内容 → POST /api/preview → 沙箱地址交给系统浏览器。
fn preview_html(app: &App, ctx: &Ctx, path: &str) {
    let client = ctx.client.clone();
    let url = ctx.url.to_owned();
    let sender = ctx.sender.clone();
    let base_url = app.base_url.clone();
    let target = path.to_owned();
    tokio::spawn(async move {
        let result = async {
            let value = crate::tui::api::fetch(
                &client,
                &url,
                &format!("/api/file?path={}", urlencode_query(&target)),
            )
            .await?;
            let html = value["content"].as_str().context("文件不可读")?;
            let created: Value = client
                .post(format!("{url}/api/preview"))
                .json(&json!({"html": html}))
                .send()
                .await?
                .json()
                .await?;
            let id = created["id"].as_str().context("预览创建失败")?;
            anyhow::Ok(format!("{base_url}/api/preview/{id}"))
        }
        .await;
        let message = match result {
            Ok(preview) => {
                let _ = webbrowser::open(&preview);
                format!("已在系统浏览器预览 {target}")
            }
            Err(error) => format!("预览失败：{error}"),
        };
        let _ = sender.send(crate::tui::Update::Notice(message)).await;
    });
}

/// 面板键位：Listing / Content / Editor 三态分发。
pub(crate) fn handle_key(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match &mut app.files.mode {
        FileMode::Listing => handle_listing(app, ctx, key),
        FileMode::Content { .. } => handle_content(app, ctx, key, ctrl),
        FileMode::Editor { .. } => handle_editor(app, ctx, key, ctrl),
        FileMode::Binary { path } => {
            let path = path.clone();
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
                    app.files.mode = FileMode::Listing;
                }
                KeyCode::Enter | KeyCode::Char('o') => open_raw_externally(app, &path),
                _ => {}
            }
        }
    }
}

fn handle_listing(app: &mut App, ctx: &Ctx, key: KeyEvent) {
    let files = &mut app.files;
    let count = files.entries.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => files.cursor = files.cursor.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if count > 0 {
                files.cursor = (files.cursor + 1).min(count - 1);
            }
        }
        KeyCode::Home => files.cursor = 0,
        KeyCode::End => files.cursor = count.saturating_sub(1),
        KeyCode::Backspace => {
            // 返回上级（root 内）
            if let Some(parent) = files.cwd.rsplit_once('/') {
                files.cwd = parent.0.to_owned();
            } else {
                files.cwd = String::new();
            }
            files.cursor = 0;
            refresh(ctx, &files.root.clone(), &files.cwd.clone());
        }
        KeyCode::Enter => {
            let Some(entry) = files.selected().cloned() else {
                return;
            };
            let kind = entry["kind"].as_str().unwrap_or("file");
            let name = entry["name"].as_str().unwrap_or_default().to_owned();
            if kind == "dir" {
                files.cwd = files.cwd_join(&name);
                files.cursor = 0;
                refresh(ctx, &files.root.clone(), &files.cwd.clone());
                return;
            }
            let path = files.cwd_join(&name);
            let lower = name.to_ascii_lowercase();
            if ["png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "bmp"]
                .iter()
                .any(|ext| lower.ends_with(ext))
            {
                open_raw_externally(app, &path);
            } else if ["html", "htm"].iter().any(|ext| lower.ends_with(ext)) {
                preview_html(app, ctx, &path);
            } else {
                open_file(ctx, &path);
            }
        }
        KeyCode::Char('o') => {
            if let Some(path) = selected_path(files) {
                open_raw_externally(app, path.trim_end_matches('/'));
            }
        }
        KeyCode::Char('m') => {
            if let Some(path) = selected_path(files) {
                open_file(ctx, path.trim_end_matches('/'));
            }
        }
        KeyCode::Char('p') => {
            if let Some(path) = selected_path(files) {
                preview_html(app, ctx, path.trim_end_matches('/'));
            }
        }
        KeyCode::Char('s') => {
            files.root = files.cwd.clone();
            app.status = if files.root.is_empty() {
                "树根已重置为 workspace 根".into()
            } else {
                format!("树根已设为 {}", files.root)
            };
        }
        KeyCode::Char('n') => {
            // 新建文件
            app.overlay = Some(crate::tui::Overlay::Prompt {
                title: "新建文件（相对当前目录）".into(),
                input: String::new(),
                action: crate::tui::PromptAction::NewFile(app.files.cwd.clone()),
            });
        }
        KeyCode::Char('N') => {
            app.overlay = Some(crate::tui::Overlay::Prompt {
                title: "新建目录（相对当前目录）".into(),
                input: String::new(),
                action: crate::tui::PromptAction::NewDir(app.files.cwd.clone()),
            });
        }
        KeyCode::Char('r') => {
            if let Some(path) = selected_path(files) {
                let name = path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .to_owned();
                app.overlay = Some(crate::tui::Overlay::Prompt {
                    title: format!("重命名为（原名 {name}）"),
                    input: name,
                    action: crate::tui::PromptAction::RenamePath(
                        path.trim_end_matches('/').to_owned(),
                    ),
                });
            }
        }
        KeyCode::Char('d') => {
            if let Some(path) = selected_path(files) {
                let target = path.trim_end_matches('/').to_owned();
                app.overlay = Some(crate::tui::Overlay::Confirm {
                    title: format!("确认删除 {}？", widgets::truncate(&target, 40)),
                    accept: crate::tui::Action::DeleteFile(target),
                });
            }
        }
        _ => {}
    }
}

fn handle_content(app: &mut App, _ctx: &Ctx, key: KeyEvent, ctrl: bool) {
    let FileMode::Content {
        path,
        content,
        markdown,
        scroll,
    } = &mut app.files.mode
    else {
        return;
    };
    let path = path.clone();
    match (key.code, ctrl) {
        (KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q'), _) => {
            app.files.mode = FileMode::Listing;
        }
        (KeyCode::Char('m') | KeyCode::Char('M'), _) => {
            *markdown = !*markdown;
            *scroll = 0;
        }
        (KeyCode::Char('e'), false) => {
            // 进入编辑器：按行拆分（去掉 \r，保存时统一 \n）
            let lines: Vec<String> = content
                .split('\n')
                .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
                .collect();
            app.files.mode = FileMode::Editor {
                path,
                lines,
                row: 0,
                col: 0,
            };
        }
        (KeyCode::Up | KeyCode::Char('k'), false) => *scroll = scroll.saturating_sub(1),
        (KeyCode::Down | KeyCode::Char('j'), false) => *scroll = scroll.saturating_add(1),
        (KeyCode::PageUp, _) => *scroll = scroll.saturating_sub(10),
        (KeyCode::PageDown, _) => *scroll = scroll.saturating_add(10),
        (KeyCode::Home, _) => *scroll = 0,
        _ => {}
    }
}

fn handle_editor(app: &mut App, ctx: &Ctx, key: KeyEvent, ctrl: bool) {
    // Ctrl+S 保存
    if ctrl && key.code == KeyCode::Char('s') {
        let FileMode::Editor { path, lines, .. } = &app.files.mode else {
            return;
        };
        let path = path.clone();
        let content = lines.join("\n");
        spawn_panel(
            ctx,
            PanelKind::FileSaved,
            "PUT",
            "/api/file",
            Some(json!({"path": path, "content": content})),
        );
        app.status = format!("已保存 {}", path);
        return;
    }
    let (line_count, content_len) = match &app.files.mode {
        FileMode::Editor { lines, row, .. } => (lines.len(), lines[*row].chars().count()),
        _ => return,
    };
    let mut row_delta = 0i32;
    let mut new_col: Option<usize> = None;
    match key.code {
        KeyCode::Up => row_delta = -1,
        KeyCode::Down => row_delta = 1,
        KeyCode::Left => {
            if let FileMode::Editor { col, .. } = &mut app.files.mode {
                *col = col.saturating_sub(1);
            }
        }
        KeyCode::Right => {
            if let FileMode::Editor {
                col, row, lines, ..
            } = &mut app.files.mode
            {
                let len = lines[*row].chars().count();
                *col = (*col + 1).min(len);
            }
        }
        KeyCode::Home => new_col = Some(0),
        KeyCode::End => new_col = Some(content_len),
        KeyCode::PageUp => row_delta = -10,
        KeyCode::PageDown => row_delta = 10,
        KeyCode::Backspace => {
            if let FileMode::Editor {
                lines, row, col, ..
            } = &mut app.files.mode
            {
                let mut chars: Vec<char> = lines[*row].chars().collect();
                if *col > 0 {
                    chars.remove(*col - 1);
                    *col -= 1;
                    lines[*row] = chars.into_iter().collect();
                } else if *row > 0 {
                    // 与上一行合并
                    let previous = lines.remove(*row - 1);
                    let merged_len = previous.chars().count();
                    lines[*row - 1] = format!("{previous}{}", lines[*row - 1]);
                    *row -= 1;
                    *col = merged_len;
                }
            }
        }
        KeyCode::Enter => {
            if let FileMode::Editor {
                lines, row, col, ..
            } = &mut app.files.mode
            {
                let chars: Vec<char> = lines[*row].chars().collect();
                let index = (*col).min(chars.len());
                let head: String = chars[..index].iter().collect();
                let tail: String = chars[index..].iter().collect();
                lines[*row] = head;
                lines.insert(*row + 1, tail);
                *row += 1;
                *col = 0;
            }
        }
        KeyCode::Tab => {
            if let FileMode::Editor {
                lines, row, col, ..
            } = &mut app.files.mode
            {
                lines[*row].insert(*col, ' ');
                lines[*row].insert(*col, ' ');
                lines[*row].insert(*col, ' ');
                lines[*row].insert(*col, ' ');
                *col += 4;
            }
        }
        KeyCode::Esc => {
            // 放弃编辑，回到内容查看
            if let FileMode::Editor { path, .. } = &app.files.mode {
                let path = path.clone();
                app.files.mode = FileMode::Listing;
                app.status = "已放弃编辑".into();
                open_file(ctx, &path);
            }
        }
        KeyCode::Char(c) if !ctrl => {
            if let FileMode::Editor {
                lines, row, col, ..
            } = &mut app.files.mode
            {
                let mut chars: Vec<char> = lines[*row].chars().collect();
                let index = (*col).min(chars.len());
                chars.insert(index, c);
                *col = index + 1;
                lines[*row] = chars.into_iter().collect();
            }
        }
        _ => {}
    }
    if row_delta != 0
        && let FileMode::Editor {
            lines, row, col, ..
        } = &mut app.files.mode
    {
        *row = ((*row as i32) + row_delta).clamp(0, line_count.max(1) as i32 - 1) as usize;
        let len = lines.get(*row).map_or(0, |line| line.chars().count());
        *col = (*col).min(len);
    }
    if let Some(col) = new_col
        && let FileMode::Editor { col: target, .. } = &mut app.files.mode
    {
        *target = col;
    }
}

/// 文件面板渲染（占满内容区）。
pub(crate) fn render(frame: &mut Frame, app: &App, area: Rect) {
    let (accent, muted, background) = palette(app.ui["theme"].as_str().unwrap_or("dark"));
    let focused = app.view == View::Files;
    let title = match &app.files.mode {
        FileMode::Listing => format!(
            " 文件 · {}{}{} ",
            if app.files.root.is_empty() {
                "workspace"
            } else {
                &app.files.root
            },
            if app.files.cwd.is_empty() { "" } else { "/" },
            app.files.cwd
        ),
        FileMode::Content { path, .. } => format!(" 文件 · {path} "),
        FileMode::Editor { path, .. } => format!(" 编辑 · {path}（Ctrl+S 保存，Esc 放弃） "),
        FileMode::Binary { path } => format!(" 文件 · {path}（二进制） "),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            title,
            Style::default().fg(if focused { accent } else { muted }),
        ))
        .style(Style::default().bg(background));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    match &app.files.mode {
        FileMode::Listing => render_listing(frame, app, inner, accent, muted),
        FileMode::Content {
            path,
            content,
            markdown: as_md,
            scroll,
        } => {
            if *as_md {
                let md = markdown::MarkdownPalette::from_theme(
                    app.ui["theme"].as_str().unwrap_or("dark"),
                );
                let lines = markdown::render(content, &md, inner.width);
                frame.render_widget(
                    Paragraph::new(lines)
                        .block(Block::default().borders(Borders::TOP).title(Span::styled(
                            format!(" Markdown · M 原文 · ↑↓ 滚动 · e 编辑 · {path} "),
                            Style::default().fg(muted),
                        )))
                        .scroll((*scroll, 0))
                        .wrap(Wrap { trim: false }),
                    inner,
                );
            } else {
                let lines: Vec<Line> = content
                    .lines()
                    .map(|line| Line::from(line.to_owned()))
                    .collect();
                frame.render_widget(
                    Paragraph::new(lines)
                        .block(Block::default().borders(Borders::TOP).title(Span::styled(
                            format!(" M Markdown 渲染 · e 编辑 · o 外开 · {path} "),
                            Style::default().fg(muted),
                        )))
                        .scroll((*scroll, 0))
                        .wrap(Wrap { trim: false }),
                    inner,
                );
            }
        }
        FileMode::Editor {
            lines, row, col, ..
        } => {
            render_editor(frame, lines, *row, *col, inner, accent, muted);
        }
        FileMode::Binary { path } => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "二进制文件，无法在终端内查看",
                        Style::default().fg(muted),
                    )),
                    Line::raw(""),
                    Line::from(Span::styled(
                        format!("Enter/o 系统浏览器打开 {path}"),
                        Style::default().fg(accent).add_modifier(Modifier::BOLD),
                    )),
                    Line::raw("Esc 返回列表"),
                ]),
                inner,
            );
        }
    }
}

fn render_listing(frame: &mut Frame, app: &App, area: Rect, accent: Color, muted: Color) {
    let count = app.files.entries.len();
    let cursor = if count == 0 {
        0
    } else {
        app.files.cursor.min(count - 1)
    };
    let mut lines = vec![Line::from(Span::styled(
        "Enter 打开 · Backspace 上级 · n/N 新建文件/目录 · r 改名 · d 删除 · s 设根 · p 预览 · o 外开",
        Style::default().fg(muted),
    ))];
    if app.files.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "（空目录）",
            Style::default().fg(muted),
        )));
    }
    for (index, entry) in app.files.entries.iter().enumerate() {
        let selected = index == cursor;
        let name = entry["name"].as_str().unwrap_or("");
        let kind = entry["kind"].as_str().unwrap_or("file");
        let (icon, color) = match kind {
            "dir" => ("▸", accent),
            "link" => ("~", muted),
            _ => (" ", muted),
        };
        let size = entry["size"].as_u64().unwrap_or(0);
        let size_text = if kind == "dir" {
            String::new()
        } else if size >= 1024 * 1024 {
            format!(" {:>7.1}M", size as f64 / 1024.0 / 1024.0)
        } else if size >= 1024 {
            format!(" {:>7.1}K", size as f64 / 1024.0)
        } else {
            format!(" {size:>8}B")
        };
        let modified = entry["modified"].as_u64().unwrap_or(0);
        let time_text = if modified > 0 {
            widgets::relative_time(modified, widgets::now_ms())
        } else {
            String::new()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if selected { "› " } else { "  " },
                Style::default().fg(accent),
            ),
            Span::styled(icon, Style::default().fg(color)),
            Span::styled(
                widgets::truncate(name, 44),
                Style::default()
                    .fg(if kind == "dir" { accent } else { Color::Reset })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
            Span::styled(size_text, Style::default().fg(muted)),
            Span::styled(format!("  {time_text}"), Style::default().fg(muted)),
        ]));
    }
    if app.files.truncated {
        lines.push(Line::from(Span::styled(
            "… 目录过大已截断",
            Style::default().fg(muted),
        )));
    }
    let scroll = cursor.saturating_sub(usize::from(area.height.saturating_sub(2)));
    frame.render_widget(
        Paragraph::new(lines).scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0)),
        area,
    );
}

fn render_editor(
    frame: &mut Frame,
    lines: &[String],
    row: usize,
    col: usize,
    area: Rect,
    accent: Color,
    muted: Color,
) {
    let mut lines_out: Vec<Line> = Vec::new();
    let window = area.height as usize;
    let start = row
        .saturating_sub(window / 2)
        .min(lines.len().saturating_sub(1));
    for (index, line) in lines.iter().enumerate().skip(start).take(window) {
        let marker = if index == row { "›" } else { " " };
        lines_out.push(Line::from(vec![
            Span::styled(
                format!("{marker} {:>4} ", index + 1),
                Style::default().fg(muted),
            ),
            Span::styled(
                line.clone(),
                Style::default().fg(if index == row { accent } else { Color::Reset }),
            ),
        ]));
    }
    if lines.is_empty() {
        lines_out.push(Line::from(Span::styled(
            "（空文件，直接输入内容）",
            Style::default().fg(muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines_out), area);
    // 光标位置：行内列号（按显示宽度）
    let col_width = lines
        .get(row)
        .map(|line| line.chars().take(col).collect::<String>().width())
        .unwrap_or(0);
    let view_row = row.saturating_sub(start);
    if area.width > 8 && area.height > 1 {
        frame.set_cursor_position((
            area.x
                + 7
                + u16::try_from(col_width)
                    .unwrap_or(u16::MAX)
                    .min(area.width - 8),
            area.y + u16::try_from(view_row).unwrap_or(0).min(area.height - 1),
        ));
    }
}

use unicode_width::UnicodeWidthStr;

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use serde_json::json;

    #[tokio::test]
    async fn listing_navigation_and_render() {
        let mut app = App {
            view: View::Files,
            files: FilesPanel {
                cwd: "src".into(),
                entries: vec![
                    json!({"name":"tui","kind":"dir","size":0,"modified":0}),
                    json!({"name":"main.rs","kind":"file","size":2048,"modified":1_700_000_000_000u64}),
                ],
                ..FilesPanel::default()
            },
            ..App::default()
        };
        // 目录进入拼接 cwd（cwd=src，因此带前缀）
        assert_eq!(selected_path(&app.files).as_deref(), Some("src/tui/"));
        let client = reqwest::Client::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let ctx = Ctx {
            client: &client,
            url: "http://127.0.0.1:1",
            sender: &tx,
        };
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(app.files.cursor, 1);
        assert_eq!(selected_path(&app.files).as_deref(), Some("src/main.rs"));
        // Backspace 返回上级（不 panic；无服务时请求静默失败）
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(app.files.cwd, "");
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal
            .draw(|frame| render(frame, &app, ratatui::layout::Rect::new(0, 0, 100, 24)))
            .unwrap();
    }

    #[tokio::test]
    async fn editor_typing_and_saving() {
        let client = reqwest::Client::new();
        let (tx, _rx) = tokio::sync::mpsc::channel(4);
        let ctx = Ctx {
            client: &client,
            url: "http://127.0.0.1:1",
            sender: &tx,
        };
        let mut app = App {
            view: View::Files,
            files: FilesPanel {
                mode: FileMode::Editor {
                    path: "a.txt".into(),
                    lines: vec!["hello".into(), "".into()],
                    row: 0,
                    col: 5,
                },
                ..FilesPanel::default()
            },
            ..App::default()
        };
        // 输入感叹号 → 拼在行尾
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE),
        );
        let FileMode::Editor { lines, col, .. } = &app.files.mode else {
            panic!("应停留在编辑态");
        };
        assert_eq!(lines[0], "hello!");
        assert_eq!(*col, 6);
        // Enter 拆行
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        let FileMode::Editor {
            lines, row, col, ..
        } = &app.files.mode
        else {
            panic!("应停留在编辑态");
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(*row, 1);
        assert_eq!(*col, 0);
        // Ctrl+S 保存不 panic
        handle_key(
            &mut app,
            &ctx,
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
        );
    }

    #[test]
    fn urlencode_escapes_specials() {
        assert_eq!(urlencode_query("a b/c+d"), "a%20b/c%2Bd");
        assert_eq!(urlencode_query("中文.rs"), "%E4%B8%AD%E6%96%87.rs");
    }
}
