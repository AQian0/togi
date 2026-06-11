use crate::constants;
use crate::error::{ErrorKind, TogiError};
use rig::completion::ToolDefinition;
use rig::tool::Tool;
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

impl Shell {
    fn resolve_cwd(cwd: Option<&Path>) -> Result<PathBuf, ShellError> {
        cwd.map(Path::to_path_buf).ok_or(ShellError::MissingCwd)
    }
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
    #[error("failed to start the command: {source}")]
    Spawn {
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
}

impl Tool for Shell {
    const NAME: &'static str = "shell";
    type Error = ShellError;
    type Args = ShellArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        let parameters = schemars::schema_for!(ShellArgs);
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Execute a shell command in the injected `cwd`. The command \
                          runs through the system shell (`sh -c` on Unix, `cmd /C` on Windows), so \
                          pipes, redirects and globbing work. The result reports the exit code \
                          along with captured stdout and stderr. Use the optional `timeout_secs` \
                          (default 60, max 600) to bound long-running commands. On failure the \
                          tool returns a descriptive error explaining how to fix the call."
                .to_string(),
            parameters: serde_json::to_value(parameters).unwrap(),
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
