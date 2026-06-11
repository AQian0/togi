use super::edit::unified_diff_blocking;
use super::{Modify, ModifyError};
use crate::common::{append_diff, format_size};
use crate::constants;
use std::path::Path;

#[must_use]
fn dry_run_output(action: &str, display: &str, diff: Option<String>) -> String {
    match diff {
        Some(diff) => format!("[dry run] would {action} `{display}`\n{diff}"),
        None => format!("[dry run] would {action} `{display}` (no changes)"),
    }
}

pub(super) async fn write_text_file(
    path: &Path,
    display: &str,
    content: &str,
    dry_run: bool,
) -> Result<String, ModifyError> {
    let existed = tokio::fs::try_exists(path)
        .await
        .map_err(|source| Modify::map_io(source, display))?;

    let (old_text, mtime_before, preserve_perms, diff_skip_note) = if existed {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|source| Modify::map_io(source, display))?;
        if metadata.is_dir() {
            return Err(ModifyError::NotAFile {
                path: display.to_string(),
            });
        }

        let mtime = metadata.modified().ok();
        let perms = Some(metadata.permissions());
        if metadata.len() > constants::LARGE_FILE_THRESHOLD {
            (
                None,
                mtime,
                perms,
                Some(format!(
                    "diff skipped — existing file is too large ({})",
                    format_size(metadata.len())
                )),
            )
        } else {
            match tokio::fs::read_to_string(path).await {
                Ok(text) => (Some(text), mtime, perms, None),
                Err(source) if source.kind() == std::io::ErrorKind::InvalidData => (
                    None,
                    mtime,
                    perms,
                    Some("diff skipped — existing file is not valid UTF-8".to_string()),
                ),
                Err(source) => (
                    None,
                    mtime,
                    perms,
                    Some(format!(
                        "diff skipped — could not read existing file: {source}"
                    )),
                ),
            }
        }
    } else {
        (None, None, None, None)
    };

    if dry_run {
        let action = if existed { "overwrite" } else { "create" };
        if let Some(note) = diff_skip_note {
            return Ok(format!(
                "[dry run] would {action} `{display}` ({}).\n({note})",
                format_size(content.len() as u64),
            ));
        }
        let diff = match old_text {
            Some(old) => {
                unified_diff_blocking(
                    old,
                    content.to_string(),
                    display.to_string(),
                    display.to_string(),
                    constants::DIFF_CONTEXT,
                )
                .await?
            }
            None => {
                unified_diff_blocking(
                    String::new(),
                    content.to_string(),
                    "/dev/null".to_string(),
                    display.to_string(),
                    constants::DIFF_CONTEXT,
                )
                .await?
            }
        };
        return Ok(dry_run_output(action, display, diff));
    }

    let mtime_ok = if let Some(expected) = mtime_before {
        tokio::fs::metadata(path)
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            == Some(expected)
    } else {
        true
    };

    let warning = super::atomic::write_text::<ModifyError>(
        path,
        display,
        content,
        preserve_perms,
        mtime_before,
    )
    .await?;

    let action = if existed { "overwrote" } else { "created" };
    let bytes = content.len();
    let lines = if content.is_empty() {
        0
    } else {
        content.lines().count()
    };
    let summary = format!("{action} `{display}` ({bytes} bytes, {lines} lines).");

    let diff = if mtime_ok {
        match old_text {
            Some(old) => {
                unified_diff_blocking(
                    old,
                    content.to_string(),
                    display.to_string(),
                    display.to_string(),
                    constants::DIFF_CONTEXT,
                )
                .await?
            }
            None if !existed => {
                unified_diff_blocking(
                    String::new(),
                    content.to_string(),
                    "/dev/null".to_string(),
                    display.to_string(),
                    constants::DIFF_CONTEXT,
                )
                .await?
            }
            None => None,
        }
    } else {
        None
    };

    let mut out = if diff_skip_note.is_none() {
        append_diff(summary, diff, existed)
    } else if let Some(diff) = diff {
        format!("{summary}\n{diff}")
    } else {
        summary
    };

    if let Some(note) = diff_skip_note {
        out.push_str(&format!(
            "\n(warning: {note}; use `shell` with `diff` to compare)"
        ));
    }
    if let Some(w) = warning {
        out.push('\n');
        out.push_str(&w);
    }
    Ok(out)
}

pub(super) async fn write_binary_file(
    path: &Path,
    display: &str,
    data: &[u8],
    dry_run: bool,
) -> Result<String, ModifyError> {
    let existed = tokio::fs::try_exists(path)
        .await
        .map_err(|source| Modify::map_io(source, display))?;

    let (mtime_before, preserve_perms) = if existed {
        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|source| Modify::map_io(source, display))?;
        if metadata.is_dir() {
            return Err(ModifyError::NotAFile {
                path: display.to_string(),
            });
        }
        (metadata.modified().ok(), Some(metadata.permissions()))
    } else {
        (None, None)
    };

    if dry_run {
        let action = if existed { "overwrite" } else { "create" };
        return Ok(format!(
            "[dry run] would {action} `{display}` ({}).",
            format_size(data.len() as u64)
        ));
    }

    let warning = super::atomic::write_bytes::<ModifyError>(
        path,
        display,
        data,
        preserve_perms,
        mtime_before,
    )
    .await?;

    let action = if existed { "overwrote" } else { "created" };
    let summary = format!("{action} `{display}` ({}).", format_size(data.len() as u64));

    let mut out = format!("{summary}\n(binary — no diff available)");
    if let Some(w) = warning {
        out.push('\n');
        out.push_str(&w);
    }
    Ok(out)
}
