//! 统一错误语义。
//!
//! 具体模块仍保留自己的 `thiserror` enum；本模块只提供跨模块稳定的
//! 错误分类、错误码约定和应用入口错误。这样不会破坏工具面向模型的
//! 具体 Display 文案，同时为日志、测试和未来 UI 结构化展示保留语义。
//!
//! # 约定
//!
//! - `Display`：面向 LLM 的英文技术描述，包含可操作的诊断信息。
//! - [`TogiError::user_message`]：面向终端用户的本地化消息，通过 `t!` 宏翻译。
//! - [`TogiError::code`] / [`TogiError::kind`]：稳定分类，供 UI、日志、重试策略使用。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidArgument,
    MissingRuntimeInjection,
    NotFound,
    NotAFile,
    PermissionDenied,
    Conflict,
    Timeout,
    TooLarge,
    NotUtf8,
    Io,
    External,
    Cancelled,
    Internal,
}

/// 项目内部错误的稳定语义接口。
///
/// 约定（见 [crate::error] 模块文档）：
/// - `Display`：面向 LLM 的英文技术描述。
/// - [`TogiError::user_message`]：面向终端用户的本地化消息。
/// - `code` / `kind`：稳定分类，供 UI、日志、重试策略使用。
pub trait TogiError: std::error::Error {
    /// 稳定错误码。使用小写点分格式，例如 `read.not_found`。
    fn code(&self) -> &'static str;

    /// 粗粒度错误分类，用于 UI、日志、重试策略等。
    fn kind(&self) -> ErrorKind;

    /// 面向终端用户展示的本地化消息。
    ///
    /// 与 `Display`（面向 LLM 的英文技术描述）不同，此方法返回
    /// 适合在终端 UI 中展示的翻译后消息。默认回退到 `Display`。
    fn user_message(&self) -> String {
        self.to_string()
    }

    /// 是否值得在相同输入之外重试。默认只对环境性错误返回 true。
    fn retryable(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::PermissionDenied | ErrorKind::Timeout | ErrorKind::Io | ErrorKind::External
        )
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),

    #[error(transparent)]
    Theme(#[from] crate::ui::theme::ThemeError),

    #[error(transparent)]
    Agent(#[from] crate::agent::AgentError),

    #[error(transparent)]
    Ui(#[from] crate::ui::UiError),

    #[error("failed to initialize default model (deepseek-v4-pro): {source}")]
    DefaultModelInit {
        #[source]
        source: crate::agent::AgentError,
    },

    #[error("background task failed: {0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: std::io::Error,
    },

    #[error("cancelled")]
    Cancelled,

    #[error("internal error: {0}")]
    Internal(String),
}

impl TogiError for AppError {
    fn code(&self) -> &'static str {
        match self {
            Self::Config(err) => err.code(),
            Self::Theme(err) => err.code(),
            Self::Agent(err) => err.code(),
            Self::Ui(err) => err.code(),
            Self::DefaultModelInit { .. } => "app.default_model_init",
            Self::TaskJoin(_) => "app.task_join",
            Self::Io { .. } => "app.io",
            Self::Cancelled => "app.cancelled",
            Self::Internal(_) => "app.internal",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::Config(err) => err.kind(),
            Self::Theme(err) => err.kind(),
            Self::Agent(err) => err.kind(),
            Self::Ui(err) => err.kind(),
            Self::DefaultModelInit { .. } => ErrorKind::External,
            Self::TaskJoin(_) => ErrorKind::Internal,
            Self::Io { .. } => ErrorKind::Io,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::Internal(_) => ErrorKind::Internal,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::Config(err) => err.user_message(),
            Self::Theme(err) => err.user_message(),
            Self::Agent(err) => err.user_message(),
            Self::Ui(err) => err.user_message(),
            Self::DefaultModelInit { source } => {
                crate::t!("error-default-model-init", source = source.to_string())
            }
            Self::TaskJoin(_) | Self::Internal(_) => self.to_string(),
            Self::Io { context, source } => {
                crate::t!("app-io-error", context = (*context).to_string(), error = source.to_string())
            }
            Self::Cancelled => crate::t!("app-cancelled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_error_has_stable_code_and_kind() {
        let err = AppError::Cancelled;
        assert_eq!(err.code(), "app.cancelled");
        assert_eq!(err.kind(), ErrorKind::Cancelled);
        assert!(!err.retryable());
    }

    #[test]
    fn internal_error_has_stable_code_and_kind() {
        let err = AppError::Internal("something broke".to_string());
        assert_eq!(err.code(), "app.internal");
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(!err.retryable());
    }

    #[test]
    fn io_error_carries_context() {
        let err = AppError::Io {
            context: "get current working directory",
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such directory"),
        };
        assert_eq!(err.code(), "app.io");
        assert_eq!(err.kind(), ErrorKind::Io);
        assert!(err.retryable());
        assert!(err.to_string().contains("get current working directory"));
    }
}
