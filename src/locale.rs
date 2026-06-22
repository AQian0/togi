//! 多语言（i18n）支持模块。
//!
//! 基于 Mozilla Project Fluent（`fluent-bundle`）实现。FTL 翻译资源在编译期
//! 通过 `include_str!` 嵌入二进制。语言通过 `TOGI_LANG` 或 `LANG` 环境变量
//! 检测，默认为 `zh-CN`。
//!
//! `FluentBundle` 的默认 memoizer 非 `Send + Sync`，因此每次翻译调用都会
//! 重新构造一个 bundle。FTL 文件极小（~1KB），解析开销在实际使用中可忽略。
//!
//! # 使用
//!
//! ```ignore
//! use crate::t;
//!
//! let msg = t!("builtins-clear-done", count = 5);
//! ```

use fluent::{FluentArgs, FluentBundle, FluentResource};
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

// ── 语言检测（仅执行一次） ──────────────────────────────────────────

static LANG: OnceLock<LanguageIdentifier> = OnceLock::new();

/// 将原始 locale 字符串（如 `LANG` 环境变量值）标准化为 BCP 47 语言标识符。
///
/// 处理 Unix `LANG` 常见的 `zh_CN.UTF-8` 格式：截去编码后缀，下划线替换为连字符。
fn normalize_lang(raw: &str) -> Option<LanguageIdentifier> {
    let lang_part = raw.split('.').next().unwrap_or(raw);
    let normalized = lang_part.replace('_', "-");
    normalized.parse().ok()
}

fn lang() -> &'static LanguageIdentifier {
    LANG.get_or_init(|| {
        std::env::var("TOGI_LANG")
            .or_else(|_| std::env::var("LANG"))
            .ok()
            .and_then(|s| normalize_lang(&s))
            .unwrap_or_else(|| "zh-CN".parse().unwrap())
    })
}

// ── FTL 源文本（编译期嵌入） ────────────────────────────────────────

fn ftl_source() -> &'static str {
    match lang().language.as_str() {
        "zh" => include_str!("../locales/zh-CN/main.ftl"),
        _ => include_str!("../locales/en/main.ftl"),
    }
}

fn make_bundle() -> FluentBundle<FluentResource> {
    let mut bundle = FluentBundle::new(vec![lang().clone()]);
    let res = FluentResource::try_new(ftl_source().to_string())
        .expect("Failed to parse FTL resource");
    bundle
        .add_resource(res)
        .expect("Failed to add FTL resource");
    bundle
}

// ── 公共 API ────────────────────────────────────────────────────────

/// 创建一个空的 `FluentArgs`，供 `t!` 宏内部使用。
#[doc(hidden)]
pub fn new_args() -> FluentArgs<'static> {
    FluentArgs::new()
}

/// 获取无参数翻译。缺失 key 会触发 panic。
pub fn tr(key: &str) -> String {
    tr_args(key, &FluentArgs::new())
}

/// 获取带参数翻译。缺失 key 或 pattern 会触发 panic。
pub fn tr_args(key: &str, args: &FluentArgs<'_>) -> String {
    let bundle = make_bundle();
    let msg = bundle
        .get_message(key)
        .unwrap_or_else(|| panic!("missing translation key: {key}"));
    let pattern = msg
        .value()
        .unwrap_or_else(|| panic!("missing value for key: {key}"));
    let mut errors = vec![];
    bundle
        .format_pattern(pattern, Some(args), &mut errors)
        .to_string()
}

// ── 宏 ──────────────────────────────────────────────────────────────

/// 翻译快捷宏。无参数时直接传 key；带参数时使用 `key = value` 形式。
///
/// 参数值会被自动转为 `String`，因此可以安全地传递临时变量。
///
/// ```ignore
/// let s = t!("builtins-help-title");
/// let s = t!("builtins-clear-done", count = 5);
/// let s = t!("error-not-found", path = "src/main.rs");
/// ```
#[macro_export]
macro_rules! t {
    ($key:expr) => {
        $crate::locale::tr($key)
    };
    ($key:expr, $($arg:ident = $val:expr),+ $(,)?) => {{
        let mut __args = $crate::locale::new_args();
        $(
            // 转为 owned String 以避免 borrow 生命周期问题。
            __args.set(stringify!($arg), ($val).to_string());
        )+
        $crate::locale::tr_args($key, &__args)
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_zh_cn_utf8() {
        let id = normalize_lang("zh_CN.UTF-8").unwrap();
        assert_eq!(id.language.as_str(), "zh");
    }

    #[test]
    fn normalize_zh_cn() {
        let id = normalize_lang("zh_CN").unwrap();
        assert_eq!(id.language.as_str(), "zh");
    }

    #[test]
    fn normalize_zh_cn_hyphen() {
        let id = normalize_lang("zh-CN").unwrap();
        assert_eq!(id.language.as_str(), "zh");
    }

    #[test]
    fn normalize_en_us_utf8() {
        let id = normalize_lang("en_US.UTF-8").unwrap();
        assert_eq!(id.language.as_str(), "en");
    }

    #[test]
    fn normalize_en() {
        let id = normalize_lang("en").unwrap();
        assert_eq!(id.language.as_str(), "en");
    }

    #[test]
    fn normalize_togi_lang_format() {
        // TOGI_LANG 可直接使用 BCP 47 格式
        let id = normalize_lang("zh-CN").unwrap();
        assert_eq!(id.to_string(), "zh-CN");
    }

    #[test]
    fn invalid_lang_returns_none() {
        assert!(normalize_lang("not-a-real-language!").is_none());
        assert!(normalize_lang("").is_none());
    }
}
