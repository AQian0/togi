use crate::shared::constants;
use crate::shared::error::{ErrorKind, TogiError};
use rig::tool::{Tool, ToolFailure};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod capture;
mod process;
mod render;
mod runner;

#[derive(Clone, Copy, Default)]
pub struct Shell;

impl crate::tools::ClassifyEffect for Shell {
    fn name() -> &'static str {
        Self::NAME
    }
    fn classify(args: &serde_json::Value) -> crate::tools::ToolEffect {
        let is_query = args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(is_query_command);
        if is_query {
            crate::tools::ToolEffect::ReadOnly
        } else {
            crate::tools::ToolEffect::Mutating
        }
    }
}

impl Shell {
    fn resolve_cwd(cwd: Option<&Path>) -> Result<PathBuf, ShellError> {
        cwd.map(Path::to_path_buf).ok_or(ShellError::MissingCwd)
    }
}

/// 判断一条 shell 命令是否为"纯查询"（只读）命令。
///
/// 仅用于决定结果在对话区的展示方式（隐藏冗长输出），不影响命令执行；
/// 因此采用保守的白名单：命令序列 / 管道中的每一段，其首词都必须是已知
/// 只读命令，且不含输出重定向。无法确定时一律按有副作用处理（照常展示）。
#[must_use]
fn is_query_command(command: &str) -> bool {
    let command = command.trim();
    // 任何输出重定向都可能写文件，视为有副作用。
    if command.is_empty() || command.contains('>') {
        return false;
    }
    let segments: Vec<&str> = command
        .split(['|', ';', '&'])
        .map(str::trim)
        .filter(|segment| !segment.is_empty())
        .collect();
    !segments.is_empty()
        && segments.iter().all(|segment| {
            segment
                .split_whitespace()
                .next()
                .is_some_and(is_read_only_command)
        })
}

/// 常见的"纯查询 / 只读"命令白名单。只纳入明确无副作用的命令
/// （故意排除 `sed`/`awk`/`tee` 等可写入的命令）。
#[must_use]
#[inline]
fn is_read_only_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "ls" | "cat"
            | "head"
            | "tail"
            | "wc"
            | "stat"
            | "file"
            | "tree"
            | "find"
            | "grep"
            | "rg"
            | "egrep"
            | "fgrep"
            | "pwd"
            | "which"
            | "whoami"
            | "date"
            | "du"
            | "df"
            | "echo"
            | "dirname"
            | "basename"
            | "realpath"
            | "readlink"
            | "sort"
            | "uniq"
            | "cut"
            | "nl"
            | "diff"
    )
}

#[derive(Deserialize, JsonSchema)]
pub struct ShellArgs {
    /// The shell command to execute. It is run through the system shell
    /// (`sh -c` on Unix, `cmd /C` on Windows), so pipes, redirects, globbing
    /// and other shell syntax work as expected.
    command: String,
    /// Current working directory (`cwd`). This is injected as hidden call context by
    /// `inject::inject`. The command runs with this directory as its working
    /// directory.
    #[serde(default)]
    #[schemars(skip)]
    cwd: Option<PathBuf>,
    /// Maximum number of seconds to let the command run before it is killed.
    /// Defaults to 60 and is capped at 600.
    #[serde(default)]
    timeout_secs: Option<u64>,
    /// Environment variables set for this command. Injected as hidden context,
    /// overlaid on the inherited environment.
    #[serde(default)]
    #[schemars(skip)]
    env: Option<HashMap<String, String>>,
    /// When true, stdout and stderr are interleaved in arrival order with
    /// source labels, like a real terminal. Default false (separated sections).
    #[serde(default)]
    interleave: Option<bool>,
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("`command` must not be empty.")]
    EmptyCommand,
    #[error(
        "`cwd` was not injected. Register `shell` through `inject::inject` with a \
         hidden `cwd` value."
    )]
    MissingCwd,
    #[error(
        "the working directory `{path}` does not exist or is not a directory. Double-check the \
         path, then retry."
    )]
    BadWorkingDir { path: String },
    #[error("failed to start the command in `{cwd}`: {source}")]
    Spawn {
        cwd: String,
        #[source]
        source: std::io::Error,
    },
    #[error("io error while running the command: {source}")]
    Io {
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the command did not finish within {secs} seconds and was killed. Increase `timeout_secs` \
         or run a faster command."
    )]
    Timeout { secs: u64 },
}

impl TogiError for ShellError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyCommand => "shell.empty_command",
            Self::MissingCwd => "tool.missing_cwd",
            Self::BadWorkingDir { .. } => "shell.bad_working_dir",
            Self::Spawn { .. } => "shell.spawn",
            Self::Io { .. } => "shell.io",
            Self::Timeout { .. } => "shell.timeout",
        }
    }

    fn kind(&self) -> ErrorKind {
        match self {
            Self::EmptyCommand => ErrorKind::InvalidArgument,
            Self::MissingCwd => ErrorKind::MissingRuntimeInjection,
            Self::BadWorkingDir { .. } => ErrorKind::NotFound,
            Self::Spawn { .. } => ErrorKind::External,
            Self::Io { .. } => ErrorKind::Io,
            Self::Timeout { .. } => ErrorKind::Timeout,
        }
    }

    fn user_message(&self) -> String {
        match self {
            Self::BadWorkingDir { path } => {
                crate::t!("error-bad-working-dir", path = path.clone())
            }
            Self::Timeout { secs } => {
                crate::t!("error-shell-timeout", secs = *secs)
            }
            Self::Spawn { cwd, source } => {
                crate::t!(
                    "error-shell-spawn",
                    cwd = cwd.clone(),
                    error = source.to_string()
                )
            }
            Self::Io { source } => {
                crate::t!("error-shell-io", error = source.to_string())
            }
            _ => self.to_string(),
        }
    }
}

impl Tool for Shell {
    const NAME: &'static str = "shell";
    type Error = ShellError;
    type Args = ShellArgs;
    type Output = String;

    fn description(&self) -> String {
        "Execute a shell command in the injected `cwd`. The command \
         runs through the system shell (`sh -c` on Unix, `cmd /C` on Windows), so \
         pipes, redirects and globbing work. The result reports the exit code \
         along with captured stdout and stderr. Use the optional `timeout_secs` \
         (default 60, max 600) to bound long-running commands. On failure the \
         tool returns a descriptive error explaining how to fix the call."
            .to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(ShellArgs)).unwrap()
    }

    fn classify_error(&self, error: &Self::Error) -> ToolFailure {
        match error {
            ShellError::Timeout { secs: _ } => {
                ToolFailure::timeout(error.to_string()).with_code("shell.timeout")
            }
            ShellError::EmptyCommand
            | ShellError::MissingCwd
            | ShellError::BadWorkingDir { .. } => {
                ToolFailure::invalid_args(error.to_string())
            }
            ShellError::Spawn { .. } | ShellError::Io { .. } => {
                ToolFailure::other(error.to_string())
            }
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let command = args.command.trim();
        if command.is_empty() {
            return Err(ShellError::EmptyCommand);
        }
        let cwd = Self::resolve_cwd(args.cwd.as_deref())?;
        if !cwd.is_dir() {
            return Err(ShellError::BadWorkingDir {
                path: cwd.display().to_string(),
            });
        }
        let secs = args
            .timeout_secs
            .unwrap_or(constants::DEFAULT_TIMEOUT_SECS)
            .clamp(1, constants::MAX_TIMEOUT_SECS);
        let duration = Duration::from_secs(secs);
        let env = args.env.as_ref();

        if args.interleave.unwrap_or(false) {
            return runner::run_interleaved(command, &cwd, duration, env).await;
        }

        runner::run_separated(command, &cwd, duration, env).await
    }
}

#[cfg(test)]
mod query_tests {
    use super::is_query_command;

    #[test]
    fn plain_read_only_commands_are_queries() {
        assert!(is_query_command("ls -la"));
        assert!(is_query_command("cat src/main.rs"));
        assert!(is_query_command("grep -rn TODO ."));
    }

    #[test]
    fn read_only_pipelines_and_sequences_are_queries() {
        assert!(is_query_command("cat foo | grep bar"));
        assert!(is_query_command("ls; pwd"));
    }

    #[test]
    fn mutating_commands_are_not_queries() {
        assert!(!is_query_command("rm -rf target"));
        assert!(!is_query_command("git commit -m x"));
        assert!(!is_query_command("cargo build"));
    }

    #[test]
    fn redirection_disqualifies_a_query() {
        assert!(!is_query_command("cat foo > bar"));
        assert!(!is_query_command("echo hi >> log"));
    }

    #[test]
    fn any_mutating_segment_disqualifies_the_whole() {
        assert!(!is_query_command("ls && rm foo"));
        assert!(!is_query_command("cat foo | tee out"));
    }

    #[test]
    fn empty_command_is_not_a_query() {
        assert!(!is_query_command(""));
        assert!(!is_query_command("   "));
    }
}
