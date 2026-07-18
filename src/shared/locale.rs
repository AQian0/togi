//! 多语言（i18n）支持模块。
//!
//! 基于 Mozilla Project Fluent（`fluent-bundle`）实现。FTL 翻译资源在编译期
//! 通过 `include_str!` 嵌入二进制。
//!
//! 语言检测优先级：
//!   1. `TOGI_LANG` 环境变量（显式覆盖）
//!   2. `LC_ALL` → `LC_MESSAGES` → `LANG` 环境变量（Unix 约定）
//!   3. 平台原生 locale（macOS: `defaults read -g AppleLocale`）
//!   4. 默认回退至英文 `en`
//!
//! 检测到的 locale 与内置可用语言按以下策略匹配：
//!   - 精确匹配（如 `zh-CN` → `zh-CN`）
//!   - 同语言回退（如 `zh-TW` / `zh-Hans` → `zh-CN`）
//!   - 最终回退至英文 `en`
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

// ── 内置可用语言 ────────────────────────────────────────────────────

/// 内置可用 locale tag，与 `locales/` 目录下的子目录一一对应。
/// 顺序即同语言回退时的优先级。
const AVAILABLE_TAGS: &[&str] = &["zh-CN", "en"];

/// 按 locale tag 获取编译期嵌入的 FTL 源文本。未匹配返回 `None`。
fn ftl_for_tag(tag: &str) -> Option<&'static str> {
    match tag {
        "zh-CN" => Some(include_str!("../../locales/zh-CN/main.ftl")),
        "en" => Some(include_str!("../../locales/en/main.ftl")),
        _ => None,
    }
}

// ── 语言检测（仅执行一次） ──────────────────────────────────────────

static LANG: OnceLock<LanguageIdentifier> = OnceLock::new();

/// 将原始 locale 字符串（如 `LANG` 环境变量值）标准化为 BCP 47 语言标识符。
///
/// 处理 Unix `LANG` 常见的 `zh_CN.UTF-8` 格式：截去编码后缀，下划线替换为连字符。
/// `C` / `POSIX` locale 不携带语言信息，返回 `None`。
fn normalize_lang(raw: &str) -> Option<LanguageIdentifier> {
    let raw = raw.trim();
    if raw.is_empty() || raw == "C" || raw == "POSIX" || raw.starts_with("C.") {
        return None;
    }
    let lang_part = raw.split('.').next().unwrap_or(raw);
    let normalized = lang_part.replace('_', "-");
    normalized.parse().ok()
}

/// 在 macOS 上通过系统命令检测用户 locale（`defaults read -g AppleLocale`）。
/// 仅在环境变量未提供有效 locale 时调用，结果被 `OnceLock` 缓存。
#[cfg(target_os = "macos")]
fn detect_platform_locale() -> Option<LanguageIdentifier> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    normalize_lang(&String::from_utf8_lossy(&out.stdout))
}

/// 非 macOS 平台暂无原生检测，留作扩展点。
#[cfg(not(target_os = "macos"))]
fn detect_platform_locale() -> Option<LanguageIdentifier> {
    None
}

/// 按优先级检测当前设备 locale。
fn detect_locale() -> Option<LanguageIdentifier> {
    // 1. 显式覆盖
    if let Some(id) = std::env::var("TOGI_LANG")
        .ok()
        .and_then(|s| normalize_lang(&s))
    {
        return Some(id);
    }
    // 2. Unix locale 环境变量（LC_ALL 优先级最高）
    for var in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(id) = std::env::var(var).ok().and_then(|s| normalize_lang(&s)) {
            return Some(id);
        }
    }
    // 3. 平台原生检测
    detect_platform_locale()
}

fn lang() -> &'static LanguageIdentifier {
    LANG.get_or_init(|| {
        detect_locale().unwrap_or_else(|| "en".parse::<LanguageIdentifier>().expect("valid langid"))
    })
}

// ── FTL 源文本选择 ──────────────────────────────────────────────────

/// 将检测到的 langid 解析为实际使用的 locale tag。
///
/// 匹配策略：精确 tag → 同语言回退 → 英文。
fn resolve_tag(detected: &LanguageIdentifier) -> &'static str {
    let detected_str = detected.to_string();

    // 1. 精确匹配
    for &tag in AVAILABLE_TAGS {
        if tag == detected_str {
            return tag;
        }
    }

    // 2. 同语言回退（如 zh-TW → zh-CN）
    let primary = detected.language.as_str();
    for &tag in AVAILABLE_TAGS {
        if let Ok(id) = tag.parse::<LanguageIdentifier>()
            && id.language.as_str() == primary
        {
            return tag;
        }
    }

    // 3. 回退至英文
    "en"
}

/// 根据检测到的 locale 选择 FTL 源文本。
fn ftl_source() -> &'static str {
    let tag = resolve_tag(lang());
    ftl_for_tag(tag).expect("resolved tag must have embedded FTL")
}

fn make_bundle() -> FluentBundle<FluentResource> {
    let langid = resolve_tag(lang())
        .parse::<LanguageIdentifier>()
        .expect("resolved tag is a valid langid");
    let mut bundle = FluentBundle::new(vec![langid]);
    let res =
        FluentResource::try_new(ftl_source().to_string()).expect("Failed to parse FTL resource");
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

/// 获取带参数翻译。缺失 key 或 pattern 时返回 `⚠key` 格式的占位字符串，
/// 不会触发 panic。
pub fn tr_args(key: &str, args: &FluentArgs<'_>) -> String {
    let bundle = make_bundle();
    let Some(msg) = bundle.get_message(key) else {
        return format!("⚠{key}");
    };
    let Some(pattern) = msg.value() else {
        return format!("⚠{key}");
    };
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

    // ── normalize_lang ──────────────────────────────────────────────

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
    fn normalize_trims_whitespace() {
        let id = normalize_lang("  en_US.UTF-8  ").unwrap();
        assert_eq!(id.language.as_str(), "en");
    }

    #[test]
    fn normalize_c_locale_returns_none() {
        assert!(normalize_lang("C").is_none());
        assert!(normalize_lang("POSIX").is_none());
        assert!(normalize_lang("C.UTF-8").is_none());
    }

    #[test]
    fn invalid_lang_returns_none() {
        assert!(normalize_lang("not-a-real-language!").is_none());
        assert!(normalize_lang("").is_none());
    }

    // ── resolve_tag ─────────────────────────────────────────────────

    #[test]
    fn resolve_exact_zh_cn() {
        let id: LanguageIdentifier = "zh-CN".parse().unwrap();
        assert_eq!(resolve_tag(&id), "zh-CN");
    }

    #[test]
    fn resolve_exact_en() {
        let id: LanguageIdentifier = "en".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");
    }

    #[test]
    fn resolve_same_language_fallback_zh_tw() {
        // zh-TW 没有独立翻译，回退到同语言的 zh-CN
        let id: LanguageIdentifier = "zh-TW".parse().unwrap();
        assert_eq!(resolve_tag(&id), "zh-CN");
    }

    #[test]
    fn resolve_same_language_fallback_zh_hans() {
        // zh-Hans（简体中文，无 region）回退到 zh-CN
        let id: LanguageIdentifier = "zh-Hans".parse().unwrap();
        assert_eq!(resolve_tag(&id), "zh-CN");
    }

    #[test]
    fn resolve_language_only_zh() {
        let id: LanguageIdentifier = "zh".parse().unwrap();
        assert_eq!(resolve_tag(&id), "zh-CN");
    }

    #[test]
    fn resolve_language_only_en() {
        let id: LanguageIdentifier = "en".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");
    }

    #[test]
    fn resolve_en_gb_falls_back_to_en() {
        let id: LanguageIdentifier = "en-GB".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");
    }

    #[test]
    fn resolve_unsupported_language_falls_back_to_en() {
        let id: LanguageIdentifier = "fr-FR".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");

        let id: LanguageIdentifier = "ja-JP".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");

        let id: LanguageIdentifier = "de".parse().unwrap();
        assert_eq!(resolve_tag(&id), "en");
    }

    // ── ftl_for_tag ─────────────────────────────────────────────────

    #[test]
    fn ftl_for_known_tags() {
        assert!(ftl_for_tag("zh-CN").is_some());
        assert!(ftl_for_tag("en").is_some());
    }

    #[test]
    fn ftl_for_unknown_tag() {
        assert!(ftl_for_tag("zh-TW").is_none());
        assert!(ftl_for_tag("fr").is_none());
        assert!(ftl_for_tag("").is_none());
    }

    #[test]
    fn ftl_source_never_empty() {
        // 无论检测到什么语言，ftl_source 必须返回非空源文本
        assert!(!ftl_source().is_empty());
    }
}
