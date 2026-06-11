use crate::constants;
use crate::ui::markdown;
use crate::ui::output::{OutputItem, SectionKind};
use crate::ui::style;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// 对话消息的水平对齐方式。
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Align {
    Left,
    Right,
}

/// 块状输出的视觉样式：有背景时渲染为全宽色块，否则退化为左侧竖条。
#[derive(Clone, Copy)]
pub(crate) struct BlockStyle {
    pub gutter: Color,
    pub bg: Option<Color>,
}

/// 对话项——样式行或已缓存的 Markdown 渲染结果。
#[derive(Clone)]
enum ConvItem {
    Line(ConvLine),
    Markdown(Vec<Line<'static>>, Align, Option<BlockStyle>),
}

#[derive(Clone)]
struct ConvLine {
    spans: Vec<(String, Style)>,
    align: Align,
    block: Option<BlockStyle>,
}

pub(crate) struct MarkdownRenderResult {
    version: u64,
    lines: Vec<Line<'static>>,
}

impl ConvLine {
    fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            spans: vec![(text.into(), style)],
            align: Align::Left,
            block: None,
        }
    }

    fn block(text: impl Into<String>, style: Style, block: BlockStyle) -> Self {
        Self {
            spans: vec![(text.into(), style)],
            align: Align::Left,
            block: Some(block),
        }
    }

    fn empty() -> Self {
        Self {
            spans: Vec::new(),
            align: Align::Left,
            block: None,
        }
    }

    fn to_ratatui_line(&self) -> Line<'static> {
        Line::from(
            self.spans
                .iter()
                .map(|(t, s)| Span::styled(t.clone(), *s))
                .collect::<Vec<_>>(),
        )
    }
}

pub(crate) struct Conversation {
    items: Vec<ConvItem>,
    md_buf: String,
    md_block: Option<BlockStyle>,
    md_version: u64,
    live_md_rendered_version: u64,
    live_md_rendered: Vec<Line<'static>>,
    live_md_render_inflight: Option<u64>,
}

impl Conversation {
    pub(crate) fn new() -> Self {
        Self {
            items: Vec::new(),
            md_buf: String::new(),
            md_block: None,
            md_version: 0,
            live_md_rendered_version: 0,
            live_md_rendered: Vec::new(),
            live_md_render_inflight: None,
        }
    }

    pub(crate) fn flush_md(&mut self) {
        if !self.md_buf.is_empty() {
            let source = std::mem::take(&mut self.md_buf);
            let rendered = if self.live_md_rendered_version == self.md_version {
                self.live_md_rendered.clone()
            } else {
                markdown::render_markdown(&source)
            };
            self.items
                .push(ConvItem::Markdown(rendered, Align::Left, self.md_block));
            self.bump_md_version();
            self.live_md_rendered.clear();
            self.live_md_rendered_version = self.md_version;
            self.live_md_render_inflight = None;
        }
    }

    pub(crate) fn schedule_live_markdown_render(
        &mut self,
        tx: &tokio::sync::mpsc::UnboundedSender<MarkdownRenderResult>,
    ) {
        if self.md_buf.is_empty()
            || self.live_md_rendered_version == self.md_version
            || self.live_md_render_inflight.is_some()
        {
            return;
        }

        let version = self.md_version;
        let source = self.md_buf.clone();
        let tx = tx.clone();
        self.live_md_render_inflight = Some(version);
        tokio::task::spawn_blocking(move || {
            let lines = markdown::render_markdown(&source);
            let _ = tx.send(MarkdownRenderResult { version, lines });
        });
    }

    pub(crate) fn apply_markdown_render(&mut self, result: MarkdownRenderResult) -> bool {
        if self.live_md_render_inflight == Some(result.version) {
            self.live_md_render_inflight = None;
        }
        if result.version != self.md_version {
            return false;
        }
        self.live_md_rendered = result.lines;
        self.live_md_rendered_version = result.version;
        true
    }

    fn bump_md_version(&mut self) {
        self.md_version = self.md_version.wrapping_add(1);
    }

    pub(crate) fn push_user_message(&mut self, text: &str) {
        self.flush_md();
        self.md_block = None;
        self.items.push(ConvItem::Line(ConvLine::empty()));
        let user_style = style::user_block();
        self.items.push(ConvItem::Line(ConvLine {
            spans: vec![(format!(" {text} "), user_style)],
            align: Align::Right,
            block: Some(BlockStyle {
                gutter: style::gutter_of(user_style),
                bg: user_style.bg,
            }),
        }));
    }

    /// 返回 (ratatui 行, 对齐方式, 所属块样式)。
    pub(crate) fn all_lines_with_align(&self) -> Vec<(Line<'static>, Align, Option<BlockStyle>)> {
        let mut out: Vec<(Line<'static>, Align, Option<BlockStyle>)> = Vec::new();
        for item in &self.items {
            match item {
                ConvItem::Line(line) => {
                    out.push((line.to_ratatui_line(), line.align, line.block));
                }
                ConvItem::Markdown(rendered, align, block) => {
                    for line in rendered {
                        out.push((line.clone(), *align, *block));
                    }
                }
            }
        }
        if !self.md_buf.is_empty() {
            if self.live_md_rendered_version == self.md_version || !self.live_md_rendered.is_empty()
            {
                for line in &self.live_md_rendered {
                    out.push((line.clone(), Align::Left, self.md_block));
                }
            } else {
                for line in self.md_buf.lines() {
                    out.push((
                        Line::from(Span::styled(line.to_string(), style::md_base())),
                        Align::Left,
                        self.md_block,
                    ));
                }
            }
        }
        out
    }

    /// 应用输出事件。返回 true 表示当前回答完成。
    pub(crate) fn apply_output(&mut self, item: OutputItem) -> bool {
        match item {
            OutputItem::Section(kind) => {
                self.flush_md();
                self.items.push(ConvItem::Line(ConvLine::empty()));
                match kind {
                    SectionKind::Reasoning => {
                        let blk = block_reasoning();
                        self.md_block = Some(blk);
                        self.items.push(ConvItem::Line(ConvLine::block(
                            "思考过程",
                            style::thinking_block(),
                            blk,
                        )));
                    }
                    SectionKind::Answer => {
                        let blk = block_answer();
                        self.md_block = Some(blk);
                        self.items.push(ConvItem::Line(ConvLine::block(
                            "回答",
                            style::assistant_block(),
                            blk,
                        )));
                    }
                }
                false
            }
            OutputItem::Chunk(text) => {
                if self.md_block.is_none() {
                    self.items.push(ConvItem::Line(ConvLine::empty()));
                    self.md_block = Some(block_answer());
                }
                self.md_buf.push_str(&text);
                self.bump_md_version();
                false
            }
            OutputItem::ToolCall { name, summary } => {
                self.flush_md();
                self.md_block = None;
                let blk = block_tool_call();
                self.items.push(ConvItem::Line(ConvLine::empty()));
                let label = if summary.is_empty() {
                    name
                } else {
                    format!("{name} · {summary}")
                };
                self.items.push(ConvItem::Line(ConvLine::block(
                    label,
                    style::tool_call_block(),
                    blk,
                )));
                false
            }
            OutputItem::ToolResult(text) => {
                self.flush_md();
                self.md_block = None;
                let blk = block_tool_result();
                let lines: Vec<&str> = text.lines().collect();
                let total = lines.len();
                if total == 0 || (total == 1 && lines[0].trim().is_empty()) {
                    self.items.push(ConvItem::Line(ConvLine::block(
                        "(无输出)",
                        style::tool_result_block(),
                        blk,
                    )));
                } else {
                    let shown = total.min(constants::TOOL_RESULT_MAX_LINES);
                    for line in lines.iter().take(shown) {
                        self.items.push(ConvItem::Line(ConvLine::block(
                            (*line).to_string(),
                            style::tool_result_block(),
                            blk,
                        )));
                    }
                    if total > shown {
                        self.items.push(ConvItem::Line(ConvLine::block(
                            format!("… 其余 {} 行（已折叠）", total - shown),
                            style::tool_result_block(),
                            blk,
                        )));
                    }
                }
                false
            }
            OutputItem::Notice(text) => {
                self.flush_md();
                self.md_block = None;
                self.items
                    .push(ConvItem::Line(ConvLine::styled(text, style::dim())));
                false
            }
            OutputItem::Error(info) => {
                self.flush_md();
                self.md_block = None;
                let retry = if info.retryable { " · 可重试" } else { "" };
                self.items.push(ConvItem::Line(ConvLine::styled(
                    format!(
                        "!! [{} · {:?}{retry}] {}",
                        info.code, info.kind, info.message
                    ),
                    style::error(),
                )));
                false
            }
            OutputItem::Done => {
                self.flush_md();
                self.md_block = None;
                self.items.push(ConvItem::Line(ConvLine::empty()));
                true
            }
        }
    }
}

fn block_reasoning() -> BlockStyle {
    let role = style::thinking_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}

fn block_answer() -> BlockStyle {
    let role = style::assistant_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}

fn block_tool_call() -> BlockStyle {
    let role = style::tool_call_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}

fn block_tool_result() -> BlockStyle {
    let role = style::tool_result_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}
