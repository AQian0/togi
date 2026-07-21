//! 工具装饰管道：对 [`rig::tool::ToolDyn`] 进行分层包装。
//!
//! - [`inject`]：向工具调用注入运行时参数（如工作目录）。
//! - [`paginate`]：为工具输出添加按行分页能力。

pub mod inject;
pub(crate) mod paginate;
