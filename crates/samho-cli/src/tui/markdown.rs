//! TUI 的完整 Markdown 渲染器：pulldown-cmark 解析 → ratatui `Vec<Line>`。
//!
//! 覆盖网页端 markdown.ts 的 CommonMark + GFM 子集：标题、段落、有序/无序/
//! 任务列表（嵌套）、代码围栏（syntect 语法高亮，未知语言降级纯文本）、表格
//! （box-drawing 边框、按宽度截断、对齐）、引用块、分隔线、行内代码、粗体/
//! 斜体/删除线、链接（下划线）、图片（🖼 + alt 标注）、数学公式降级原文。
//!
//! 流式输出时按 revision 全量重渲（累计文本量级安全），调用方负责记忆化。

use std::sync::OnceLock;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// 渲染用配色（由调用方按主题派生，见 mod.rs 的 `palette`）。
#[derive(Clone, Copy)]
pub struct MarkdownPalette {
    pub accent: Color,
    pub muted: Color,
    pub code_bg: Color,
}

impl MarkdownPalette {
    pub fn from_theme(theme: &str) -> Self {
        match theme {
            "light" => Self {
                accent: Color::Rgb(49, 91, 38),
                muted: Color::Rgb(69, 80, 91),
                code_bg: Color::Rgb(238, 240, 234),
            },
            "contrast" => Self {
                accent: Color::Yellow,
                muted: Color::White,
                code_bg: Color::Rgb(40, 40, 40),
            },
            _ => Self {
                accent: Color::Rgb(188, 229, 87),
                muted: Color::Rgb(128, 142, 151),
                code_bg: Color::Rgb(24, 32, 38),
            },
        }
    }
}

/// 把 Markdown 渲染为定宽受限的行列表（表格按 width 截断，其余交给
/// Paragraph 的自动折行）。
pub fn render(markdown: &str, palette: &MarkdownPalette, width: u16) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let mut r = Renderer {
        palette: *palette,
        width: usize::from(width.max(10)),
        lines: Vec::new(),
        spans: Vec::new(),
        stack: Vec::new(),
        bold: 0,
        italic: 0,
        strike: 0,
        link: false,
        image_depth: 0,
    };
    for event in Parser::new_ext(markdown, options) {
        r.event(event);
    }
    r.flush_inline();
    r.lines
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Paragraph,
    TableCell,
    Emphasis,
    Strong,
    Strike,
    Link,
    Image,
}

enum Block {
    Code {
        lang: Option<String>,
        buf: String,
    },
    List {
        ordered: Option<u64>,
        items: u64,
    },
    Item {
        prefix: Option<Span<'static>>,
        first_done: bool,
    },
    Quote {
        depth: usize,
    },
    Table {
        aligns: Vec<Alignment>,
        header: Option<Vec<Vec<Span<'static>>>>,
        rows: Vec<Vec<Vec<Span<'static>>>>,
        current: Vec<Vec<Span<'static>>>,
        in_head: bool,
    },
    Other(Kind),
}

struct Renderer {
    palette: MarkdownPalette,
    width: usize,
    lines: Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    stack: Vec<Block>,
    bold: u32,
    italic: u32,
    strike: u32,
    link: bool,
    image_depth: u32,
}

impl Renderer {
    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag_end) => self.end(tag_end),
            Event::Text(text) => {
                if let Some(Block::Code { buf, .. }) = self.stack.last_mut() {
                    buf.push_str(&text); // 代码块正文不入行内缓冲
                } else {
                    self.push_text(&text);
                }
            }
            Event::Code(code) => {
                self.spans.push(Span::styled(
                    code.to_string(),
                    Style::default()
                        .fg(self.palette.accent)
                        .bg(self.palette.code_bg),
                ));
            }
            Event::InlineMath(math) => self.spans.push(Span::styled(
                format!("${math}$"),
                Style::default()
                    .fg(self.palette.muted)
                    .add_modifier(Modifier::ITALIC),
            )),
            Event::DisplayMath(math) => {
                self.flush_inline();
                self.ensure_blank();
                let style = Style::default().fg(self.palette.muted);
                self.push_prefixed_lines(
                    math.lines()
                        .map(|line| Line::styled(line.to_owned(), style))
                        .collect(),
                    false,
                );
                self.lines.push(Line::raw(""));
            }
            Event::SoftBreak => self.spans.push(Span::raw(" ")),
            Event::HardBreak => {
                if self.in_cell() {
                    self.spans.push(Span::raw(" "));
                } else {
                    self.flush_inline();
                }
            }
            Event::Rule => {
                self.flush_inline();
                self.ensure_blank();
                let rule = "─".repeat(24.min(self.width.saturating_sub(4)).max(4));
                self.push_prefixed_lines(
                    vec![Line::styled(rule, Style::default().fg(self.palette.muted))],
                    true,
                );
            }
            Event::TaskListMarker(checked) => self.set_item_prefix(Span::styled(
                if checked { "☑ " } else { "☐ " },
                Style::default().fg(if checked {
                    self.palette.accent
                } else {
                    self.palette.muted
                }),
            )),
            Event::FootnoteReference(name) => self.spans.push(Span::styled(
                format!("[^{name}]"),
                Style::default().fg(self.palette.muted),
            )),
            Event::Html(html) | Event::InlineHtml(html) => {
                for (index, segment) in html.split('\n').enumerate() {
                    if index > 0 {
                        self.flush_inline();
                    }
                    if !segment.is_empty() {
                        self.spans.push(Span::styled(
                            segment.to_string(),
                            Style::default().fg(self.palette.muted),
                        ));
                    }
                }
            }
        }
    }

    fn start(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                self.flush_inline();
                self.ensure_blank();
                self.open(Block::Other(Kind::Paragraph));
            }
            Tag::Heading { .. } => {
                self.flush_inline();
                self.ensure_blank();
                self.open(Block::Other(Kind::Paragraph));
            }
            Tag::BlockQuote(_) => {
                self.flush_inline();
                self.ensure_blank();
                let depth = self.quote_depth() + 1;
                self.open(Block::Quote { depth });
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                self.ensure_blank();
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => info
                        .split([' ', '\t', ','])
                        .next()
                        .filter(|lang| !lang.is_empty())
                        .map(str::to_owned),
                    CodeBlockKind::Indented => None,
                };
                self.open(Block::Code {
                    lang,
                    buf: String::new(),
                });
            }
            Tag::List(start) => {
                self.flush_inline();
                self.ensure_blank();
                self.open(Block::List {
                    ordered: start,
                    items: 0,
                });
            }
            Tag::Item => {
                // 序号/项目符在 Item 开始时即刻确定；任务列表随后用 ☑/☐ 覆盖
                let list = self.stack.iter().rev().find_map(|block| match block {
                    Block::List { ordered, items, .. } => Some((*ordered, *items)),
                    _ => None,
                });
                if let Some(Block::List { items, .. }) = self.stack.last_mut() {
                    *items += 1;
                }
                let prefix = match list {
                    Some((Some(start), index)) => format!("{}. ", start + index),
                    _ => "• ".to_owned(),
                };
                self.open(Block::Item {
                    prefix: Some(Span::styled(
                        prefix,
                        Style::default().fg(self.palette.accent),
                    )),
                    first_done: false,
                });
            }
            Tag::Emphasis => {
                self.italic += 1;
                self.open(Block::Other(Kind::Emphasis));
            }
            Tag::Strong => {
                self.bold += 1;
                self.open(Block::Other(Kind::Strong));
            }
            Tag::Strikethrough => {
                self.strike += 1;
                self.open(Block::Other(Kind::Strike));
            }
            Tag::Link { .. } => {
                self.link = true;
                self.open(Block::Other(Kind::Link));
            }
            Tag::Image { .. } => {
                self.flush_inline();
                self.image_depth += 1;
                self.open(Block::Other(Kind::Image));
                self.spans
                    .push(Span::styled("🖼 ", Style::default().fg(self.palette.accent)));
            }
            Tag::Table(aligns) => {
                self.flush_inline();
                self.ensure_blank();
                self.open(Block::Table {
                    aligns,
                    header: None,
                    rows: Vec::new(),
                    current: Vec::new(),
                    in_head: false,
                });
            }
            Tag::TableHead => {
                if let Some(Block::Table {
                    in_head, current, ..
                }) = self.stack.last_mut()
                {
                    *in_head = true;
                    current.clear();
                }
            }
            Tag::TableRow => {
                self.flush_inline();
                if let Some(Block::Table { current, .. }) = self.stack.last_mut() {
                    current.clear();
                }
            }
            Tag::TableCell => {
                self.flush_inline();
                self.open(Block::Other(Kind::TableCell));
            }
            // HTML 块/脚注/定义列表等按透明容器处理，End 时直接出栈
            _ => self.open(Block::Other(Kind::Paragraph)),
        }
    }

    fn end(&mut self, tag_end: TagEnd) {
        match tag_end {
            TagEnd::Paragraph => {
                self.flush_inline();
                let in_item = matches!(self.stack.last(), Some(Block::Item { .. }));
                self.close();
                if !in_item {
                    self.lines.push(Line::raw(""));
                }
            }
            TagEnd::Heading(level) => {
                self.flush_inline();
                let spans = std::mem::take(&mut self.spans);
                let heading_style = heading_style(heading_number(level), &self.palette);
                let styled: Vec<Span<'static>> = spans
                    .into_iter()
                    .map(|span| {
                        if span.style.bg.is_some() {
                            span // 行内代码保留底色
                        } else {
                            Span::styled(span.content, heading_style)
                        }
                    })
                    .collect();
                self.push_prefixed_lines(vec![Line::from(styled)], false);
                self.close();
                self.lines.push(Line::raw(""));
            }
            TagEnd::BlockQuote(_) => self.close(),
            TagEnd::CodeBlock => {
                let lang = match self.stack.last() {
                    Some(Block::Code { lang, .. }) => lang.clone(),
                    _ => None,
                };
                let buf = match self.stack.last_mut() {
                    Some(Block::Code { buf, .. }) => std::mem::take(buf),
                    _ => String::new(),
                };
                self.close();
                let lines = highlight_code(&lang, &buf, &self.palette);
                self.push_prefixed_lines(lines, true);
                self.lines.push(Line::raw(""));
            }
            TagEnd::List(_) => {
                self.close();
                if self.list_depth() == 0 {
                    self.lines.push(Line::raw(""));
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                self.close();
            }
            TagEnd::Emphasis => {
                self.italic = self.italic.saturating_sub(1);
                self.close();
            }
            TagEnd::Strong => {
                self.bold = self.bold.saturating_sub(1);
                self.close();
            }
            TagEnd::Strikethrough => {
                self.strike = self.strike.saturating_sub(1);
                self.close();
            }
            TagEnd::Link => {
                self.link = false;
                self.close();
            }
            TagEnd::Image => {
                self.image_depth = self.image_depth.saturating_sub(1);
                self.spans.push(Span::styled(
                    "（网页端可渲染）",
                    Style::default().fg(self.palette.muted),
                ));
                self.close();
            }
            TagEnd::Table => {
                let (header, rows, aligns) = match self.stack.last_mut() {
                    Some(Block::Table {
                        header,
                        rows,
                        aligns,
                        ..
                    }) => (header.take(), std::mem::take(rows), aligns.clone()),
                    _ => (None, Vec::new(), Vec::new()),
                };
                self.close();
                let table_lines =
                    render_table(header.as_deref(), &rows, &aligns, self.width, &self.palette);
                self.push_prefixed_lines(table_lines, false);
                self.lines.push(Line::raw(""));
            }
            TagEnd::TableHead => {
                // pulldown 的表头行不经过 TableRow：TableHead 直接包含 TableCell
                if let Some(Block::Table {
                    in_head,
                    header,
                    current,
                    ..
                }) = self.stack.last_mut()
                {
                    *in_head = false;
                    let row = std::mem::take(current);
                    if !row.is_empty() {
                        *header = Some(row);
                    }
                }
            }
            TagEnd::TableRow => {
                // 表头行与数据行都在行结束时入库（行结束先于 TableHead 结束）
                if let Some(Block::Table {
                    rows,
                    header,
                    in_head,
                    current,
                    ..
                }) = self.stack.last_mut()
                {
                    let row = std::mem::take(current);
                    if !row.is_empty() {
                        if *in_head {
                            *header = Some(row);
                        } else {
                            rows.push(row);
                        }
                    }
                }
            }
            TagEnd::TableCell => {
                let spans = std::mem::take(&mut self.spans);
                self.close(); // 先出 TableCell 容器，栈顶回到 Table 再入库
                if let Some(Block::Table { current, .. }) = self.stack.last_mut() {
                    current.push(spans);
                }
            }
            TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => self.close(),
        }
    }

    // —— 行内状态 ——

    fn in_cell(&self) -> bool {
        matches!(self.stack.last(), Some(Block::Other(Kind::TableCell)))
    }

    fn push_text(&mut self, text: &str) {
        for (index, segment) in text.split('\n').enumerate() {
            if index > 0 {
                self.flush_inline();
            }
            if segment.is_empty() {
                continue;
            }
            let mut style = Style::default();
            if self.bold > 0 {
                style = style.add_modifier(Modifier::BOLD);
            }
            if self.italic > 0 {
                style = style.add_modifier(Modifier::ITALIC);
            }
            if self.strike > 0 {
                style = style.add_modifier(Modifier::CROSSED_OUT);
            }
            if self.link && self.image_depth == 0 {
                style = style
                    .add_modifier(Modifier::UNDERLINED)
                    .fg(self.palette.accent);
            }
            if self.image_depth > 0 {
                style = Style::default().fg(self.palette.muted);
            }
            self.spans.push(Span::styled(segment.to_owned(), style));
        }
    }

    fn set_item_prefix(&mut self, prefix: Span<'static>) {
        if let Some(Block::Item { prefix: slot, .. }) = self.stack.last_mut() {
            *slot = Some(prefix);
        }
    }

    fn flush_inline(&mut self) {
        let spans = std::mem::take(&mut self.spans);
        if spans.is_empty() {
            return;
        }
        let (prefix_spans, _) = self.prefix_chain(true);
        let mut line_spans = prefix_spans;
        line_spans.extend(spans);
        self.lines.push(Line::from(line_spans));
        if let Some(Block::Item { first_done, .. }) = self.stack.last_mut() {
            *first_done = true;
        }
    }

    // —— 前缀与缩进 ——

    fn quote_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|block| matches!(block, Block::Quote { .. }))
            .count()
    }

    fn list_depth(&self) -> usize {
        self.stack
            .iter()
            .filter(|block| matches!(block, Block::List { .. }))
            .count()
    }

    /// 生成 (首行前缀 spans, 后续行前缀 spans)：引用竖线 + 列表缩进 + 项目符。
    fn prefix_chain(&self, first: bool) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let mut quote_bar = String::new();
        let mut indent = String::new();
        let mut item_prefix: Option<Span<'static>> = None;
        for block in &self.stack {
            match block {
                Block::Quote { depth } => {
                    for _ in 0..*depth {
                        quote_bar.push_str("▌ ");
                    }
                }
                Block::Item {
                    prefix, first_done, ..
                } => {
                    if first && !*first_done && item_prefix.is_none() {
                        item_prefix = prefix.clone();
                    } else {
                        indent.push_str("  ");
                    }
                }
                _ => {}
            }
        }
        let mut first_line = Vec::new();
        let mut cont_line = Vec::new();
        if !quote_bar.is_empty() {
            let bar = Span::styled(quote_bar.clone(), Style::default().fg(self.palette.accent));
            first_line.push(bar.clone());
            cont_line.push(bar);
        }
        if !indent.is_empty() {
            let indent = Span::raw(indent.clone());
            first_line.push(indent.clone());
            cont_line.push(indent);
        }
        if let Some(prefix) = item_prefix {
            first_line.push(prefix);
        }
        (first_line, cont_line)
    }

    fn open(&mut self, block: Block) {
        self.stack.push(block);
    }

    fn close(&mut self) {
        self.stack.pop();
    }

    fn ensure_blank(&mut self) {
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.lines.push(Line::raw(""));
        }
    }

    /// 按 prefix 链批量推入块级行（表格、代码、引用等）；`indent_all` 时
    /// 所有行都带首行前缀（代码块整块缩进）。
    fn push_prefixed_lines(&mut self, lines: Vec<Line<'static>>, indent_all: bool) {
        for (index, line) in lines.into_iter().enumerate() {
            let (first_prefix, cont_prefix) = self.prefix_chain(index == 0 || indent_all);
            let prefix = if index == 0 || indent_all {
                first_prefix
            } else {
                cont_prefix
            };
            let mut spans = prefix;
            spans.extend(line.spans);
            self.lines.push(Line::from(spans));
        }
        if let Some(Block::Item { first_done, .. }) = self.stack.last_mut() {
            *first_done = true;
        }
    }
}

fn heading_number(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        _ => 4,
    }
}

fn heading_style(level: u32, palette: &MarkdownPalette) -> Style {
    match level {
        1 => Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default()
            .fg(palette.accent)
            .add_modifier(Modifier::BOLD),
        3 => Style::default().add_modifier(Modifier::BOLD),
        _ => Style::default()
            .fg(palette.muted)
            .add_modifier(Modifier::BOLD),
    }
}

// —— 表格渲染 ——

/// 按列宽截断单元格 spans（超出预算时截断并追加 …）。
fn truncate_spans(spans: &[Span<'static>], budget: usize) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= budget {
            break;
        }
        let remaining = budget - used;
        let width = span.width();
        if width <= remaining {
            used += width;
            out.push(span.clone());
            continue;
        }
        // 按 显示宽度 截断当前 span
        let mut piece = String::new();
        let mut piece_width = 0usize;
        for ch in span.content.chars() {
            let ch_width = UnicodeWidthStr::width(ch.to_string().as_str());
            if piece_width + ch_width > remaining.saturating_sub(1) {
                break;
            }
            piece.push(ch);
            piece_width += ch_width;
        }
        piece.push('…');
        out.push(Span::styled(piece, span.style));
        used = budget;
    }
    if out.is_empty() {
        out.push(Span::raw(""));
    }
    out
}

fn cell_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(Span::width).sum()
}

fn pad_cell(spans: &[Span<'static>], column: usize, align: Alignment) -> Vec<Span<'static>> {
    let content = cell_width(spans);
    if content >= column {
        return truncate_spans(spans, column);
    }
    let pad = column - content;
    let (left, right) = match align {
        Alignment::Right => (pad, 0),
        Alignment::Center => (pad / 2, pad - pad / 2),
        _ => (0, pad),
    };
    let mut out = Vec::new();
    if left > 0 {
        out.push(Span::raw(" ".repeat(left)));
    }
    out.extend(spans.iter().cloned());
    if right > 0 {
        out.push(Span::raw(" ".repeat(right)));
    }
    out
}

fn render_table(
    header: Option<&[Vec<Span<'static>>]>,
    rows: &[Vec<Vec<Span<'static>>>],
    aligns: &[Alignment],
    width: usize,
    palette: &MarkdownPalette,
) -> Vec<Line<'static>> {
    let align_of = |index: usize| aligns.get(index).copied().unwrap_or(Alignment::None);
    // 列数与内容宽度
    let mut columns: Vec<usize> = Vec::new();
    let mut all_rows: Vec<(bool, &[Vec<Span<'static>>])> = Vec::new();
    if let Some(header) = header {
        all_rows.push((true, header));
    }
    for row in rows {
        all_rows.push((false, row));
    }
    for (is_head, row) in &all_rows {
        for (index, cell) in row.iter().enumerate() {
            if columns.len() <= index {
                columns.push(0);
            }
            let width = cell_width(cell);
            if width > columns[index] {
                columns[index] = width;
            }
            let _ = is_head;
        }
    }
    // 总宽 = Σ列宽 + 每列两侧空格 + 列间竖线 + 首尾竖线
    let overhead = 2 * columns.len() + columns.len().saturating_sub(1) + 2;
    let budget = width.saturating_sub(overhead);
    loop {
        if columns.iter().sum::<usize>() <= budget {
            break;
        }
        let Some((index, widest)) = columns.iter().enumerate().max_by_key(|(_, w)| **w) else {
            break;
        };
        if *widest <= 3 {
            break;
        }
        columns[index] -= 1;
    }
    let vertical = Span::styled("│", Style::default().fg(palette.muted));
    let mut lines = Vec::new();
    for (is_head, row) in &all_rows {
        let mut line = vec![vertical.clone()];
        for (index, column) in columns.iter().enumerate() {
            if index > 0 {
                line.push(vertical.clone());
            }
            let empty: Vec<Span<'static>> = Vec::new();
            let cell = row.get(index).unwrap_or(&empty);
            let padded = pad_cell(cell, *column, align_of(index));
            line.push(Span::raw(" "));
            if *is_head {
                for span in padded {
                    line.push(Span::styled(
                        span.content,
                        Style::default()
                            .fg(palette.accent)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            } else {
                line.extend(padded);
            }
            line.push(Span::raw(" "));
        }
        line.push(vertical.clone());
        lines.push(Line::from(line));
        if *is_head {
            // 表头下的分隔线
            let mut sep = vec![vertical.clone()];
            for (index, column) in columns.iter().enumerate() {
                if index > 0 {
                    sep.push(Span::styled("┼", Style::default().fg(palette.muted)));
                }
                sep.push(Span::styled(
                    "─".repeat(column + 2),
                    Style::default().fg(palette.muted),
                ));
            }
            sep.push(vertical.clone());
            lines.push(Line::from(sep));
        }
    }
    lines
}

// —— 代码高亮 ——

/// 代码围栏高亮：syntect 语法集按语言匹配，未知语言降级纯文本。
/// 主题随 TUI 主题切换（dark/contrast 用深色主题，light 用浅色）。
fn highlight_code(
    lang: &Option<String>,
    code: &str,
    palette: &MarkdownPalette,
) -> Vec<Line<'static>> {
    static SYNTAXES: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<syntect::highlighting::ThemeSet> = OnceLock::new();
    let syntaxes = SYNTAXES.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines);
    let themes = THEMES.get_or_init(syntect::highlighting::ThemeSet::load_defaults);
    let dark = matches!(palette.code_bg, Color::Rgb(r, _, _) if r < 100);
    let theme_name = if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    };
    let Some(theme) = themes.themes.get(theme_name) else {
        return plain_code(code, palette);
    };
    let syntax = lang
        .as_deref()
        .and_then(|token| syntaxes.find_syntax_by_token(token))
        .unwrap_or_else(|| syntaxes.find_syntax_plain_text());
    let mut highlighter = syntect::easy::HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();
    for source_line in code.lines() {
        let line_with_end = format!("{source_line}\n");
        let Ok(ranges) = highlighter.highlight_line(&line_with_end, syntaxes) else {
            return plain_code(code, palette);
        };
        let mut spans = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches('\n');
            if text.is_empty() {
                continue;
            }
            let mut rat_style = Style::default()
                .fg(Color::Rgb(
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                ))
                .bg(palette.code_bg);
            if style
                .font_style
                .contains(syntect::highlighting::FontStyle::BOLD)
            {
                rat_style = rat_style.add_modifier(Modifier::BOLD);
            }
            if style
                .font_style
                .contains(syntect::highlighting::FontStyle::ITALIC)
            {
                rat_style = rat_style.add_modifier(Modifier::ITALIC);
            }
            if style
                .font_style
                .contains(syntect::highlighting::FontStyle::UNDERLINE)
            {
                rat_style = rat_style.add_modifier(Modifier::UNDERLINED);
            }
            spans.push(Span::styled(text.to_owned(), rat_style));
        }
        if spans.is_empty() {
            spans.push(Span::raw(""));
        }
        lines.push(Line::from(spans));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::raw("")));
    }
    lines
}

fn plain_code(code: &str, palette: &MarkdownPalette) -> Vec<Line<'static>> {
    let style = Style::default().fg(palette.accent).bg(palette.code_bg);
    code.lines()
        .map(|line| Line::from(Span::styled(line.to_owned(), style)))
        .collect::<Vec<_>>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    const PALETTE: MarkdownPalette = MarkdownPalette {
        accent: Color::Rgb(10, 20, 30),
        muted: Color::Rgb(40, 50, 60),
        code_bg: Color::Rgb(7, 8, 9),
    };

    fn line_texts(markdown: &str, width: u16) -> Vec<String> {
        render(markdown, &PALETTE, width)
            .into_iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn joined(markdown: &str, width: u16) -> String {
        line_texts(markdown, width).join("\n")
    }

    #[test]
    fn headings_lists_and_inline_styles_render() {
        let text = joined(
            "# 大标题\n\n正文 **粗** *斜* ~~删~~ `代码` [链接](http://x)\n\n1. 第一\n2. 第二\n   - 嵌套项\n",
            60,
        );
        assert!(text.contains("大标题"));
        assert!(text.contains("1. 第一"));
        assert!(text.contains("2. 第二"));
        assert!(text.contains("• 嵌套项"));
        assert!(text.contains("粗"));
        let lines = render("# 标题\n\n**粗体**文字", &PALETTE, 60);
        let bold = lines.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.content.contains("粗体") && span.style.has_modifier(Modifier::BOLD)
            })
        });
        assert!(bold, "粗体应带 BOLD modifier");
    }

    #[test]
    fn task_lists_use_checked_boxes() {
        let text = joined("- [x] 已完成\n- [ ] 待办\n", 40);
        assert!(text.contains("☑"));
        assert!(text.contains("☐"));
        assert!(text.contains("已完成"));
        assert!(text.contains("待办"));
    }

    #[test]
    fn table_renders_borders_and_truncates_cjk_at_narrow_width() {
        let md = "| 名称 | 说明 |\n|---|---|\n| 短名 | 一个很长的说明文字会被截断 |\n";
        let wide = line_texts(md, 60);
        // 列按最宽内容对齐，允许右侧填充
        assert!(wide.iter().any(|line| line.starts_with("│ 名称 │ 说明")));
        assert!(wide.iter().any(|line| line.contains("│ 短名 │")));
        assert!(wide.iter().any(|line| line.contains("┼")));
        // 窄宽度下应出现省略号且行宽不超限
        let narrow = line_texts(md, 18);
        let joined_narrow = narrow.join("\n");
        assert!(joined_narrow.contains("…"), "窄表应截断: {joined_narrow}");
        for line in &narrow {
            assert!(line.width() <= 20, "表格行超宽: {line}");
        }
    }

    #[test]
    fn fenced_code_is_highlighted_and_unknown_language_falls_back() {
        let rust = render("```rust\nfn main() { let x = 1; }\n```", &PALETTE, 60);
        let has_keyword = rust.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("fn") && span.style.bg == Some(PALETTE.code_bg))
        });
        assert!(has_keyword, "代码块应有代码底色");
        let fallback = render("```nosuchlang\nplain = 1\n```", &PALETTE, 60);
        assert!(fallback.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains("plain = 1"))
        }));
    }

    #[test]
    fn blockquote_rule_and_math_degrade_gracefully() {
        let text = joined("> 引用一行\n\n---\n\n行内 $x^2+1$ 公式\n", 40);
        assert!(text.contains("▌ 引用一行"));
        assert!(text.contains("─"));
        assert!(text.contains("$x^2+1$"));
    }

    #[test]
    fn images_render_as_annotation_and_streaming_repeated_render_is_stable() {
        let md = "![截图](http://host/a.png)";
        let text = joined(md, 60);
        assert!(text.contains("🖼"));
        assert!(text.contains("截图"));
        assert!(text.contains("（网页端可渲染）"));
        let first = render(md, &PALETTE, 60);
        let second = render(md, &PALETTE, 60);
        let first_text: Vec<_> = first
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let second_text: Vec<_> = second
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(first_text, second_text);
    }

    #[test]
    fn cjk_emoji_and_tiny_width_do_not_panic_or_overflow() {
        for width in [12u16, 20, 40, 120] {
            let lines = render(
                "# 中文标题 🦀\n\n| 宽列 | 列二 |\n|---|---|\n| 表格里 的 emoji 🎉 | b |\n\n- 列表 e\u{301}中文\n",
                &PALETTE,
                width,
            );
            for line in &lines {
                let total: usize = line.spans.iter().map(Span::width).sum();
                assert!(total <= usize::from(width) + 2, "行超宽 {total}: {line:?}");
            }
        }
    }
}
