//! 主题与样式定义。
//!
//! 所有颜色、样式和语法高亮配置集中于此。颜色值来自 Catppuccin 主题系统。

use crate::ui::theme;
use ratatui::style::{Color, Modifier, Style};
use std::sync::OnceLock;
use syntect::highlighting::{FontStyle, Theme};

pub fn c() -> &'static theme::CatppuccinColors {
    theme::c()
}

pub fn app_background() -> Style {
    Style::default().bg(c().crust).fg(c().text)
}

pub fn input_background() -> Style {
    Style::default().bg(c().base).fg(c().text)
}

pub fn normal() -> Style {
    Style::default().fg(c().text)
}

pub fn dim() -> Style {
    Style::default().fg(c().overlay0)
}

pub fn separator() -> Style {
    Style::default().bg(c().base).fg(c().surface1)
}

pub fn error() -> Style {
    Style::default().fg(c().red)
}

pub fn assistant_block() -> Style {
    Style::default()
        .bg(c().mantle)
        .fg(c().green)
        .add_modifier(Modifier::BOLD)
}

pub fn thinking_block() -> Style {
    Style::default()
        .bg(c().base)
        .fg(c().overlay1)
        .add_modifier(Modifier::ITALIC)
}

pub fn user_block() -> Style {
    Style::default()
        .bg(c().surface0)
        .fg(c().blue)
        .add_modifier(Modifier::BOLD)
}

pub fn tool_call_block() -> Style {
    Style::default()
        .bg(c().mantle)
        .fg(c().text)
        .add_modifier(Modifier::BOLD)
}

pub fn tool_result_block() -> Style {
    Style::default().bg(c().mantle).fg(c().subtext0)
}

pub fn gutter_of(style: Style) -> Color {
    style.fg.unwrap_or_else(|| c().text)
}

pub fn md_base() -> Style {
    Style::default().fg(c().text)
}

pub fn md_heading() -> Style {
    Style::default()
        .fg(c().lavender)
        .add_modifier(Modifier::BOLD)
}

pub fn md_bold() -> Style {
    Style::default().fg(c().text).add_modifier(Modifier::BOLD)
}

pub fn md_italic() -> Style {
    Style::default().fg(c().text).add_modifier(Modifier::ITALIC)
}

pub fn md_inline_code() -> Style {
    Style::default().bg(c().surface0).fg(c().pink)
}

pub fn md_code_block() -> Style {
    Style::default().fg(c().teal)
}

pub fn md_code_border() -> Style {
    Style::default().fg(c().surface1)
}

pub fn md_link() -> Style {
    Style::default()
        .fg(c().blue)
        .add_modifier(Modifier::UNDERLINED)
}

pub fn md_quote() -> Style {
    Style::default()
        .fg(c().overlay0)
        .add_modifier(Modifier::ITALIC)
}

pub fn md_list_bullet() -> Style {
    Style::default().fg(c().overlay1)
}

static SYNTAX_SET: OnceLock<syntect::parsing::SyntaxSet> = OnceLock::new();
static HIGHLIGHT_THEME: OnceLock<Theme> = OnceLock::new();

pub fn syntax_set() -> &'static syntect::parsing::SyntaxSet {
    SYNTAX_SET.get_or_init(syntect::parsing::SyntaxSet::load_defaults_newlines)
}

pub fn highlight_theme() -> &'static Theme {
    HIGHLIGHT_THEME.get_or_init(build_syntect_theme)
}

fn build_syntect_theme() -> Theme {
    use syntect::highlighting::{Color as SynColor, FontStyle as SynFontStyle, StyleModifier};
    use syntect::highlighting::{ScopeSelector, ScopeSelectors, ThemeItem};
    use syntect::parsing::{Scope, ScopeStack};

    let c = theme::c();

    fn syn_c(color: Color) -> SynColor {
        SynColor {
            r: to_u8(color, 0),
            g: to_u8(color, 1),
            b: to_u8(color, 2),
            a: 255,
        }
    }

    fn item(scope: &str, fg: Color, fs: Option<SynFontStyle>) -> ThemeItem {
        ThemeItem {
            scope: ScopeSelectors {
                selectors: vec![ScopeSelector {
                    path: ScopeStack::from_vec(vec![Scope::new(scope).unwrap()]),
                    excludes: vec![],
                }],
            },
            style: StyleModifier {
                foreground: Some(syn_c(fg)),
                background: None,
                font_style: fs,
            },
        }
    }

    let mut scopes: Vec<ThemeItem> = Vec::new();

    scopes.push(item("comment", c.overlay0, Some(SynFontStyle::ITALIC)));
    scopes.push(item("comment.line", c.overlay0, Some(SynFontStyle::ITALIC)));
    scopes.push(item(
        "comment.block",
        c.overlay0,
        Some(SynFontStyle::ITALIC),
    ));

    scopes.push(item("string", c.green, None));
    scopes.push(item("string.quoted", c.green, None));
    scopes.push(item("string.regexp", c.peach, None));

    scopes.push(item("constant", c.peach, None));
    scopes.push(item("constant.numeric", c.peach, None));
    scopes.push(item("constant.language", c.peach, None));
    scopes.push(item("constant.character", c.teal, None));
    scopes.push(item("constant.character.escape", c.pink, None));

    scopes.push(item("keyword", c.mauve, None));
    scopes.push(item("keyword.control", c.mauve, None));
    scopes.push(item("keyword.operator", c.sky, None));
    scopes.push(item("keyword.other", c.mauve, None));

    scopes.push(item("storage", c.mauve, None));
    scopes.push(item("storage.type", c.mauve, None));
    scopes.push(item("storage.modifier", c.mauve, None));

    scopes.push(item("entity.name.function", c.blue, None));
    scopes.push(item("entity.name.type", c.yellow, None));
    scopes.push(item("entity.name.tag", c.blue, None));
    scopes.push(item("entity.name.section", c.lavender, None));
    scopes.push(item("entity.other.inherited-class", c.yellow, None));
    scopes.push(item("entity.other.attribute-name", c.teal, None));

    scopes.push(item("support.function", c.blue, None));
    scopes.push(item("support.function.builtin", c.blue, None));
    scopes.push(item("support.class", c.teal, None));
    scopes.push(item("support.type", c.teal, None));
    scopes.push(item("support.constant", c.peach, None));
    scopes.push(item("support.variable", c.text, None));

    scopes.push(item("variable", c.text, None));
    scopes.push(item("variable.parameter", c.text, None));
    scopes.push(item("variable.language", c.mauve, None));
    scopes.push(item("variable.other", c.text, None));

    scopes.push(item("markup.heading", c.lavender, Some(SynFontStyle::BOLD)));
    scopes.push(item("markup.bold", c.text, Some(SynFontStyle::BOLD)));
    scopes.push(item("markup.italic", c.text, Some(SynFontStyle::ITALIC)));
    scopes.push(item(
        "markup.underline",
        c.text,
        Some(SynFontStyle::UNDERLINE),
    ));
    scopes.push(item("markup.quote", c.overlay0, Some(SynFontStyle::ITALIC)));
    scopes.push(item("markup.raw", c.teal, None));
    scopes.push(item("markup.link", c.blue, None));
    scopes.push(item("markup.list", c.text, None));
    scopes.push(item("markup.inserted", c.green, None));
    scopes.push(item("markup.deleted", c.red, None));
    scopes.push(item("markup.changed", c.yellow, None));

    scopes.push(item("invalid", c.red, None));
    scopes.push(item("invalid.deprecated", c.overlay0, None));

    scopes.push(item("punctuation", c.overlay1, None));
    scopes.push(item("punctuation.section", c.overlay0, None));
    scopes.push(item("punctuation.definition.tag", c.overlay1, None));
    scopes.push(item("punctuation.definition.string", c.green, None));

    scopes.push(item("meta.function", c.text, None));
    scopes.push(item("meta.class", c.text, None));
    scopes.push(item("meta.struct", c.text, None));
    scopes.push(item("meta.preprocessor", c.pink, None));
    scopes.push(item("meta.diff", c.text, None));
    scopes.push(item("meta.diff.header", c.lavender, None));

    Theme {
        name: Some(format!("Catppuccin {}", theme::flavor().name())),
        author: Some("Catppuccin".into()),
        settings: syntect::highlighting::ThemeSettings {
            background: Some(syn_c(c.base)),
            foreground: Some(syn_c(c.text)),
            caret: Some(syn_c(c.lavender)),
            line_highlight: Some(syn_c(c.surface0)),
            selection: Some(syn_c(c.surface1)),
            misspelling: Some(syn_c(c.red)),
            ..Default::default()
        },
        scopes,
    }
}

fn to_u8(c: Color, idx: usize) -> u8 {
    match c {
        Color::Rgb(r, g, b) => [r, g, b][idx],
        _ => 0,
    }
}

pub fn syntect_to_ratatui(style: syntect::highlighting::Style) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
    let bg = Color::Rgb(style.background.r, style.background.g, style.background.b);
    let mut s = Style::default().fg(fg).bg(bg);
    if style.font_style.contains(FontStyle::BOLD) {
        s = s.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        s = s.add_modifier(Modifier::UNDERLINED);
    }
    s
}

pub fn find_syntax(lang: &str) -> Option<&'static syntect::parsing::SyntaxReference> {
    if lang.is_empty() {
        return None;
    }
    let ss = syntax_set();
    ss.find_syntax_by_token(lang)
        .or_else(|| ss.find_syntax_by_extension(lang))
        .or_else(|| ss.find_syntax_by_first_line(lang))
}
