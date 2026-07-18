//! 工具装饰管道：对 [`rig::tool::ToolDyn`] 进行分层包装。
//!
//! - [`tool_pipeline`]：通用的单工具 / 工具集合形状分发机制。
//! - [`inject`]：向工具调用注入运行时参数（如工作目录）。
//! - [`paginate`]：为工具输出添加按行分页能力。

pub(crate) mod inject;
pub(crate) mod paginate;
pub(crate) mod tool_pipeline;
