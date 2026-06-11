//! 对话帧渲染函数。
//!
//! 负责将对话行序列渲染为 ratatui 帧，包括折行、CJK 宽度计算、
//! 块样式竖条与背景填充、滚动偏移处理等。

use crate::constants;
use crate::ui::conversation::{Align, BlockStyle};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

const BLOCK_PAD: &str = "  ";
const GUTTER_MARK: &str = "▎ ";

/// 计算单个字符在终端中占用的列宽。
///
/// ASCII 字符占 1 列，CJK 等宽字符占 2 列，控制字符和零宽字符占 0 列。
pub fn display_width(c: char) -> usize {
    if c == '\n' || c == '\r' {
        return 0;
    }
    if c.is_control() {
        return 0;
    }
    let cp = c as u32;
    if matches!(
        cp,
        0x0300..=0x036F
            | 0x200B..=0x200F
            | 0xFE00..=0xFE0F
            | 0xFEFF
    ) {
        return 0;
    }
    if matches!(
        cp,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    ) {
        return 2;
    }
    1
}

/// 计算字符串前 `col` 个字符在终端中的显示宽度。
pub fn prefix_width(s: &str, col: usize) -> usize {
    s.chars().take(col).map(display_width).sum()
}

/// 按显示宽度截取字符串的可见部分。
///
/// 跳过前 `offset` 显示宽度，然后取最多 `width` 显示宽度的字符。
/// CJK 字符跨边界时用空格填充。
pub fn visible_slice(line: &str, offset: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut col = 0usize;
    let mut out = String::new();
    for ch in line.chars() {
        let w = display_width(ch);
        if col + w <= offset {
            col += w;
            continue;
        }
        if col < offset {
            out.push(' ');
            col += w;
            continue;
        }
        if col - offset + w > width {
            break;
        }
        out.push(ch);
        col += w;
    }
    out
}

/// 按显示宽度折行。
///
/// 跨 CJK 友好，保留每段 span 的样式，并会切开超长 URL/代码行等单个 span。
pub fn wrap_line(line: &Line<'_>, width: usize) -> Vec<Line<'static>> {
    if width == 0 || line.spans.is_empty() {
        return vec![Line::from("")];
    }

    fn push_char(spans: &mut Vec<Span<'static>>, style: Style, ch: char) {
        if let Some(last) = spans.last_mut()
            && last.style == style
        {
            last.content.to_mut().push(ch);
            return;
        }
        spans.push(Span::styled(ch.to_string(), style));
    }

    fn flush(out: &mut Vec<Line<'static>>, spans: &mut Vec<Span<'static>>, width: &mut usize) {
        if spans.is_empty() {
            out.push(Line::from(""));
        } else {
            out.push(Line::from(std::mem::take(spans)));
        }
        *width = 0;
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for span in &line.spans {
        let style = span.style;
        for ch in span.content.chars() {
            if ch == '\n' {
                flush(&mut out, &mut current, &mut current_width);
                continue;
            }
            let ch_width = display_width(ch);
            if current_width > 0 && current_width + ch_width > width {
                flush(&mut out, &mut current, &mut current_width);
            }
            push_char(&mut current, style, ch);
            current_width += ch_width;
        }
    }

    if !current.is_empty() || out.is_empty() {
        out.push(Line::from(current));
    }
    out
}

fn with_block_background(style: Style, bg: Option<Color>) -> Style {
    if let Some(bg) = bg
        && style.bg.is_none()
    {
        return style.bg(bg);
    }
    style
}

fn block_gutter_span(block: BlockStyle) -> Span<'static> {
    let text = if block.bg.is_some() {
        BLOCK_PAD
    } else {
        GUTTER_MARK
    };
    let mut gutter_style = Style::default().fg(block.gutter);
    if let Some(bg) = block.bg {
        gutter_style = gutter_style.bg(bg);
    }
    Span::styled(text, gutter_style)
}

pub(crate) struct FrameRenderState<'a> {
    pub(crate) conv_lines: &'a [(Line<'static>, Align, Option<BlockStyle>)],
    pub(crate) submitting: bool,
    pub(crate) conv_scroll_offset: usize,
    pub(crate) editor_lines: &'a [String],
    pub(crate) editor_row: usize,
    pub(crate) editor_col: usize,
    pub(crate) editor_scroll_row: usize,
    pub(crate) editor_scroll_col: usize,
    pub(crate) visible_rows: usize,
    pub(crate) text_width: usize,
    pub(crate) separator_style: Style,
    pub(crate) dim_style: Style,
    pub(crate) normal_style: Style,
}

pub(crate) fn render_frame(frame: &mut Frame, state: FrameRenderState<'_>) {
    let FrameRenderState {
        conv_lines,
        submitting,
        conv_scroll_offset,
        editor_lines,
        editor_row,
        editor_col,
        editor_scroll_row,
        editor_scroll_col,
        visible_rows,
        text_width,
        separator_style,
        dim_style,
        normal_style,
    } = state;
    let area = frame.area();
    if area.width < constants::MIN_TERMINAL_WIDTH || area.height < constants::MIN_TERMINAL_HEIGHT {
        return;
    }
    frame.render_widget(
        Block::default().style(crate::ui::style::app_background()),
        area,
    );

    let input_rows = editor_lines.len().clamp(1, constants::MAX_TEXT_ROWS) as u16;
    let input_height = (input_rows + 2).min(area.height.saturating_sub(2));
    let conv_height = area.height.saturating_sub(input_height);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(conv_height),
            Constraint::Length(input_height),
        ])
        .split(area);

    let conv_area = layout[0];
    let input_area = layout[1];

    {
        let mut display_lines: Vec<Line<'static>> = Vec::new();
        for (line, align, block) in conv_lines {
            let gutter = if *align == Align::Right { None } else { *block };
            let reserve = if gutter.is_some() {
                constants::GUTTER_W
            } else {
                0
            };
            let base_width = (conv_area.width as usize).saturating_sub(reserve).max(1);
            let wrapped = wrap_line(line, base_width);
            for wline in wrapped {
                let effective_width = if *align == Align::Right {
                    conv_area
                        .width
                        .saturating_sub(constants::USER_MARGIN as u16) as usize
                } else {
                    base_width
                };
                let re_wrapped = wrap_line(&wline, effective_width.max(1));
                for rline in re_wrapped {
                    let dw: usize = rline
                        .spans
                        .iter()
                        .flat_map(|s| s.content.chars())
                        .map(display_width)
                        .sum();
                    let mut spans: Vec<Span> = Vec::new();

                    if let Some(bs) = gutter {
                        spans.push(block_gutter_span(bs));
                    }

                    let block_bg = block.and_then(|bs| bs.bg);
                    spans.extend(rline.spans.iter().map(|s| {
                        Span::styled(s.content.clone(), with_block_background(s.style, block_bg))
                    }));

                    if *align == Align::Right {
                        let pad = effective_width.saturating_sub(dw);
                        if pad > 0 {
                            spans.insert(0, Span::styled(" ".repeat(pad), Style::default()));
                        }
                    }

                    if *align == Align::Left
                        && let Some(bg_color) = block_bg
                    {
                        let full_width = conv_area.width as usize;
                        let current_w: usize = spans
                            .iter()
                            .flat_map(|s| s.content.chars())
                            .map(display_width)
                            .sum();
                        if current_w < full_width {
                            spans.push(Span::styled(
                                " ".repeat(full_width - current_w),
                                Style::default().bg(bg_color),
                            ));
                        }
                    }
                    display_lines.push(Line::from(spans));
                }
            }
        }

        let total = display_lines.len();
        let visible = conv_area.height as usize;
        if total <= visible {
            let mut final_lines = display_lines;
            if submitting {
                final_lines.push(Line::from(""));
                final_lines.push(Line::from(Span::styled(" …", dim_style)));
            }
            frame.render_widget(
                Paragraph::new(final_lines).style(crate::ui::style::app_background()),
                conv_area,
            );
        } else {
            let auto_scroll = total.saturating_sub(visible);
            let scrolled_up = conv_scroll_offset > 0;
            let content_vis = if scrolled_up {
                visible.saturating_sub(1)
            } else {
                visible
            };
            let scroll = auto_scroll.saturating_sub(conv_scroll_offset);
            let scroll = scroll.min(total.saturating_sub(content_vis));
            let mut final_lines: Vec<Line> = display_lines
                .into_iter()
                .skip(scroll)
                .take(content_vis)
                .collect();
            if scrolled_up {
                final_lines.push(Line::from(Span::styled(
                    "── 回到底部 (PageDown) ──",
                    dim_style,
                )));
            }
            if submitting {
                final_lines.push(Line::from(""));
                final_lines.push(Line::from(Span::styled(" …", dim_style)));
            }
            frame.render_widget(
                Paragraph::new(final_lines).style(crate::ui::style::app_background()),
                conv_area,
            );
        }
    }

    frame.render_widget(
        Block::default().style(crate::ui::style::input_background()),
        input_area,
    );

    let sep_rect = Rect::new(input_area.x, input_area.y, input_area.width, 1);
    let sep = format!("{:─<width$}", "", width = sep_rect.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(sep, separator_style))),
        sep_rect,
    );

    let edit_area = Rect::new(
        input_area.x,
        input_area.y + 1,
        input_area.width,
        input_area.height.saturating_sub(2),
    );
    if edit_area.height > 0 && edit_area.width >= 2 && visible_rows > 0 {
        let vis = visible_rows;
        let mut edit_display: Vec<Line> = Vec::with_capacity(vis);
        for v in 0..vis {
            let li = editor_scroll_row + v;
            if li >= editor_lines.len() {
                edit_display.push(Line::from(""));
                continue;
            }
            let rest = visible_slice(&editor_lines[li], editor_scroll_col, text_width);
            edit_display.push(Line::from(Span::styled(rest, normal_style)));
        }
        frame.render_widget(
            Paragraph::new(edit_display).style(crate::ui::style::input_background()),
            Rect::new(edit_area.x, edit_area.y, edit_area.width, vis as u16),
        );

        let vis_row = editor_row.saturating_sub(editor_scroll_row);
        let cy = (edit_area.y as usize + vis_row).min(edit_area.y as usize + vis - 1);
        let cx = edit_area.x as usize
            + prefix_width(&editor_lines[editor_row], editor_col).saturating_sub(editor_scroll_col);
        let cx = cx.min((edit_area.x + edit_area.width).saturating_sub(1) as usize);
        frame.set_cursor_position(Position::new(cx as u16, cy as u16));
    }

    let bottom_sep_y = input_area.y + input_area.height.saturating_sub(1);
    if bottom_sep_y > input_area.y {
        let bottom_rect = Rect::new(input_area.x, bottom_sep_y, input_area.width, 1);
        let sep = format!("{:─<width$}", "", width = bottom_rect.width as usize);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(sep, separator_style))),
            bottom_rect,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_width_covers_common_cases() {
        assert_eq!(display_width('a'), 1);
        assert_eq!(display_width('你'), 2);
        assert_eq!(display_width('\n'), 0);
    }

    #[test]
    fn visible_slice_honours_offset_and_width() {
        assert_eq!(visible_slice("abcdef", 2, 3), "cde");
        assert_eq!(visible_slice("你好", 1, 3), " 好");
    }

    #[test]
    fn wrap_line_splits_single_long_span() {
        let wrapped = wrap_line(&Line::from("abcdefghij"), 4);
        let text: Vec<String> = wrapped
            .iter()
            .map(|line| line.spans.iter().map(|s| &*s.content).collect())
            .collect();
        assert_eq!(text, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn wrap_line_respects_cjk_width() {
        let wrapped = wrap_line(&Line::from("你好abc"), 4);
        let text: Vec<String> = wrapped
            .iter()
            .map(|line| line.spans.iter().map(|s| &*s.content).collect())
            .collect();
        assert_eq!(text, vec!["你好", "abc"]);
    }

    #[test]
    fn block_background_fills_only_unset_background() {
        let bg = Color::Rgb(1, 2, 3);
        let custom = Color::Rgb(4, 5, 6);
        assert_eq!(
            with_block_background(Style::default(), Some(bg)).bg,
            Some(bg)
        );
        assert_eq!(
            with_block_background(Style::default().bg(custom), Some(bg)).bg,
            Some(custom)
        );
    }
}
