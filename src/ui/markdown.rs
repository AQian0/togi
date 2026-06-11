//! 基于 pulldown-cmark 的 Markdown → ratatui Line 渲染器。
//!
//! 产出的 Line 未经折行——调用方应使用 interaction 模块的 wrap_line() 做 CJK 友好的折行。
//! 代码块通过 syntect 做语法高亮。
use crate::ui::style;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
#[cfg(test)]
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
fn render_code_block(lang: &str, raw_lines: &[String]) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let lang_hint = if lang.is_empty() {
        String::new()
    } else {
        format!(" {lang}")
    };
    out.push(Line::from(Span::styled(
        format!("```{lang_hint}"),
        style::md_code_border(),
    )));
    let lines: Vec<&str> = raw_lines.iter().flat_map(|s| s.split('\n')).collect();
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    let lines = &lines[..end];
    let syntax = style::find_syntax(lang);
    let theme = style::highlight_theme();
    let mut hl = syntax.map(|s| syntect::easy::HighlightLines::new(s, theme));
    for &line_text in lines {
        if let Some(hl) = hl.as_mut() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if let Ok(regions) = hl.highlight_line(line_text, style::syntax_set()) {
                let mut first = true;
                for (style, text) in regions {
                    if first {
                        first = false;
                        spans.push(Span::styled(
                            format!("  {text}"),
                            style::syntect_to_ratatui(style),
                        ));
                    } else {
                        spans.push(Span::styled(
                            text.to_string(),
                            style::syntect_to_ratatui(style),
                        ));
                    }
                }
            } else {
                spans.push(Span::styled(
                    format!("  {line_text}"),
                    style::md_code_block(),
                ));
            }
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(Span::styled(
                format!("  {line_text}"),
                style::md_code_block(),
            )));
        }
    }
    out.push(Line::from(Span::styled("```", style::md_code_border())));
    out.push(Line::from(""));
    out
}
/// 将 Markdown 文本转为 styled ratatui Line 序列。
/// 产出的行未按终端宽度折行，调用方应自行折行。
pub fn render_markdown(text: &str) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return vec![];
    }
    let parser = Parser::new_ext(text, Options::all());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![style::md_base()];
    let mut link_urls: Vec<String> = Vec::new();
    let mut in_code_block = false;
    let mut code_block_lang = String::new();
    let mut code_buf: Vec<String> = Vec::new();
    let mut list_stack: Vec<(bool, usize)> = Vec::new();
    let mut blockquote_depth: usize = 0;
    for event in parser {
        if in_code_block {
            match event {
                Event::End(TagEnd::CodeBlock) => {
                    in_code_block = false;
                    lines.extend(render_code_block(&code_block_lang, &code_buf));
                    code_buf.clear();
                    code_block_lang.clear();
                }
                Event::Text(text) => {
                    code_buf.push(text.to_string());
                }
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Heading { level: _, .. } => {
                    flush_spans(&mut spans, &mut lines);
                    style_stack.push(style::md_heading());
                }
                Tag::BlockQuote(_) => {
                    blockquote_depth += 1;
                    if blockquote_depth == 1 {
                        flush_spans(&mut spans, &mut lines);
                    }
                    style_stack.push(style::md_quote());
                }
                Tag::CodeBlock(kind) => {
                    flush_spans(&mut spans, &mut lines);
                    in_code_block = true;
                    code_block_lang = match kind {
                        pulldown_cmark::CodeBlockKind::Fenced(lang) => lang.to_string(),
                        pulldown_cmark::CodeBlockKind::Indented => String::new(),
                    };
                }
                Tag::List(start) => {
                    flush_spans(&mut spans, &mut lines);
                    let ordered = start.is_some();
                    let counter = start.unwrap_or(1) as usize;
                    list_stack.push((ordered, counter));
                }
                Tag::Item => {
                    flush_spans(&mut spans, &mut lines);
                    if let Some((ordered, counter)) = list_stack.last_mut() {
                        let prefix = if *ordered {
                            let p = format!("{counter}. ");
                            *counter += 1;
                            p
                        } else {
                            "- ".to_string()
                        };
                        let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                        spans.push(Span::styled(
                            format!("{indent}{prefix}"),
                            style::md_list_bullet(),
                        ));
                    }
                }
                Tag::Emphasis => {
                    style_stack.push(style::md_italic());
                }
                Tag::Strong => {
                    style_stack.push(style::md_bold());
                }
                Tag::Link {
                    link_type: _,
                    dest_url,
                    title: _,
                    id: _,
                } => {
                    style_stack.push(style::md_link());
                    link_urls.push(dest_url.to_string());
                }
                Tag::Strikethrough | Tag::Image { .. } | Tag::MetadataBlock { .. } => {}
                Tag::Paragraph
                | Tag::Table(_)
                | Tag::TableHead
                | Tag::TableRow
                | Tag::TableCell
                | Tag::FootnoteDefinition(_)
                | Tag::HtmlBlock
                | Tag::DefinitionList
                | Tag::DefinitionListTitle
                | Tag::DefinitionListDefinition
                | Tag::Superscript
                | Tag::Subscript => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    style_stack.pop();
                    flush_block(&mut spans, &mut lines);
                }
                TagEnd::BlockQuote(_) => {
                    style_stack.pop();
                    blockquote_depth = blockquote_depth.saturating_sub(1);
                    flush_block(&mut spans, &mut lines);
                    if blockquote_depth == 0 {
                        lines.push(Line::from(""));
                    }
                }
                TagEnd::List(_) => {
                    list_stack.pop();
                    flush_block(&mut spans, &mut lines);
                }
                TagEnd::Item => {
                    flush_spans(&mut spans, &mut lines);
                }
                TagEnd::Paragraph => {
                    flush_block(&mut spans, &mut lines);
                }
                TagEnd::Emphasis | TagEnd::Strong => {
                    style_stack.pop();
                }
                TagEnd::Link => {
                    style_stack.pop();
                    if let Some(url) = link_urls.pop()
                        && !url.is_empty()
                    {
                        spans.push(Span::styled(format!(" ({url})"), style::dim()));
                    }
                }
                TagEnd::Strikethrough
                | TagEnd::Image
                | TagEnd::MetadataBlock(_)
                | TagEnd::FootnoteDefinition
                | TagEnd::CodeBlock
                | TagEnd::Table
                | TagEnd::TableHead
                | TagEnd::TableRow
                | TagEnd::TableCell
                | TagEnd::HtmlBlock
                | TagEnd::DefinitionList
                | TagEnd::DefinitionListTitle
                | TagEnd::DefinitionListDefinition
                | TagEnd::Superscript
                | TagEnd::Subscript => {}
            },
            Event::Text(text) => {
                let style = current_style(&style_stack);
                let parts: Vec<&str> = text.split('\n').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        lines.push(Line::from(std::mem::take(&mut spans)));
                    }
                    if !part.is_empty() {
                        spans.push(Span::styled(part.to_string(), style));
                    }
                }
            }
            Event::Code(text) => {
                spans.push(Span::styled(text.to_string(), style::md_inline_code()));
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                spans.push(Span::styled(text.to_string(), current_style(&style_stack)));
            }
            Event::SoftBreak => {
                spans.push(Span::styled(" ", current_style(&style_stack)));
            }
            Event::HardBreak => {
                lines.push(Line::from(std::mem::take(&mut spans)));
            }
            Event::Rule => {
                flush_spans(&mut spans, &mut lines);
                lines.push(Line::from(Span::styled(
                    "─".repeat(crate::constants::HORIZONTAL_RULE_WIDTH),
                    style::dim(),
                )));
                lines.push(Line::from(""));
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                spans.push(Span::styled(marker, style::md_list_bullet()));
            }
            Event::FootnoteReference(name) => {
                spans.push(Span::styled(format!("[{name}]"), style::dim()));
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                spans.push(Span::styled(text.to_string(), current_style(&style_stack)));
            }
        }
    }
    flush_spans(&mut spans, &mut lines);
    while lines.last().is_some_and(|l| l.spans.is_empty()) {
        lines.pop();
    }
    lines
}
fn current_style(stack: &[Style]) -> Style {
    *stack.last().unwrap_or(&style::md_base())
}

fn flush_spans(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    if !spans.is_empty() {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn flush_block(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>) {
    flush_spans(spans, lines);
    lines.push(Line::from(""));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_text_returns_empty() {
        assert!(render_markdown("").is_empty());
        assert!(render_markdown("   ").is_empty());
    }
    #[test]
    fn plain_text_single_line() {
        let lines = render_markdown("hello world");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[0].content, "hello world");
    }
    #[test]
    fn bold_text() {
        let lines = render_markdown("hello **world**");
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content == "world" && s.style.add_modifier.contains(Modifier::BOLD))
        );
    }
    #[test]
    fn inline_code() {
        let lines = render_markdown("use `println!` macro");
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content == "println!" && s.style.bg == style::md_inline_code().bg)
        );
    }
    #[test]
    fn code_block() {
        let md = "```rust\nfn main() {}\n```";
        let lines = render_markdown(md);
        assert!(lines.len() >= 3);
        assert!(lines[0].spans[0].content.contains("```"));
        let code_line = &lines[1];
        let combined: String = code_line.spans.iter().map(|s| &*s.content).collect();
        assert!(combined.contains("fn"), "combined: {combined}");
        assert!(combined.contains("main"), "combined: {combined}");
    }
    #[test]
    fn code_block_no_language() {
        let md = "```\nplain text\n```";
        let lines = render_markdown(md);
        assert!(lines[0].spans[0].content.contains("```"));
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("plain text")))
        );
    }
    #[test]
    fn heading() {
        let lines = render_markdown("# Title");
        assert!(lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.content == "Title" && s.style.add_modifier.contains(Modifier::BOLD))
        }));
    }
    #[test]
    fn unordered_list() {
        let md = "- item1\n- item2";
        let lines = render_markdown(md);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("item1")))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("item2")))
        );
    }
    #[test]
    fn link_renders_text_and_url() {
        let lines = render_markdown("[click](https://example.com)");
        assert!(lines[0].spans.iter().any(|s| s.content == "click"));
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|s| s.content.contains("example.com"))
        );
    }
    #[test]
    fn horizontal_rule() {
        let md = "before\n\n---\n\nafter";
        let lines = render_markdown(md);
        assert!(
            lines
                .iter()
                .any(|l| l.spans.iter().any(|s| s.content.contains("─")))
        );
    }
}
