pub mod agent;
pub mod modify;
pub mod read;
pub mod shell;

use serde_json::Value;
use std::collections::HashMap;

/// 工具调用的副作用类别——决定是否确认及结果如何展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEffect {
    /// 只读 / 查询：无需确认，仅展示行为与简短摘要。
    ReadOnly,
    /// 有副作用（写入、变更等）：执行前确认，完整展示结果。
    Mutating,
    /// 无直接副作用，但结果需完整展示（如委派结论、dry-run diff）。
    ReadOnlyVerbose,
}

/// 用户对一次变更类工具调用的决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    AllowOnce,
    AlwaysAllow,
    Deny,
}

/// 由具体工具声明其副作用类别与名称，供 [`ToolRegistry`] 收集。
///
/// 默认视为 [`ToolEffect::Mutating`]；只读 / 查询型工具覆盖 [`Self::classify`]。
/// 每个工具必须提供 [`Self::name`] 作为注册表键值。
/// 这样"是否只读"的判断完全内置于工具、在代码中确定，不依赖 LLM 自述——
/// 既可靠（同一调用恒定分类）又可单测，对 `shell` 这类按命令而定的工具，
/// 也由工具自身检视参数得出，而非交给模型判断。
pub trait ClassifyEffect {
    /// 工具的唯一名称，对应其 `Tool::NAME`。
    fn name() -> &'static str;

    /// 根据调用参数判定本次调用的副作用类别。
    #[must_use]
    fn classify(_args: &Value) -> ToolEffect {
        ToolEffect::Mutating
    }
}

/// 工具注册表：收集所有工具的副作用分类器，消除硬编码的分派。
///
/// 在构建工具时，通过 [`Self::register`] 将每个工具的分类函数登记到表中。
/// 运行时通过 [`Self::classify`] 按名称查找并调用对应的分类器。
/// 未知名称默认返回 [`ToolEffect::Mutating`]（安全保守策略）。
#[derive(Debug, Clone)]
pub struct ToolRegistry {
    classifiers: HashMap<&'static str, fn(&Value) -> ToolEffect>,
}

impl ToolRegistry {
    /// 创建一个空的注册表。
    #[must_use]
    pub fn new() -> Self {
        Self {
            classifiers: HashMap::new(),
        }
    }

    /// 注册一个工具类型。调用后将工具的 [`ClassifyEffect::classify`] 函数
    /// 与该工具的 [`ClassifyEffect::name`] 绑定。
    pub fn register<T: ClassifyEffect>(&mut self) {
        self.classifiers.insert(T::name(), T::classify);
    }

    /// 根据工具名称和参数判定副作用类别。
    #[must_use]
    pub fn classify(&self, name: &str, args: &Value) -> ToolEffect {
        self.classifiers
            .get(name)
            .map(|f| f(args))
            .unwrap_or(ToolEffect::Mutating)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_classify_read_as_read_only() {
        let mut registry = ToolRegistry::new();
        registry.register::<read::Read>();
        let args = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(registry.classify("read", &args), ToolEffect::ReadOnly);
    }

    #[test]
    fn registry_classify_modify_as_mutating() {
        let mut registry = ToolRegistry::new();
        registry.register::<modify::Modify>();
        let args = serde_json::json!({ "path": "a.txt", "content": "x" });
        assert_eq!(registry.classify("modify", &args), ToolEffect::Mutating);
    }

    #[test]
    fn registry_classify_modify_dry_run_as_read_only_verbose() {
        let mut registry = ToolRegistry::new();
        registry.register::<modify::Modify>();
        let args = serde_json::json!({ "path": "a.txt", "content": "x", "dry_run": true });
        assert_eq!(
            registry.classify("modify", &args),
            ToolEffect::ReadOnlyVerbose
        );
    }

    #[test]
    fn registry_classify_agent_as_read_only_verbose() {
        let mut registry = ToolRegistry::new();
        registry.register::<agent::AgentTool>();
        assert_eq!(
            registry.classify("agent", &serde_json::json!({ "task": "inspect" })),
            ToolEffect::ReadOnlyVerbose
        );
    }

    #[test]
    fn registry_classify_shell_query_as_read_only_but_mutation_is_not() {
        let mut registry = ToolRegistry::new();
        registry.register::<shell::Shell>();
        let query = serde_json::json!({ "command": "ls -la | grep rs" });
        assert_eq!(registry.classify("shell", &query), ToolEffect::ReadOnly);
        let mutate = serde_json::json!({ "command": "rm -rf target" });
        assert_eq!(registry.classify("shell", &mutate), ToolEffect::Mutating);
        let custom_env = serde_json::json!({ "command": "echo $VALUE", "env": { "VALUE": "x" } });
        assert_eq!(
            registry.classify("shell", &custom_env),
            ToolEffect::Mutating
        );
    }

    #[test]
    fn registry_unknown_tool_defaults_to_mutating() {
        let registry = ToolRegistry::new();
        let args = serde_json::json!({});
        assert_eq!(
            registry.classify("does-not-exist", &args),
            ToolEffect::Mutating
        );
    }
}
