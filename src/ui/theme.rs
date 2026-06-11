//! Catppuccin 主题系统。
//!
//! 包含全部四种风味：Latte、Frappe、Macchiato、Mocha。
//! 默认使用 Latte。

use crate::error::{ErrorKind, TogiError};
use ratatui::style::Color;
use std::str::FromStr;
use std::sync::LazyLock;
use std::sync::OnceLock;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatppuccinFlavor {
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl CatppuccinFlavor {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Latte => "Latte",
            Self::Frappe => "Frappe",
            Self::Macchiato => "Macchiato",
            Self::Mocha => "Mocha",
        }
    }

    pub fn colors(&self) -> &'static CatppuccinColors {
        match self {
            Self::Latte => &LATTE,
            Self::Frappe => &FRAPPE,
            Self::Macchiato => &MACCHIATO,
            Self::Mocha => &MOCHA,
        }
    }
}

#[allow(dead_code)]
pub struct CatppuccinColors {
    pub rosewater: Color,
    pub flamingo: Color,
    pub pink: Color,
    pub mauve: Color,
    pub red: Color,
    pub maroon: Color,
    pub peach: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub sky: Color,
    pub sapphire: Color,
    pub blue: Color,
    pub lavender: Color,
    pub text: Color,
    pub subtext1: Color,
    pub subtext0: Color,
    pub overlay2: Color,
    pub overlay1: Color,
    pub overlay0: Color,
    pub surface2: Color,
    pub surface1: Color,
    pub surface0: Color,
    pub base: Color,
    pub mantle: Color,
    pub crust: Color,
}

fn cp(hex: &str) -> Color {
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap();
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap();
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap();
    Color::Rgb(r, g, b)
}

static LATTE: LazyLock<CatppuccinColors> = LazyLock::new(|| CatppuccinColors {
    rosewater: cp("dc8a78"),
    flamingo: cp("dd7878"),
    pink: cp("ea76cb"),
    mauve: cp("8839ef"),
    red: cp("d20f39"),
    maroon: cp("e64553"),
    peach: cp("fe640b"),
    yellow: cp("df8e1d"),
    green: cp("40a02b"),
    teal: cp("179299"),
    sky: cp("04a5e5"),
    sapphire: cp("209fb3"),
    blue: cp("1e66f5"),
    lavender: cp("7287fd"),
    text: cp("4c4f69"),
    subtext1: cp("5c5f77"),
    subtext0: cp("6c6f85"),
    overlay2: cp("7c7f93"),
    overlay1: cp("8c8fa1"),
    overlay0: cp("9ca0b0"),
    surface2: cp("acb0be"),
    surface1: cp("bcc0cc"),
    surface0: cp("ccd0da"),
    base: cp("eff1f5"),
    mantle: cp("e6e9ef"),
    crust: cp("dce0e8"),
});

static FRAPPE: LazyLock<CatppuccinColors> = LazyLock::new(|| CatppuccinColors {
    rosewater: cp("f2d5d2"),
    flamingo: cp("eebebe"),
    pink: cp("f4b8e4"),
    mauve: cp("ca9ee6"),
    red: cp("e78284"),
    maroon: cp("ea999c"),
    peach: cp("ef9f76"),
    yellow: cp("e5c890"),
    green: cp("a6d189"),
    teal: cp("81c8be"),
    sky: cp("99d1db"),
    sapphire: cp("85c1dc"),
    blue: cp("8caaee"),
    lavender: cp("babbf1"),
    text: cp("c6d0f5"),
    subtext1: cp("b5bfe2"),
    subtext0: cp("a5adce"),
    overlay2: cp("949cbb"),
    overlay1: cp("838ba7"),
    overlay0: cp("737994"),
    surface2: cp("626880"),
    surface1: cp("51576d"),
    surface0: cp("414559"),
    base: cp("303446"),
    mantle: cp("292c3c"),
    crust: cp("232634"),
});

static MACCHIATO: LazyLock<CatppuccinColors> = LazyLock::new(|| CatppuccinColors {
    rosewater: cp("f4dbd6"),
    flamingo: cp("f0c6c6"),
    pink: cp("f5bde6"),
    mauve: cp("c6a0f6"),
    red: cp("ed8796"),
    maroon: cp("ee99a0"),
    peach: cp("f5a97f"),
    yellow: cp("eed49f"),
    green: cp("a6da95"),
    teal: cp("8bd5ca"),
    sky: cp("91d7e3"),
    sapphire: cp("7dc4e4"),
    blue: cp("8aadf4"),
    lavender: cp("b7bdf8"),
    text: cp("cad3f5"),
    subtext1: cp("b8c0e0"),
    subtext0: cp("a5adcb"),
    overlay2: cp("939ab7"),
    overlay1: cp("8087a2"),
    overlay0: cp("6e738d"),
    surface2: cp("5b6078"),
    surface1: cp("494d64"),
    surface0: cp("363a4f"),
    base: cp("24273a"),
    mantle: cp("1e2030"),
    crust: cp("181926"),
});

static MOCHA: LazyLock<CatppuccinColors> = LazyLock::new(|| CatppuccinColors {
    rosewater: cp("f5e0dc"),
    flamingo: cp("f2cdcd"),
    pink: cp("f5c2e7"),
    mauve: cp("cba6f7"),
    red: cp("f38ba8"),
    maroon: cp("eba0ac"),
    peach: cp("fab387"),
    yellow: cp("f9e2af"),
    green: cp("a6e3a1"),
    teal: cp("94e2d5"),
    sky: cp("89dceb"),
    sapphire: cp("74c7ec"),
    blue: cp("89b4fa"),
    lavender: cp("b4befe"),
    text: cp("cdd6f4"),
    subtext1: cp("bac2de"),
    subtext0: cp("a6adc8"),
    overlay2: cp("9399b2"),
    overlay1: cp("7f849c"),
    overlay0: cp("6c7086"),
    surface2: cp("585b70"),
    surface1: cp("45475a"),
    surface0: cp("313244"),
    base: cp("1e1e2e"),
    mantle: cp("181825"),
    crust: cp("11111b"),
});

#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error(
        "unsupported theme flavor '{input}', available flavors: Latte, Frappe, Macchiato, Mocha"
    )]
    UnsupportedFlavor { input: String },
}

impl TogiError for ThemeError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFlavor { .. } => "theme.unsupported_flavor",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::UnsupportedFlavor { .. } => ErrorKind::InvalidArgument,
        }
    }
}

impl FromStr for CatppuccinFlavor {
    type Err = ThemeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "latte" => Ok(Self::Latte),
            "frappe" => Ok(Self::Frappe),
            "macchiato" => Ok(Self::Macchiato),
            "mocha" => Ok(Self::Mocha),
            _ => Err(ThemeError::UnsupportedFlavor {
                input: s.to_string(),
            }),
        }
    }
}

static ACTIVE_THEME: OnceLock<CatppuccinFlavor> = OnceLock::new();

pub fn set_flavor(flavor: CatppuccinFlavor) {
    let _ = ACTIVE_THEME.set(flavor);
}

pub fn flavor() -> CatppuccinFlavor {
    *ACTIVE_THEME.get_or_init(|| CatppuccinFlavor::Latte)
}

pub fn c() -> &'static CatppuccinColors {
    flavor().colors()
}
