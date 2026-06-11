use crate::constants;
use crate::error::{ErrorKind, TogiError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("无法读取配置文件 {path}：{source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("配置文件 {path} 解析失败：{source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
}

impl TogiError for ConfigError {
    fn code(&self) -> &'static str {
        match self {
            Self::Read { .. } => "config.read",
            Self::Parse { .. } => "config.parse",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::Read { .. } => ErrorKind::Io,
            Self::Parse { .. } => ErrorKind::InvalidArgument,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    #[serde(default)]
    pub system: SystemConfig,
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
                if log_enabled() {
                    eprintln!("[togi] 已加载配置文件：{}", candidate.display());
                }
                return Ok(config);
            }
        }
        Ok(Config::default())
    }
}

fn log_enabled() -> bool {
    std::env::var(constants::ENV_LOG_CONFIG).is_ok()
}

fn candidate_paths() -> impl Iterator<Item = std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        paths.push(cwd.join(constants::CONFIG_FILENAME));
    }
    if let Some(p) = default_config_path() {
        paths.push(p);
    }
    paths.into_iter()
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

    #[test]
    fn load_from_empty_paths_returns_default() {
        let config = Config::load_from(Vec::<PathBuf>::new()).unwrap();
        assert!(config.system.model.is_none());
        assert!(config.system.theme.is_none());
    }

    #[test]
    fn load_from_nonexistent_returns_default() {
        let config =
            Config::load_from(vec![PathBuf::from("/tmp/togi_nonexistent_config.toml")]).unwrap();
        assert!(config.system.model.is_none());
    }

    #[test]
    fn load_from_valid_toml() {
        let dir = std::env::temp_dir().join("togi_config_test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("togi.toml");
        std::fs::write(
            &path,
            "[system]\nmodel = \"gpt-4\"\ntheme = \"Mocha\"\nmax_multi_turn = 5\n",
        )
        .unwrap();

        let config = Config::load_from(vec![path.clone()]).unwrap();
        assert_eq!(config.system.model.as_deref(), Some("gpt-4"));
        assert_eq!(config.system.theme.as_deref(), Some("Mocha"));
        assert_eq!(config.system.max_multi_turn, Some(5));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_from_invalid_toml_returns_error() {
        let dir = std::env::temp_dir().join("togi_config_test_bad");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("togi.toml");
        std::fs::write(&path, "this is not valid toml [[[").unwrap();

        let result = Config::load_from(vec![path.clone()]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("解析失败"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_from_picks_first_existing() {
        let dir = std::env::temp_dir().join("togi_config_test_order");
        let _ = std::fs::create_dir_all(&dir);
        let path1 = dir.join("first.toml");
        let path2 = dir.join("second.toml");
        std::fs::write(&path1, "[system]\nmodel = \"first\"\n").unwrap();
        std::fs::write(&path2, "[system]\nmodel = \"second\"\n").unwrap();

        let config = Config::load_from(vec![path1.clone(), path2.clone()]).unwrap();
        assert_eq!(config.system.model.as_deref(), Some("first"));

        let _ = std::fs::remove_file(&path1);
        let _ = std::fs::remove_file(&path2);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_from_skips_nonexistent_and_uses_later() {
        let dir = std::env::temp_dir().join("togi_config_test_skip");
        let _ = std::fs::create_dir_all(&dir);
        let path2 = dir.join("real.toml");
        std::fs::write(&path2, "[system]\ntheme = \"Frappe\"\n").unwrap();

        let config = Config::load_from(vec![
            PathBuf::from("/tmp/togi_definitely_not_here.toml"),
            path2.clone(),
        ])
        .unwrap();
        assert_eq!(config.system.theme.as_deref(), Some("Frappe"));

        let _ = std::fs::remove_file(&path2);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn effective_preamble_uses_default_when_none() {
        let config = Config::default();
        assert_eq!(config.effective_preamble(), constants::DEFAULT_PREAMBLE);
    }

    #[test]
    fn effective_preamble_uses_config_value() {
        let mut config = Config::default();
        config.system.preamble = Some("custom preamble".into());
        assert_eq!(config.effective_preamble(), "custom preamble");
    }

    #[test]
    fn effective_max_multi_turn_uses_default_when_none() {
        let config = Config::default();
        assert_eq!(
            config.effective_max_multi_turn(),
            constants::MAX_MULTI_TURN_ITERATIONS
        );
    }
}
