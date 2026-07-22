use super::edit::unified_diff_blocking;
use super::{Modify, ModifyError};
use crate::shared::constants;
use crate::shared::text_encoding::{decode_text, encode_text};
use crate::shared::util::{append_diff, format_size};
use std::path::Path;

#[must_use]
fn dry_run_output(action: String, display: &str, diff: Option<String>) -> String {
    let head = crate::t!(
        "modify-dry-run",
        action = action,
        display = display.to_string()
    );
    match diff {
        Some(diff) => format!("{head}\n{diff}"),
        None => format!("{head} {}", crate::t!("common-no-changes")),
    }
}

fn action_word(existed: bool, past: bool) -> String {
    let key = match (existed, past) {
        (true, true) => "modify-action-overwrote",
        (false, true) => "modify-action-created",
        (true, false) => "modify-action-overwrite",
        (false, false) => "modify-action-create",
    };
    crate::t!(key)
}

pub(super) async fn write_text_file(
    path: &Path,
    display: &str,
    content: &str,
    dry_run: bool,
    requested_encoding: Option<&'static encoding_rs::Encoding>,
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
                Some(crate::t!(
                    "modify-diff-skipped-large",
                    size = format_size(metadata.len())
                )),
            )
        } else {
            match tokio::fs::read(path).await {
                Ok(bytes) => match decode_text(&bytes, requested_encoding) {
                    Ok(decoded) => (Some(decoded.text), mtime, perms, None),
                    Err(source) => (
                        None,
                        mtime,
                        perms,
                        Some(crate::t!(
                            "modify-diff-skipped-decode",
                            error = source.to_string()
                        )),
                    ),
                },
                Err(source) => (
                    None,
                    mtime,
                    perms,
                    Some(crate::t!(
                        "modify-diff-skipped-read",
                        error = source.to_string()
                    )),
                ),
            }
        }
    } else {
        (None, None, None, None)
    };

    if dry_run {
        if let Some(note) = diff_skip_note {
            return Ok(format!(
                "{}\n({note})",
                crate::t!(
                    "modify-dry-run-sized",
                    action = action_word(existed, false),
                    display = display.to_string(),
                    size = format_size(content.len() as u64)
                )
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
        return Ok(dry_run_output(action_word(existed, false), display, diff));
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

    let encoding = requested_encoding.unwrap_or(encoding_rs::UTF_8);
    let data = encode_text(content, encoding, &[])?;
    let warning = super::atomic::write_bytes(
        path,
        display,
        &data,
        preserve_perms,
        mtime_before,
    )
    .await?;

    let bytes = content.len();
    let lines = if content.is_empty() {
        0
    } else {
        content.lines().count()
    };
    let summary = crate::t!(
        "modify-summary",
        action = action_word(existed, true),
        display = display.to_string(),
        bytes = bytes,
        lines = lines
    );

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
        out.push_str(&crate::t!("modify-diff-skipped-warning", note = note));
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
        return Ok(crate::t!(
            "modify-dry-run-sized",
            action = action_word(existed, false),
            display = display.to_string(),
            size = format_size(data.len() as u64)
        ));
    }

    let warning = super::atomic::write_bytes(
        path,
        display,
        data,
        preserve_perms,
        mtime_before,
    )
    .await?;

    let summary = crate::t!(
        "modify-summary-sized",
        action = action_word(existed, true),
        display = display.to_string(),
        size = format_size(data.len() as u64)
    );

    let mut out = format!("{summary}\n{}", crate::t!("modify-binary-no-diff"));
    if let Some(w) = warning {
        out.push('\n');
        out.push_str(&w);
    }
    Ok(out)
}
