//! 统一错误语义。
//!
//! 具体模块仍保留自己的 `thiserror` enum；本模块只提供跨模块稳定的
//! 错误分类、错误码约定和应用入口错误。这样不会破坏工具面向模型的
//! 具体 Display 文案，同时为日志、测试和未来 UI 结构化展示保留语义。

#[allow(dead_code)]
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
/// `Display` 仍负责给用户/模型看的可读文案；`code` 和 `kind` 负责稳定分类。
#[allow(dead_code)]
pub trait TogiError: std::error::Error {
    /// 稳定错误码。使用小写点分格式，例如 `read.not_found`。
    fn code(&self) -> &'static str;

    /// 粗粒度错误分类，用于 UI、日志、重试策略等。
    fn kind(&self) -> ErrorKind;

    /// 是否值得在相同输入之外重试。默认只对环境性错误返回 true。
    fn retryable(&self) -> bool {
        matches!(
            self.kind(),
            ErrorKind::PermissionDenied | ErrorKind::Timeout | ErrorKind::Io | ErrorKind::External
        )
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

#[allow(dead_code)]
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

    #[error(
        "无法初始化默认模型 (deepseek-v4-pro)：{source}\n请设置 DEEPSEEK_API_KEY 环境变量，或在 togi.toml 中通过 system.model 指定其他模型。"
    )]
    DefaultModelInit {
        #[source]
        source: crate::agent::AgentError,
    },

    #[error("后台任务失败：{0}")]
    TaskJoin(#[from] tokio::task::JoinError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("已取消")]
    Cancelled,

    #[error("内部错误：{0}")]
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
            Self::Io(_) => "app.io",
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
            Self::Io(_) => ErrorKind::Io,
            Self::Cancelled => ErrorKind::Cancelled,
            Self::Internal(_) => ErrorKind::Internal,
        }
    }
}
