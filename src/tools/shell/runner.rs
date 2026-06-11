use super::ShellError;
use super::capture::{self, StreamCapture, StreamChunk};
use super::{process, render};
use crate::constants;
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
        .map_err(|source| ShellError::Spawn { source })?;
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
        Ok(Err(source)) => return Err(ShellError::Io { source }),
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

pub(super) async fn run_interleaved(
    command: &str,
    cwd: &Path,
    duration: Duration,
    env: Option<&HashMap<String, String>>,
) -> Result<String, ShellError> {
    let mut child = process::shell_command(command, cwd, env)
        .spawn()
        .map_err(|source| ShellError::Spawn { source })?;
    let child_id = child.id();

    let stdout_handle = child.stdout.take().ok_or_else(|| ShellError::Io {
        source: std::io::Error::other("failed to capture stdout"),
    })?;
    let stderr_handle = child.stderr.take().ok_or_else(|| ShellError::Io {
        source: std::io::Error::other("failed to capture stderr"),
    })?;

    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<StreamChunk>(constants::INTERLEAVED_CHANNEL_CAPACITY);

    let stdout_task = tokio::spawn(capture::forward_chunks(stdout_handle, true, tx.clone()));
    let stderr_task = tokio::spawn(capture::forward_chunks(stderr_handle, false, tx));

    let mut chunks: Vec<StreamChunk> = Vec::new();
    let mut total_bytes = 0usize;
    let mut stored_bytes = 0usize;
    let read_result = tokio::time::timeout(duration, async {
        while let Some((is_stdout, data)) = rx.recv().await {
            total_bytes = total_bytes.saturating_add(data.len());
            let remaining = constants::SHELL_MAX_OUTPUT_BYTES.saturating_sub(stored_bytes);
            if remaining > 0 {
                let keep = data.len().min(remaining);
                chunks.push((is_stdout, data[..keep].to_vec()));
                stored_bytes += keep;
            }
        }
    })
    .await;

    match read_result {
        Ok(()) => {
            stdout_task.await.ok();
            stderr_task.await.ok();
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
    }

    let status = child
        .wait()
        .await
        .map_err(|source| ShellError::Io { source })?;
    Ok(render::render_interleaved(
        status,
        &chunks,
        total_bytes,
        stored_bytes,
        cwd,
    ))
}
