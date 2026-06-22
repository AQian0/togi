//! 全屏对话 UI 子系统入口。
//!
//! 本模块为 facade，将 [`Session`]、[`OutputItem`]、[`SectionKind`] 重新导出，
//! 具体实现分散在以下子模块中：
//! - [`session`]：Session 生命周期、事件循环、输入输出、渲染。
//! - [`keys`]：按键分发与退出命令识别。
//! - [`terminal`]：终端模式管理（raw mode / alternate screen）与事件轮询。

pub use crate::ui::session::Session;
pub use crate::ui::{OutputItem, SectionKind};

#[cfg(test)]
mod tests {
    use super::super::keys::is_exit_command;

    #[test]
    fn exit_commands_recognised() {
        assert!(is_exit_command("exit"));
        assert!(!is_exit_command("hello"));
    }
}
