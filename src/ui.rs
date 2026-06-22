//! 终端 UI 子系统：对话渲染、工具调用摘要。
//!
//! - [`interaction`]：基于 ratatui 的全屏对话 UI（Session / 流式渲染）。
//! - [`session`]：Session 生命周期、事件循环、输入输出、帧渲染。
//! - [`keys`]：按键分发与退出命令识别。
//! - [`terminal`]：终端模式管理与事件轮询。
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

    fn user_message(&self) -> String {
        match self {
            Self::Terminal(source) => {
                crate::t!("app-session-error", error = source.to_string())
            }
            Self::HistorySave { source } => {
                crate::t!("app-history-save-error", error = source.to_string())
            }
        }
    }
}

pub enum OutputItem {
    Section(SectionKind),
    Chunk(String),
    ToolCall { name: String, summary: String },
    ToolResult(String),
    Notice(String),
    Error(ErrorInfo),
    Done,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SectionKind {
    Reasoning,
    Answer,
}

#[derive(Clone)]
pub struct ErrorInfo {
    pub code: &'static str,
    pub kind: ErrorKind,
    pub retryable: bool,
    pub message: String,
}

impl ErrorInfo {
    pub fn from_error(error: &impl TogiError) -> Self {
        Self {
            code: error.code(),
            kind: error.kind(),
            retryable: error.retryable(),
            message: error.user_message(),
        }
    }
}

pub(crate) mod conversation;
pub(crate) mod editor;
pub(crate) mod history;
pub(crate) mod interaction;
pub(crate) mod keys;
pub(crate) mod markdown;
pub(crate) mod render;
pub(crate) mod session;
pub(crate) mod style;
pub(crate) mod summarize;
pub(crate) mod terminal;
pub mod theme;
