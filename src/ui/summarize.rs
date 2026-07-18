//! 工具调用摘要等实用函数。
//!
//! 原模块中的终端打印函数（banner, section_header 等）已迁移至 interaction.rs
//! 的 ratatui 全屏渲染管线，本模块仅保留纯数据变换辅助。
use itertools::Itertools;
use serde_json::Value;
pub fn summarize_call(name: &str, value: &Value) -> String {
    match name {
        "read" => value
            .get("path")
            .and_then(Value::as_str)
            .map(truncate_inline)
            .unwrap_or_default(),
        "shell" => value
            .get("command")
            .and_then(Value::as_str)
            .map(|c| format!("$ {}", truncate_inline(c)))
            .unwrap_or_default(),
        "modify" => summarize_modify(value),
        _ => summarize_generic(value),
    }
}
pub fn summarize_modify(value: &Value) -> String {
    let path = value
        .get("path")
        .and_then(Value::as_str)
        .map(truncate_inline)
        .unwrap_or_default();
    let action = if value.get("content_base64").is_some() {
        crate::t!("summarize-write-binary")
    } else if value.get("content").is_some() {
        crate::t!("summarize-write")
    } else if let Some(edits) = value.get("edits").and_then(Value::as_array) {
        let count = edits.len() + usize::from(value.get("old_text").is_some());
        return format!("{path} · {}", crate::t!("summarize-edits", count = count));
    } else if value.get("old_text").is_some() {
        crate::t!("summarize-replace")
    } else {
        String::new()
    };
    if action.is_empty() {
        path
    } else {
        format!("{path} · {action}")
    }
}
pub fn summarize_generic(value: &Value) -> String {
    let Some(obj) = value.as_object() else {
        return String::new();
    };
    let mut parts = Vec::with_capacity(3);
    for (k, v) in obj.iter().take(3) {
        match v {
            Value::String(s) => parts.push(format!("{k}={}", truncate_inline(s))),
            Value::Number(n) => parts.push(format!("{k}={n}")),
            Value::Bool(b) => parts.push(format!("{k}={b}")),
            _ => {}
        }
    }
    parts.join(" ")
}
/// 为只读 / 查询类工具（如 `read`、`shell` 的纯查询命令）的结果
/// 生成不含具体内容的简短摘要。
///
/// 多行结果折叠为行数说明（隐藏具体输出）；单行结果（错误信息、
/// 空文件标记、极短输出等）原样返回，以便错误等状态仍能展示给用户。
///
/// 注意：这只影响对话区的展示，完整内容仍由 rig 内部交给模型，不受影响。
pub fn summarize_readonly_result(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let lines = text.lines().count();
    if lines <= 1 {
        text.trim_end().to_string()
    } else {
        crate::t!("summarize-readonly-result", lines = lines)
    }
}

pub fn truncate_inline(text: &str) -> String {
    let max = crate::shared::constants::SUMMARY_MAX_INLINE_CHARS;
    let collapsed: String = text.split_whitespace().join(" ");
    let mut out: String = collapsed.chars().take(max).collect();
    if collapsed.chars().count() > max {
        out.push('…');
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn truncate_collapses_and_caps() {
        let long = "a".repeat(100);
        let out = truncate_inline(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 73);
    }
    #[test]
    fn truncate_collapses_whitespace() {
        assert_eq!(truncate_inline("foo   bar\n  baz"), "foo bar baz");
    }
    #[test]
    fn read_summary_uses_path() {
        let v = serde_json::json!({"path": "src/main.rs"});
        assert_eq!(summarize_call("read", &v), "src/main.rs");
    }
    #[test]
    fn shell_summary_prefixes_dollar() {
        let v = serde_json::json!({"command": "cargo build"});
        assert_eq!(summarize_call("shell", &v), "$ cargo build");
    }
    #[test]
    fn modify_summary_reports_write() {
        let v = serde_json::json!({"path": "a.txt", "content": "x"});
        let expected = format!("a.txt · {}", crate::t!("summarize-write"));
        assert_eq!(summarize_call("modify", &v), expected);
    }
    #[test]
    fn modify_summary_counts_edits() {
        let v = serde_json::json!({
            "path": "a.txt",
            "edits": [{"old_text": "a", "new_text": "b"}, {"old_text": "c", "new_text": "d"}]
        });
        let expected = format!("a.txt · {}", crate::t!("summarize-edits", count = 2));
        assert_eq!(summarize_call("modify", &v), expected);
    }
    #[test]
    fn modify_summary_reports_replace() {
        let v = serde_json::json!({"path": "a.txt", "old_text": "a", "new_text": "b"});
        let expected = format!("a.txt · {}", crate::t!("summarize-replace"));
        assert_eq!(summarize_call("modify", &v), expected);
    }
    #[test]
    fn readonly_result_collapses_multiline_content() {
        let out = summarize_readonly_result("1 | foo\n2 | bar\n3 | baz");
        let expected = crate::t!("summarize-readonly-result", lines = 3);
        assert_eq!(out, expected);
    }

    #[test]
    fn readonly_result_keeps_single_line_errors() {
        let err = "no such file: `missing.rs`. Double-check the path, then retry.";
        assert_eq!(summarize_readonly_result(err), err);
    }

    #[test]
    fn readonly_result_blank_is_empty() {
        assert_eq!(summarize_readonly_result("   "), "");
    }

    #[test]
    fn generic_summary_joins_scalars() {
        let v = serde_json::json!({"name": "x", "n": 3, "flag": true});
        let out = summarize_call("other", &v);
        assert!(out.contains("name=x"));
        assert!(out.contains("n=3"));
        assert!(out.contains("flag=true"));
    }
}
