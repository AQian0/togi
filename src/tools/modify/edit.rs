use super::{Modify, ModifyError};
use crate::shared::constants;
use crate::shared::text_encoding::{decode_text, encode_text};
use crate::shared::util::append_diff;
use similar::{DiffOp, TextDiff};
use std::path::Path;
use tokio::io::AsyncReadExt;

const FUZZY_MAX_DISTANCE_RATIO: f64 = 0.5;

/// `old_text` 未命中时的相似行建议。
///
/// `render_en` 拼进 `Display`（面向模型的英文描述）；`render_localized`
/// 拼进 `user_message`（面向用户的本地化消息）。
#[derive(Debug, Clone)]
pub enum Suggestion {
    Line {
        line: usize,
        text: String,
        dist: usize,
    },
    Lines {
        start: usize,
        end: usize,
        dist: usize,
    },
}

impl Suggestion {
    pub(crate) fn render_en(&self) -> String {
        match self {
            Self::Line { line, text, dist } => format!(
                "did you mean line {line}: `{text}` ({dist} char{})?",
                if *dist == 1 { "" } else { "s" }
            ),
            Self::Lines { start, end, dist } => format!(
                "did you mean lines {start}-{end} ({dist} char{})?",
                if *dist == 1 { "" } else { "s" }
            ),
        }
    }

    pub(crate) fn render_localized(&self) -> String {
        match self {
            Self::Line { line, text, dist } => crate::t!(
                "modify-suggestion-line",
                line = *line,
                text = text.clone(),
                chars = *dist
            ),
            Self::Lines { start, end, dist } => crate::t!(
                "modify-suggestion-lines",
                start = *start,
                end = *end,
                chars = *dist
            ),
        }
    }
}

#[must_use]
pub(super) fn unified_diff(
    old: &str,
    new: &str,
    old_label: &str,
    new_label: &str,
    context: usize,
) -> Option<String> {
    let diff = TextDiff::from_lines(old, new);
    if diff
        .ops()
        .iter()
        .all(|op| matches!(op, DiffOp::Equal { .. }))
    {
        return None;
    }
    Some(
        diff.unified_diff()
            .context_radius(context)
            .header(old_label, new_label)
            .to_string(),
    )
}

async fn spawn_modify_blocking<T, F>(display: &str, f: F) -> Result<T, ModifyError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, ModifyError> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|source| ModifyError::Io {
            path: display.to_string(),
            source: std::io::Error::other(format!("blocking modify task failed: {source}")),
        })?
}

pub(super) async fn unified_diff_blocking(
    old: String,
    new: String,
    old_label: String,
    new_label: String,
    context: usize,
) -> Result<Option<String>, ModifyError> {
    let display = new_label.clone();
    spawn_modify_blocking(&display, move || {
        Ok(unified_diff(&old, &new, &old_label, &new_label, context))
    })
    .await
}

async fn apply_edits_blocking(
    content: String,
    edits: &[(String, String)],
    display: &str,
) -> Result<(String, String), ModifyError> {
    let edits_owned = edits.to_vec();
    let display_owned = display.to_string();
    spawn_modify_blocking(display, move || {
        let updated = apply_edits(&content, &edits_owned, &display_owned)?;
        Ok((content, updated))
    })
    .await
}

fn find_similar(content: &str, needle: &str) -> Option<Suggestion> {
    let needle = needle.trim();
    if needle.is_empty() {
        return None;
    }

    let max_dist = ((needle.len() as f64) * FUZZY_MAX_DISTANCE_RATIO).ceil() as usize;
    let needle_lines: Vec<&str> = needle.lines().collect();
    let content_lines: Vec<&str> = content.lines().collect();

    if needle_lines.len() == 1 {
        let best = content_lines
            .iter()
            .enumerate()
            .map(|(i, line)| (i, strsim::levenshtein(needle, line)))
            .filter(|(_, d)| *d <= max_dist)
            .min_by_key(|(_, d)| *d);
        return best.map(|(i, dist)| Suggestion::Line {
            line: i + 1,
            text: content_lines[i].to_string(),
            dist,
        });
    }

    let w = needle_lines.len();
    if w > content_lines.len() {
        return None;
    }
    let joined_needle = needle_lines.join("\n");
    let best = (0..=content_lines.len() - w)
        .map(|start| {
            let window = content_lines[start..start + w].join("\n");
            (start, strsim::levenshtein(&joined_needle, &window))
        })
        .filter(|(_, d)| *d <= max_dist)
        .min_by_key(|(_, d)| *d);
    best.map(|(start, dist)| Suggestion::Lines {
        start: start + 1,
        end: start + w,
        dist,
    })
}

fn collect_match_lines(content: &str, needle: &str) -> Vec<usize> {
    let mut positions = Vec::new();
    let mut search_from = 0;
    while let Some(offset) = content[search_from..].find(needle) {
        let abs_pos = search_from + offset;
        let line_no = content[..abs_pos].lines().count() + 1;
        positions.push(line_no);
        search_from = abs_pos + needle.len();
    }
    positions
}

pub(super) fn apply_edits(
    content: &str,
    edits: &[(String, String)],
    display: &str,
) -> Result<String, ModifyError> {
    let mut spans: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for (old, new) in edits {
        if old.is_empty() {
            return Err(ModifyError::EmptyOldText);
        }
        let start = content.find(old.as_str()).ok_or_else(|| {
            let suggestion = find_similar(content, old);
            let mut msg = format!(
                "`old_text` was not found in `{display}`. Make sure it matches \
                 the file content exactly, including whitespace."
            );
            if let Some(s) = &suggestion {
                msg.push(' ');
                msg.push_str(&s.render_en());
            }
            ModifyError::OldTextNotFound {
                path: display.to_string(),
                message: msg,
                suggestion,
            }
        })?;
        if content[start + old.len()..].contains(old.as_str()) {
            let positions = collect_match_lines(content, old);
            let total = positions.len();
            let pos_str = positions
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(ModifyError::OldTextNotUnique {
                path: display.to_string(),
                message: format!(
                    "`old_text` matched {total} times in `{display}` at lines \
                     [{pos_str}]; it must match exactly once. Add more surrounding \
                     context to make it unique."
                ),
                lines: pos_str,
            });
        }
        spans.push((start, start + old.len(), new.as_str()));
    }
    spans.sort_by_key(|(start, _, _)| *start);
    for pair in spans.windows(2) {
        let [prev, next] = pair else { unreachable!() };
        if prev.1 > next.0 {
            return Err(ModifyError::OverlappingEdits {
                path: display.to_string(),
            });
        }
    }
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0;
    for (start, end, new) in spans {
        out.push_str(&content[cursor..start]);
        out.push_str(new);
        cursor = end;
    }
    out.push_str(&content[cursor..]);
    Ok(out)
}

pub(super) async fn edit_file(
    path: &Path,
    display: &str,
    edits: &[(String, String)],
    dry_run: bool,
    requested_encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<String, ModifyError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|source| Modify::map_io(source, display))?;
    if metadata.is_dir() {
        return Err(ModifyError::NotAFile {
            path: display.to_string(),
        });
    }

    crate::shared::util::check_file_size(display, metadata.len())?;

    let perms = metadata.permissions();
    let mtime_before = metadata.modified().ok();

    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|source| Modify::map_io(source, display))?;
    let mut bytes = Vec::with_capacity(metadata.len().min(usize::MAX as u64) as usize);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|source| Modify::map_io(source, display))?;
    let decoded = decode_text(&bytes, requested_encoding)?;

    let (content, updated) = apply_edits_blocking(decoded.text, edits, display).await?;

    if dry_run {
        let summary = crate::t!(
            "modify-dry-run-edits",
            count = edits.len(),
            display = display.to_string()
        );
        let diff = unified_diff_blocking(
            content,
            updated,
            display.to_string(),
            display.to_string(),
            constants::DIFF_CONTEXT,
        )
        .await?;
        return Ok(append_diff(summary, diff, true));
    }

    let mtime_stable = tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        == mtime_before;

    let updated_bytes = encode_text(&updated, decoded.encoding, decoded.bom)?;
    let warning = super::atomic::write_bytes(
        path,
        display,
        &updated_bytes,
        Some(perms),
        mtime_before,
    )
    .await?;

    let (replacements, deletions): (Vec<_>, Vec<_>) =
        edits.iter().partition(|edit| !edit.1.is_empty());
    let mut parts: Vec<String> = Vec::new();
    if !replacements.is_empty() {
        parts.push(crate::t!("modify-replacements", count = replacements.len()));
    }
    if !deletions.is_empty() {
        parts.push(crate::t!("modify-deletions", count = deletions.len()));
    }
    let summary = if parts.is_empty() {
        crate::t!(
            "modify-edited-no-changes",
            display = display.to_string(),
            marker = crate::t!("common-no-changes")
        )
    } else {
        crate::t!(
            "modify-edited",
            display = display.to_string(),
            details = parts.join(", ")
        )
    };

    let diff = if mtime_stable {
        unified_diff_blocking(
            content,
            updated,
            display.to_string(),
            display.to_string(),
            constants::DIFF_CONTEXT,
        )
        .await?
    } else {
        None
    };
    let mut out = append_diff(summary, diff, true);
    if let Some(w) = warning {
        out.push('\n');
        out.push_str(&w);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edits_should_replace_unique_text() {
        let edits = [("beta".to_string(), "BETA".to_string())];

        let updated = apply_edits("alpha beta gamma", &edits, "test.txt").unwrap();

        assert_eq!(updated, "alpha BETA gamma");
    }

    #[test]
    fn apply_edits_should_apply_multiple_non_overlapping_replacements() {
        let edits = [
            ("alpha".to_string(), "A".to_string()),
            ("gamma".to_string(), "G".to_string()),
        ];

        let updated = apply_edits("alpha beta gamma", &edits, "test.txt").unwrap();

        assert_eq!(updated, "A beta G");
    }

    #[test]
    fn apply_edits_should_reject_overlapping_matches() {
        let edits = [
            ("abc".to_string(), "X".to_string()),
            ("cde".to_string(), "Y".to_string()),
        ];

        let result = apply_edits("abcdef", &edits, "test.txt");

        assert!(matches!(result, Err(ModifyError::OverlappingEdits { .. })));
    }

    #[test]
    fn apply_edits_should_reject_non_unique_old_text() {
        let edits = [("dup".to_string(), "".to_string())];

        let result = apply_edits("dup dup", &edits, "test.txt");

        assert!(matches!(result, Err(ModifyError::OldTextNotUnique { .. })));
    }

    #[test]
    fn apply_edits_should_suggest_similar_text_when_old_text_is_not_found() {
        let edits = [("fn mian() {".to_string(), "".to_string())];

        let result = apply_edits(
            "fn main() {\n    println!(\"hello\");\n}\n",
            &edits,
            "test.txt",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("did you mean"),
            "expected suggestion in: {err}"
        );
    }

    #[test]
    fn apply_edits_should_report_line_numbers_when_old_text_is_not_unique() {
        let edits = [("dup target".to_string(), "".to_string())];

        let result = apply_edits(
            "line one\ndup target\nline three\ndup target\nline five\n",
            &edits,
            "test.txt",
        );

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("lines [2, 4]") || err.contains("lines [2, 4"),
            "expected line info in: {err}"
        );
    }
}
