use super::ShellError;
use super::capture::{self, StreamCapture};
use super::{process, render};
use crate::shared::constants;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

pub(super) async fn run_separated(
    command: &str,
    cwd: &Path,
    duration: Duration,
    env: Option<&HashMap<String, String>>,
) -> Result<String, ShellError> {
    let mut child = process::shell_command(command, cwd, env)
        .spawn()
        .map_err(|source| ShellError::Spawn {
            cwd: cwd.display().to_string(),
            source,
        })?;
    let child_id = child.id();
    let stdout = child.stdout.take().ok_or_else(|| ShellError::Io {
        source: std::io::Error::other("failed to capture stdout"),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| ShellError::Io {
        source: std::io::Error::other("failed to capture stderr"),
    })?;

    let stream_cap = constants::SHELL_MAX_OUTPUT_BYTES / 2;
    let stdout_task = tokio::spawn(capture::read_limited(stdout, stream_cap));
    let stderr_task = tokio::spawn(capture::read_limited(stderr, stream_cap));

    let status = match tokio::time::timeout(duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(source)) => {
            stdout_task.abort();
            stderr_task.abort();
            return Err(ShellError::Io { source });
        }
        Err(_elapsed) => {
            process::kill_child(&mut child, child_id);
            stdout_task.abort();
            stderr_task.abort();
            let _ = child.wait().await;
            return Err(ShellError::Timeout {
                secs: duration.as_secs(),
            });
        }
    };

    let stdout = stdout_task.await.unwrap_or(StreamCapture {
        data: Vec::new(),
        total: 0,
    });
    let stderr = stderr_task.await.unwrap_or(StreamCapture {
        data: Vec::new(),
        total: 0,
    });
    Ok(render::render_separated(status, &stdout, &stderr, cwd))
}
