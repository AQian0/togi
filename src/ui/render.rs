//! 对话帧渲染函数。
//!
//! 负责将对话行序列渲染为 ratatui 帧，包括折行、CJK 宽度计算、
//! 块样式竖条与背景填充、滚动偏移处理等。

use crate::shared::constants;
use crate::ui::conversation::{Align, BlockStyle};
use crate::ui::editor::Editor;
use crate::ui::menu::Menu;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use tui_widgets::scrollview::{ScrollView, ScrollViewState};

const GUTTER_MARK: &str = "▎ ";

/// 计算一组 Span 的总显示宽度。
fn spans_display_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .flat_map(|s| s.content.chars())
        .map(display_width)
        .sum()
}

/// 计算单个字符在终端中占用的列宽。
///
/// 控制字符和零宽字符占 0 列，CJK 等宽字符占 2 列，其余占 1 列。
pub fn display_width(c: char) -> usize {
    unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)
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
    // 始终绘制左侧强调竖条（即便块有背景色），使每个角色块都有一条贯穿全高的色条。
    let mut gutter_style = Style::default().fg(block.gutter);
    if let Some(bg) = block.bg {
        gutter_style = gutter_style.bg(bg);
    }
    Span::styled(GUTTER_MARK, gutter_style)
}

pub(crate) struct FrameRenderState<'a> {
    pub(crate) scroll_view: &'a ScrollView,
    pub(crate) conv_state: &'a mut ScrollViewState,
    pub(crate) editor: &'a Editor,
    pub(crate) menu: &'a mut Menu,
    pub(crate) separator_style: Style,
    pub(crate) dim_style: Style,
}

/// 将原始对话行（含对齐与块样式）展开为全宽度的最终显示行（已折行、已上色）。
/// 结果供 [`render_frame`] 使用，并由 [`Conversation`] 缓存。
pub(crate) fn build_display_lines(
    conv_lines: &[(Line<'static>, Align, Option<BlockStyle>)],
    conv_width: u16,
) -> Vec<Line<'static>> {
    let mut display_lines: Vec<Line<'static>> = Vec::new();
    for (line, align, block) in conv_lines {
        let gutter = if *align == Align::Right { None } else { *block };
        let reserve = if gutter.is_some() {
            constants::GUTTER_W + constants::BLOCK_RIGHT_PAD
        } else {
            0
        };
        let base_width = (conv_width as usize).saturating_sub(reserve).max(1);
        let wrapped = wrap_line(line, base_width);
        for wline in wrapped {
            if *align == Align::Right {
                let effective_width =
                    conv_width.saturating_sub(constants::USER_MARGIN as u16) as usize;
                let re_wrapped = wrap_line(&wline, effective_width.max(1));
                for rline in re_wrapped {
                    let dw: usize = spans_display_width(&rline.spans);
                    let mut spans: Vec<Span> = Vec::new();
                    if let Some(bs) = gutter {
                        spans.push(block_gutter_span(bs));
                    }
                    let block_bg = block.and_then(|bs| bs.bg);
                    let target =
                        (conv_width as usize).saturating_sub(constants::USER_EDGE_MARGIN);
                    let pad = target.saturating_sub(dw);
                    if pad > 0 {
                        let pad_style = if let Some(bg) = block_bg {
                            Style::default().bg(bg)
                        } else {
                            Style::default()
                        };
                        spans.push(Span::styled(" ".repeat(pad), pad_style));
                    }
                    spans.extend(rline.spans.iter().map(|s| {
                        Span::styled(s.content.clone(), with_block_background(s.style, block_bg))
                    }));
                    if let Some(bg_color) = block_bg {
                        let full_width = conv_width as usize;
                        let current_w: usize = spans_display_width(&spans);
                        if current_w < full_width {
                            spans.push(Span::styled(
                                " ".repeat(full_width - current_w),
                                Style::default().bg(bg_color),
                            ));
                        }
                    }
                    display_lines.push(Line::from(spans));
                }
            } else {
                let mut spans: Vec<Span> = Vec::new();
                if let Some(bs) = gutter {
                    spans.push(block_gutter_span(bs));
                }
                let block_bg = block.and_then(|bs| bs.bg);
                spans.extend(wline.spans.iter().map(|s| {
                    Span::styled(s.content.clone(), with_block_background(s.style, block_bg))
                }));
                if let Some(bg_color) = block_bg {
                    let full_width = conv_width as usize;
                    let current_w: usize = spans_display_width(&spans);
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
    display_lines
}

pub(crate) fn render_frame(frame: &mut Frame, state: FrameRenderState<'_>) {
    let FrameRenderState {
        scroll_view,
        conv_state,
        editor,
        menu,
        separator_style,
        dim_style,
    } = state;
    let area = frame.area();
    if area.width < constants::MIN_TERMINAL_WIDTH || area.height < constants::MIN_TERMINAL_HEIGHT {
        return;
    }
    frame.render_widget(
        Block::default().style(crate::ui::style::app_background()),
        area,
    );

    let input_rows = editor
        .textarea
        .lines()
        .iter()
        .map(|line| wrap_line(&Line::from(line.as_str()), area.width as usize).len())
        .sum::<usize>()
        .clamp(1, constants::MAX_TEXT_ROWS) as u16;
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
        // 自动跟随：上一帧位于底部时，本帧钉到新内容底部（流式输出持续追底）。
        if conv_state.is_at_bottom() {
            let bottom = scroll_view.size().height.saturating_sub(conv_area.height);
            conv_state.set_offset(Position::new(0, bottom));
        }
        frame.render_stateful_widget(scroll_view, conv_area, &mut *conv_state);
        if !conv_state.is_at_bottom() {
            let area = Rect::new(
                conv_area.x,
                conv_area.y + conv_area.height.saturating_sub(1),
                conv_area.width,
                1,
            );
            frame.render_widget(Clear, area);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    crate::t!("render-scroll-indicator"),
                    dim_style,
                )))
                .style(crate::ui::style::app_background()),
                area,
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
    if edit_area.height > 0 && edit_area.width > 0 {
        frame.render_widget(&editor.textarea, edit_area);
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

    // 悬浮菜单最后渲染，覆盖在主界面之上。
    if menu.is_open() {
        menu.render(frame, area);
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
    fn scroll_view_follows_bottom_and_flags_scrolled_up() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::layout::Size;
        use tui_widgets::scrollview::ScrollbarVisibility;

        let lines: Vec<Line<'static>> = (0..20)
            .map(|i| Line::from(if i == 17 { "x".repeat(30) } else { format!("line{i}") }))
            .collect();
        let mut view = ScrollView::new(Size::new(30, lines.len() as u16))
            .scrollbars_visibility(ScrollbarVisibility::Never);
        let view_area = view.area();
        view.render_widget(Paragraph::new(lines), view_area);

        let editor = Editor::new();
        let mut menu = Menu::new();
        let mut state = ScrollViewState::new();
        let mut terminal = Terminal::new(TestBackend::new(30, 10)).unwrap();
        let mut draw = |state: &mut ScrollViewState| {
            terminal
                .draw(|frame| {
                    render_frame(
                        frame,
                        FrameRenderState {
                            scroll_view: &view,
                            conv_state: state,
                            editor: &editor,
                            menu: &mut menu,
                            separator_style: Style::default(),
                            dim_style: Style::default(),
                        },
                    );
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };

        // 初始位于底部：最新一行可见，无提示条。
        let screen = draw(&mut state);
        assert!(screen.contains("line19"), "底部应显示最新行:\n{screen}");
        assert!(!screen.contains("PageDown"), "底部不应显示提示条");

        // 向上滚动后：最新行不可见，提示条出现。
        state.scroll_up();
        state.scroll_up();
        let screen = draw(&mut state);
        assert!(
            !screen.contains("line19"),
            "上滚后不应显示最新行:\n{screen}"
        );
        assert!(screen.contains("PageDown"), "上滚后应显示提示条:\n{screen}");
        assert!(
            !screen.contains('x'),
            "提示条不应残留底层字符:\n{screen}"
        );
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
