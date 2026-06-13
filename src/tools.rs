pub mod modify;
pub mod read;
pub mod shell;

use rig::tool::Tool;
use serde_json::Value;

/// 工具调用的副作用类别——决定其结果在对话区如何展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    /// 只读 / 查询：仅展示行为与简短摘要，不铺开具体内容。
    ReadOnly,
    /// 有副作用（写入、变更等）：完整展示结果。
    Mutating,
}

/// 由具体工具根据“本次调用参数”声明其副作用类别。
///
/// 默认视为 [`ToolEffect::Mutating`]；只读 / 查询型工具覆盖 [`Self::effect`]。
/// 这样“是否只读”的判断完全内置于工具、在代码中确定，不依赖 LLM 自述——
/// 既可靠（同一调用恒定分类）又可单测，对 `shell` 这类按命令而定的工具，
/// 也由工具自身检视参数得出，而非交给模型判断。
pub trait ClassifyEffect {
    fn effect(_args: &Value) -> ToolEffect {
        ToolEffect::Mutating
    }
}

/// 按工具名把副作用判定分派回对应工具。
///
/// 事件转发层只拿得到工具名与参数，借此把判定委托给各工具自身的 [`ClassifyEffect`]。
pub fn classify_call(name: &str, args: &Value) -> ToolEffect {
    match name {
        n if n == read::Read::NAME => read::Read::effect(args),
        n if n == shell::Shell::NAME => shell::Shell::effect(args),
        n if n == modify::Modify::NAME => modify::Modify::effect(args),
        _ => ToolEffect::Mutating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_is_classified_read_only() {
        let args = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(classify_call("read", &args), ToolEffect::ReadOnly);
    }

    #[test]
    fn modify_is_classified_mutating() {
        let args = serde_json::json!({ "path": "a.txt", "content": "x" });
        assert_eq!(classify_call("modify", &args), ToolEffect::Mutating);
    }

    #[test]
    fn shell_query_is_read_only_but_mutation_is_not() {
        let query = serde_json::json!({ "command": "ls -la | grep rs" });
        assert_eq!(classify_call("shell", &query), ToolEffect::ReadOnly);
        let mutate = serde_json::json!({ "command": "rm -rf target" });
        assert_eq!(classify_call("shell", &mutate), ToolEffect::Mutating);
    }

    #[test]
    fn unknown_tool_defaults_to_mutating() {
        let args = serde_json::json!({});
        assert_eq!(classify_call("does-not-exist", &args), ToolEffect::Mutating);
    }
}
