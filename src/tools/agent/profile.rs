//! `.togi/agents/*.md` 子代理定义加载器（路线图 Q3：文件定义，格式后续可换）。
//!
//! 文件格式：`---` 围起的 TOML frontmatter 声明元数据，正文作为该代理的
//! preamble。frontmatter 字段均可选：
//!
//! ```toml
//! description = "只读代码分析"        # 展示给父代理，用于选择 profile
//! model = "deepseek-chat"           # 缺省用主代理模型
//! tools = ["read", "shell"]         # 工具白名单，缺省 read + shell
//! ```

use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

/// 一个子代理定义：工具白名单 + 可选模型 + 作为 preamble 的正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub description: String,
    pub model: Option<String>,
    pub tools: Vec<String>,
    pub preamble: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Frontmatter {
    description: String,
    model: Option<String>,
    tools: Vec<String>,
}

/// 解析单个 profile 文件内容；`name` 为文件名（不含扩展名），仅用于错误信息。
pub fn parse_profile(name: &str, content: &str) -> Result<Profile, String> {
    let (frontmatter, body) = split_frontmatter(content);
    let fm: Frontmatter = match frontmatter {
        Some(block) => toml::from_str(block)
            .map_err(|e| format!("profile `{name}` has invalid TOML frontmatter: {e}"))?,
        None => Frontmatter::default(),
    };
    Ok(Profile {
        description: fm.description,
        model: fm.model,
        tools: fm.tools,
        preamble: body.trim().to_string(),
    })
}

/// 切分 frontmatter 与正文：文件以 `---` 行开头、且存在闭合 `---` 行时
/// 视为带 frontmatter，否则整体作为正文。
fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let content = content.trim_start();
    let Some(after_open) = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
    else {
        return (None, content);
    };
    let mut search_from = 0;
    while let Some(pos) = after_open[search_from..].find("\n---") {
        let line_start = search_from + pos + 1;
        let line_end = after_open[line_start..]
            .find('\n')
            .map(|e| line_start + e)
            .unwrap_or(after_open.len());
        if after_open[line_start..line_end].trim() == "---" {
            return (
                Some(after_open[..line_start].trim()),
                &after_open[line_end..],
            );
        }
        search_from = line_start + 3;
    }
    (None, content)
}

/// 加载目录下全部 `*.md` profile，键为文件名（不含扩展名）。
/// 目录不存在或单个文件损坏时跳过并告警，不影响其余 profile。
pub fn load_profiles(dir: &Path) -> HashMap<String, Profile> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(name) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|content| parse_profile(&name, &content))
        {
            Ok(profile) => {
                out.insert(name, profile);
            }
            Err(err) => eprintln!("togi: skipping agent profile {}: {err}", path.display()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let content = "---\ndescription = \"只读分析\"\nmodel = \"deepseek-chat\"\ntools = [\"read\"]\n---\n你是分析代理。\n";
        let p = parse_profile("reader", content).unwrap();
        assert_eq!(p.description, "只读分析");
        assert_eq!(p.model.as_deref(), Some("deepseek-chat"));
        assert_eq!(p.tools, vec!["read"]);
        assert_eq!(p.preamble, "你是分析代理。");
    }

    #[test]
    fn body_without_frontmatter_becomes_preamble_with_defaults() {
        let p = parse_profile("plain", "直接是正文。\n第二行").unwrap();
        assert_eq!(p.preamble, "直接是正文。\n第二行");
        assert!(p.model.is_none());
        assert!(p.tools.is_empty());
        assert!(p.description.is_empty());
    }

    #[test]
    fn invalid_toml_frontmatter_is_an_error() {
        let err = parse_profile("bad", "---\nmodel = [unclosed\n---\n正文").unwrap_err();
        assert!(err.contains("bad"), "{err}");
    }

    #[test]
    fn unclosed_frontmatter_treated_as_body() {
        let p = parse_profile("x", "---\nmodel = \"a\"\n没有闭合").unwrap();
        assert!(p.preamble.contains("没有闭合"));
        assert!(p.model.is_none());
    }

    #[test]
    fn dashes_inside_frontmatter_value_not_treated_as_fence() {
        let content = "---\ndescription = \"a---b\"\n---\n正文";
        let p = parse_profile("x", content).unwrap();
        assert_eq!(p.description, "a---b");
        assert_eq!(p.preamble, "正文");
    }

    #[test]
    fn load_profiles_skips_non_md_and_broken_files() {
        let dir = std::env::temp_dir().join(format!("togi_profiles_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reader.md"), "---\ndescription = \"d\"\n---\n正文").unwrap();
        std::fs::write(dir.join("notes.txt"), "忽略我").unwrap();
        std::fs::write(dir.join("broken.md"), "---\nmodel = [bad\n---\n正文").unwrap();

        let profiles = load_profiles(&dir);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles["reader"].description, "d");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_profiles_missing_dir_returns_empty() {
        assert!(load_profiles(std::path::Path::new("/tmp/togi_no_such_agents_dir")).is_empty());
    }
}
