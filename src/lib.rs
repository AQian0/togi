pub mod agent;
pub mod app;
pub mod config;
pub mod shared;
pub mod store;
pub mod tools;
pub mod ui;

pub(crate) mod cli;
pub(crate) mod pipeline;
pub(crate) mod transform;

// ── 向后兼容的 re-export：保持移动前 lib.rs 顶层的公开路径不变 ──

pub use shared::FileTooLargeError;

pub use shared::error;
pub use shared::locale;

/// 向后兼容的 re-export：移动前 `inject` 是 lib.rs 顶层的公开模块。
pub mod inject {
    pub use crate::pipeline::inject::*;
}
