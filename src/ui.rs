//! 终端 UI 子系统：对话渲染、工具调用摘要。
//!
//! - [`interaction`]：基于 ratatui 的全屏对话 UI（Session / 流式渲染）。
//! - [`editor`]：多行文本编辑组件。
//! - [`render`]：帧渲染与 CJK 折行。
//! - [`summarize`]：工具调用参数的简短摘要，用于对话区展示。

use crate::error::{ErrorKind, TogiError};

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("终端 UI 错误：{0}")]
    Terminal(#[from] std::io::Error),

    #[error("无法保存输入历史：{source}")]
    HistorySave {
        #[source]
        source: std::io::Error,
    },
}

impl TogiError for UiError {
    fn code(&self) -> &'static str {
        match self {
            Self::Terminal(_) => "ui.terminal",
            Self::HistorySave { .. } => "ui.history_save",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::Terminal(_) | Self::HistorySave { .. } => ErrorKind::Io,
        }
    }
}

pub(crate) mod conversation;
pub(crate) mod editor;
pub(crate) mod history;
pub(crate) mod interaction;
pub(crate) mod markdown;
pub(crate) mod output;
pub(crate) mod render;
pub(crate) mod style;
pub(crate) mod summarize;
pub mod theme;
