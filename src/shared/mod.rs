//! 共享基础设施：错误语义、常量、跨模块公共辅助函数、i18n、文本编码。
//!
//! 本层不依赖上层业务模块（agent / ui / tools），为全项目提供基础能力。

pub mod error;
pub mod locale;

pub(crate) mod constants;
pub(crate) mod text_encoding;
pub(crate) mod util;

pub use util::FileTooLargeError;
