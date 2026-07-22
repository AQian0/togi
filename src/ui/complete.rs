//! 内置命令补全。
//!
//! 输入以 `/` 开头且尚未输入参数时，Tab 在匹配的命令间循环补全。

/// Tab 循环补全状态：首轮 Tab 计算的候选列表与当前索引。
pub(crate) struct TabCompletion {
    pub matches: Vec<&'static str>,
    pub index: usize,
}

/// 当前输入是否处于命令补全上下文（以 `/` 开头且未含空白字符）。
pub(crate) fn is_command_context(text: &str) -> bool {
    text.starts_with('/') && !text.contains(char::is_whitespace)
}

/// 返回以 `prefix` 开头的命令候选。
pub(crate) fn candidates(prefix: &str) -> Vec<&'static str> {
    crate::cli::builtins::HELP_ROWS
        .iter()
        .flat_map(|(cmds, _)| cmds.split('\u{3001}'))
        .filter(|c| c.starts_with(prefix))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_match_prefix() {
        assert_eq!(candidates("/hel"), vec!["/help"]);
        assert_eq!(candidates("/s"), vec!["/sessions", "/switch"]);
        assert_eq!(candidates("/q"), vec!["/quit"]);
        assert!(candidates("/nope").is_empty());
    }

    #[test]
    fn context_requires_slash_without_args() {
        assert!(is_command_context("/he"));
        assert!(!is_command_context("hello"));
        assert!(!is_command_context("/switch 2"));
        assert!(!is_command_context("/a\nb"));
    }
}
