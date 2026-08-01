use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("invalid context configuration: {0}")]
    InvalidContext(String),
}

impl TogiError for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            Self::Read { .. } => "config.read",
            Self::Parse { .. } => "config.parse",
            Self::InvalidContext(_) => "config.invalid_context",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::Read { .. } => ErrorKind::Io,
            Self::Parse { .. } | Self::InvalidContext(_) => ErrorKind::InvalidArgument,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::Read { path, source } => crate::t!(
                "config-read-error",
                path = path.display().to_string(),
                error = source.to_string()
            ),
            Self::Parse { path, source } => crate::t!(
                "config-parse-error",
                path = path.display().to_string(),
                error = source.to_string()
            ),
            Self::InvalidContext(message) => message.clone(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemConfig {
    pub model: Option<String>,
    pub theme: Option<String>,
    /// 系统提示词。留空则使用内置默认值。
    pub preamble: Option<String>,
    /// Agent 流式对话中允许的最大多轮工具调用循环次数。
    pub max_multi_turn: Option<u32>,
}

/// 变更类工具确认配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    /// 无需交互确认的工具名；等价于 UI 中本次会话选择“始终允许”。
    pub always_allow: Vec<String>,
}

/// 上下文窗口管理配置。`window_tokens` 未配置时关闭自动上下文管理。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub window_tokens: Option<u64>,
    pub reserve_tokens: Option<u64>,
    pub keep_recent_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub system: SystemConfig,
    pub context: ContextConfig,
    pub approval: ApprovalConfig,
}

impl Config {
    /// 获取系统提示词：优先使用配置值，否则使用内置默认值。
    pub fn effective_preamble(&self) -> &str {
        self.system
            .preamble
            .as_deref()
            .unwrap_or(constants::DEFAULT_PREAMBLE)
    }

    /// 获取多轮工具调用上限：优先使用配置值，否则使用内置默认值。
    pub fn effective_max_multi_turn(&self) -> u32 {
        self.system
            .max_multi_turn
            .unwrap_or(constants::MAX_MULTI_TURN_ITERATIONS)
    }

    /// 获取上下文窗口策略：未配置 `window_tokens` 时返回 `Ok(None)`（关闭）。
    /// 非法值返回配置错误，不在运行时静默修正。
    pub fn effective_context_policy(
        &self,
    ) -> Result<Option<crate::context::ContextPolicy>, ConfigError> {
        let Some(window) = self.context.window_tokens else {
            return Ok(None);
        };
        let reserve = self
            .context
            .reserve_tokens
            .unwrap_or(constants::DEFAULT_RESERVE_TOKENS);
        let keep = self
            .context
            .keep_recent_tokens
            .unwrap_or(constants::DEFAULT_KEEP_RECENT_TOKENS);
        if reserve >= window {
            return Err(ConfigError::InvalidContext(crate::t!(
                "config-context-reserve-too-big",
                reserve = reserve,
                window = window
            )));
        }
        if keep >= window - reserve {
            return Err(ConfigError::InvalidContext(crate::t!(
                "config-context-keep-too-big",
                keep = keep,
                available = window - reserve
            )));
        }
        Ok(Some(crate::context::ContextPolicy {
            window_tokens: window,
            reserve_tokens: reserve,
            keep_recent_tokens: keep,
        }))
    }

    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(candidate_paths())
    }

    pub fn load_from(paths: impl IntoIterator<Item = PathBuf>) -> Result<Self, ConfigError> {
        for candidate in paths {
            if candidate.exists() {
                let content =
                    std::fs::read_to_string(&candidate).map_err(|source| ConfigError::Read {
                        path: candidate.clone(),
                        source,
                    })?;
                let config: Config =
                    toml::from_str(&content).map_err(|source| ConfigError::Parse {
                        path: candidate.clone(),
                        source,
                    })?;
                tracing::debug!(path = %candidate.display(), "config loaded");
                return Ok(config);
            }
        }
        Ok(Config::default())
    }
}

fn candidate_paths() -> impl Iterator<Item = std::path::PathBuf> {
    let cwd = std::env::current_dir()
        .ok()
        .map(|cwd| cwd.join(constants::CONFIG_FILENAME));
    let default = default_config_path();
    cwd.into_iter().chain(default)
}

fn default_config_path() -> Option<std::path::PathBuf> {
    let base = dirs::config_dir()?;
    if cfg!(windows) {
        Some(
            base.join(constants::APP_DIR_NAME)
                .join(constants::WINDOWS_CONFIG_FILENAME),
        )
    } else {
        Some(base.join(constants::CONFIG_FILENAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_config(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("togi_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn load_from_missing_paths_returns_default() {
        let config =
            Config::load_from(vec![PathBuf::from("/tmp/togi_nonexistent_config.toml")]).unwrap();
        assert!(config.system.model.is_none());
        assert!(config.system.theme.is_none());
    }

    #[test]
    fn load_from_reads_first_existing_toml() {
        let first = tmp_config(
            "first.toml",
            "[system]\nmodel = \"gpt-4\"\ntheme = \"Mocha\"\nmax_multi_turn = 5\n\n\
             [approval]\nalways_allow = [\"modify\"]\n\n\
             [context]\nwindow_tokens = 128000\nreserve_tokens = 8192\n",
        );
        let second = tmp_config("second.toml", "[system]\nmodel = \"second\"\n");
        let config = Config::load_from(vec![
            PathBuf::from("/tmp/togi_not_here.toml"),
            first,
            second,
        ])
        .unwrap();
        assert_eq!(config.system.model.as_deref(), Some("gpt-4"));
        assert_eq!(config.system.theme.as_deref(), Some("Mocha"));
        assert_eq!(config.system.max_multi_turn, Some(5));
        assert_eq!(config.approval.always_allow, vec!["modify"]);
        let policy = config.effective_context_policy().unwrap().unwrap();
        assert_eq!(policy.window_tokens, 128_000);
        assert_eq!(policy.reserve_tokens, 8_192);
        assert_eq!(
            policy.keep_recent_tokens,
            constants::DEFAULT_KEEP_RECENT_TOKENS
        );
    }

    #[test]
    fn load_from_invalid_toml_returns_error() {
        let path = tmp_config("bad.toml", "this is not valid toml [[[");
        let err = Config::load_from(vec![path]).unwrap_err();
        assert_eq!(err.code(), "config.parse");
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    }

    #[test]
    fn context_policy_validation() {
        let mut config = Config::default();
        // window 未配置：关闭
        assert_eq!(config.effective_context_policy().unwrap(), None);
        // reserve >= window：非法
        config.context.window_tokens = Some(10_000);
        config.context.reserve_tokens = Some(10_000);
        let err = config.effective_context_policy().unwrap_err();
        assert_eq!(err.code(), "config.invalid_context");
        assert_eq!(err.kind(), ErrorKind::InvalidArgument);
        // keep >= window - reserve：非法；少 1 则合法
        config.context.reserve_tokens = Some(8_192);
        config.context.keep_recent_tokens = Some(10_000 - 8_192);
        assert!(config.effective_context_policy().is_err());
        config.context.keep_recent_tokens = Some(10_000 - 8_192 - 1);
        assert!(config.effective_context_policy().is_ok());
    }
}
