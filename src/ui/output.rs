use crate::error::{ErrorKind, TogiError};

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
            message: error.to_string(),
        }
    }
}
