//! 工具调用摘要等实用函数。
//!
//! 原模块中的终端打印函数（banner, section_header 等）已迁移至 interaction.rs
//! 的 ratatui 全屏渲染管线，本模块仅保留纯数据变换辅助。
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
        "写入 · 二进制"
    } else if value.get("content").is_some() {
        "写入"
    } else if let Some(edits) = value.get("edits").and_then(Value::as_array) {
        return format!(
            "{path} · {} 处改动",
            edits.len() + usize::from(value.get("old_text").is_some())
        );
    } else if value.get("old_text").is_some() {
        "替换"
    } else {
        ""
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
    let mut parts = Vec::new();
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
pub fn truncate_inline(text: &str) -> String {
    let max = crate::constants::SUMMARY_MAX_INLINE_CHARS;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
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
        assert_eq!(summarize_call("modify", &v), "a.txt · 写入");
    }
    #[test]
    fn modify_summary_counts_edits() {
        let v = serde_json::json!({
            "path": "a.txt",
            "edits": [{"old_text": "a", "new_text": "b"}, {"old_text": "c", "new_text": "d"}]
        });
        assert_eq!(summarize_call("modify", &v), "a.txt · 2 处改动");
    }
    #[test]
    fn modify_summary_reports_replace() {
        let v = serde_json::json!({"path": "a.txt", "old_text": "a", "new_text": "b"});
        assert_eq!(summarize_call("modify", &v), "a.txt · 替换");
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
