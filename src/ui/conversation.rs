use crate::constants;
use crate::ui::markdown;
use crate::ui::{OutputItem, SectionKind};
use crate::ui::style;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

/// 对话消息的水平对齐方式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Align {
    Left,
    Right,
}

/// 块状输出的视觉样式：左侧强调竖条颜色 + 可选背景色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    // 显示行缓存：缓存已折行、已上色的最终显示行，避免每次渲染重建。
    display_cache: Vec<Line<'static>>,
    cache_term_width: u16,
    cache_version: u64,
    content_version: u64,
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
            display_cache: Vec::new(),
            cache_term_width: 0,
            cache_version: 0,
            content_version: 0,
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
        self.bump_content_version();
        true
    }

    fn bump_md_version(&mut self) {
        self.md_version = self.md_version.wrapping_add(1);
    }

    fn bump_content_version(&mut self) {
        self.content_version = self.content_version.wrapping_add(1);
    }

    /// 返回已缓存的全宽度显示行（已折行、已上色）。
    /// 仅当内容或终端宽度变化时才重建缓存；纯滚动时只返回引用，O(1)。
    pub(crate) fn cached_display_lines(&mut self, term_width: u16) -> &[Line<'static>] {
        if self.cache_term_width != term_width || self.cache_version != self.content_version {
            let raw = self.all_lines_with_align();
            self.display_cache = crate::ui::render::build_display_lines(&raw, term_width);
            self.cache_term_width = term_width;
            self.cache_version = self.content_version;
        }
        &self.display_cache
    }

    pub(crate) fn push_user_message(&mut self, text: &str) {
        self.flush_md();
        self.md_block = None;
        self.items.push(ConvItem::Line(ConvLine::empty()));
        let user_style = style::user_block();
        let blk = BlockStyle {
            gutter: style::gutter_of(user_style),
            bg: user_style.bg,
        };
        self.items.push(ConvItem::Line(ConvLine {
            spans: vec![(crate::t!("conv-user-label"), user_style)],
            align: Align::Right,
            block: Some(blk),
        }));
        let text_style = user_style.fg(Color::Black);
        self.items.push(ConvItem::Line(ConvLine {
            spans: vec![(text.to_string(), text_style)],
            align: Align::Right,
            block: Some(blk),
        }));
        self.bump_content_version();
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
        pad_block_runs(out)
    }

    /// 应用输出事件。返回 true 表示当前回答完成。
    pub(crate) fn apply_output(&mut self, item: OutputItem) -> bool {
        let done = match item {
            OutputItem::Section(kind) => {
                self.flush_md();
                self.items.push(ConvItem::Line(ConvLine::empty()));
                match kind {
                    SectionKind::Reasoning => {
                        let blk = block_reasoning();
                        self.md_block = Some(blk);
                        self.items.push(ConvItem::Line(ConvLine::block(
                            &crate::t!("conv-reasoning-label"),
                            style::thinking_block(),
                            blk,
                        )));
                    }
                    SectionKind::Answer => {
                        let blk = block_answer();
                        self.md_block = Some(blk);
                        self.items.push(ConvItem::Line(ConvLine::block(
                            &crate::t!("conv-answer-label"),
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
                        &crate::t!("conv-empty-output"),
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
                            crate::t!("conv-folded-lines", count = total - shown),
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
                if text.trim().is_empty() {
                    self.items.push(ConvItem::Line(ConvLine::empty()));
                } else {
                    self.items.push(ConvItem::Line(ConvLine::block(
                        text,
                        style::system_block(),
                        block_system(),
                    )));
                }
                false
            }
            OutputItem::Error(info) => {
                self.flush_md();
                self.md_block = None;
                let retry = if info.retryable { crate::t!("conv-retryable") } else { String::new() };
                self.items.push(ConvItem::Line(ConvLine::empty()));
                self.items.push(ConvItem::Line(ConvLine::block(
                    crate::t!(
                        "conv-error-format",
                        code = info.code,
                        kind = format!("{:?}", info.kind),
                        retry = retry,
                        message = info.message
                    ),
                    style::error_block(),
                    block_error(),
                )));
                false
            }
            OutputItem::Done => {
                self.flush_md();
                self.md_block = None;
                self.items.push(ConvItem::Line(ConvLine::empty()));
                true
            }
        };
        self.bump_content_version();
        done
    }
}

/// 为每个连续块（同一 [`BlockStyle`]）的首尾各插入一行"同色空行"，
/// 从而在色块内部形成上下内边距（card 观感）。
fn pad_block_runs(
    lines: Vec<(Line<'static>, Align, Option<BlockStyle>)>,
) -> Vec<(Line<'static>, Align, Option<BlockStyle>)> {
    let mut out: Vec<(Line<'static>, Align, Option<BlockStyle>)> =
        Vec::with_capacity(lines.len() + 8);
    // 当前已打开（已补上顶部内边距）的块。
    let mut open: Option<BlockStyle> = None;
    for (line, align, block) in lines {
        let run = block;
        if open != run {
            if let Some(prev) = open {
                out.push((Line::from(""), Align::Left, Some(prev)));
            }
            if let Some(next) = run {
                out.push((Line::from(""), Align::Left, Some(next)));
            }
            open = run;
        }
        out.push((line, align, block));
    }
    if let Some(prev) = open {
        out.push((Line::from(""), Align::Left, Some(prev)));
    }
    out
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

fn block_system() -> BlockStyle {
    let role = style::system_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}

fn block_error() -> BlockStyle {
    let role = style::error_block();
    BlockStyle {
        gutter: style::gutter_of(role),
        bg: role.bg,
    }
}

#[cfg(test)]
mod tests {
    use crate::error::ErrorKind;
    use crate::ui::{ErrorInfo, OutputItem};
    use crate::ui::style;

    fn has_block_bg(conv: &super::Conversation, want: Option<ratatui::style::Color>) -> bool {
        conv.all_lines_with_align()
            .iter()
            .any(|(_, _, block)| block.map(|b| b.bg) == Some(want))
    }

    #[test]
    fn notice_renders_as_system_block() {
        let mut conv = super::Conversation::new();
        conv.apply_output(OutputItem::Notice("系统提示".to_string()));
        assert!(has_block_bg(&conv, style::system_block().bg));
    }

    #[test]
    fn blank_notice_stays_plain() {
        let mut conv = super::Conversation::new();
        conv.apply_output(OutputItem::Notice("   ".to_string()));
        assert!(
            conv.all_lines_with_align()
                .iter()
                .all(|(_, _, block)| block.is_none())
        );
    }

    #[test]
    fn error_renders_as_error_block() {
        let mut conv = super::Conversation::new();
        conv.apply_output(OutputItem::Error(ErrorInfo {
            code: "test.boom",
            kind: ErrorKind::Io,
            retryable: true,
            message: "boom".to_string(),
        }));
        assert!(has_block_bg(&conv, style::error_block().bg));
    }

    #[test]
    fn left_block_run_gets_top_and_bottom_padding() {
        use ratatui::style::Color;
        use ratatui::text::Line;
        let block = super::BlockStyle {
            gutter: Color::Red,
            bg: Some(Color::Blue),
        };
        let input = vec![
            (Line::from("header"), super::Align::Left, Some(block)),
            (Line::from("body"), super::Align::Left, Some(block)),
        ];
        let out = super::pad_block_runs(input);
        // 顶部内边距 + header + body + 底部内边距
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].2, Some(block), "首行应为同色顶部内边距");
        assert_eq!(out[3].2, Some(block), "末行应为同色底部内边距");
        let middle: Vec<String> = out[1..3]
            .iter()
            .map(|(line, _, _)| line.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(middle, vec!["header".to_string(), "body".to_string()]);
    }

    #[test]
    fn right_aligned_block_also_gets_padding() {
        use ratatui::style::Color;
        use ratatui::text::Line;
        let block = super::BlockStyle {
            gutter: Color::Blue,
            bg: Some(Color::Blue),
        };
        let input = vec![(Line::from("hi"), super::Align::Right, Some(block))];
        let out = super::pad_block_runs(input);
        // 顶部内边距 + 内容 + 底部内边距
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].2, Some(block), "首行应为同色顶部内边距");
        assert_eq!(out[1].1, super::Align::Right, "内容行保持右对齐");
        assert_eq!(out[2].2, Some(block), "末行应为同色底部内边距");
    }
}
