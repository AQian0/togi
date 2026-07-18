use super::{Modify, ModifyError};
use crate::shared::util::append_diff;
use crate::shared::constants;
use crate::shared::text_encoding::{decode_text, encode_text};
use itertools::Itertools;
use similar::{DiffOp, TextDiff};
use std::path::Path;
use tokio::io::AsyncReadExt;

const FUZZY_MAX_DISTANCE_RATIO: f64 = 0.5;

pub(super) trait EditFailure: Sized {
    fn empty_old_text() -> Self;
    fn old_text_not_found(path: String, message: String) -> Self;
    fn old_text_not_unique(path: String, message: String) -> Self;
    fn overlapping_edits(path: String) -> Self;
}

pub(super) struct Replacement<'a> {
    pub(super) old: &'a str,
    pub(super) new: &'a str,
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
    edits: &[Replacement<'_>],
    display: &str,
) -> Result<(String, String), ModifyError> {
    let edits_owned: Vec<(String, String)> = edits
        .iter()
        .map(|edit| (edit.old.to_string(), edit.new.to_string()))
        .collect();
    let display_owned = display.to_string();
    spawn_modify_blocking(display, move || {
        let replacements: Vec<Replacement<'_>> = edits_owned
            .iter()
            .map(|(old, new)| Replacement {
                old: old.as_str(),
                new: new.as_str(),
            })
            .collect();
        let updated = apply_edits::<ModifyError>(&content, &replacements, &display_owned)?;
        Ok((content, updated))
    })
    .await
}

fn find_similar(content: &str, needle: &str) -> Option<String> {
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
        return best.map(|(i, dist)| {
            format!(
                "did you mean line {}: `{}` ({} char{})?",
                i + 1,
                content_lines[i],
                dist,
                if dist == 1 { "" } else { "s" }
            )
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
    best.map(|(start, dist)| {
        format!(
            "did you mean lines {}-{} ({} char{})?",
            start + 1,
            start + w,
            dist,
            if dist == 1 { "" } else { "s" }
        )
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

pub(super) fn apply_edits<E>(
    content: &str,
    edits: &[Replacement<'_>],
    display: &str,
) -> Result<String, E>
where
    E: EditFailure,
{
    let mut spans: Vec<(usize, usize, &str)> = Vec::with_capacity(edits.len());
    for edit in edits {
        if edit.old.is_empty() {
            return Err(E::empty_old_text());
        }
        let start = content.find(edit.old).ok_or_else(|| {
            let mut msg = format!(
                "`old_text` was not found in `{display}`. Make sure it matches \
                 the file content exactly, including whitespace."
            );
            if let Some(suggestion) = find_similar(content, edit.old) {
                msg.push(' ');
                msg.push_str(&suggestion);
            }
            E::old_text_not_found(display.to_string(), msg)
        })?;
        if let Some(dup) = content[start + edit.old.len()..].find(edit.old) {
            let tail = &content[start + edit.old.len() + dup + edit.old.len()..];
            let extra = tail.matches(edit.old).count();
            let total = 2 + extra;
            let positions = collect_match_lines(content, edit.old);
            let pos_str = positions
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(E::old_text_not_unique(
                display.to_string(),
                format!(
                    "`old_text` matched {total} times in `{display}` at lines \
                     [{pos_str}]; it must match exactly once. Add more surrounding \
                     context to make it unique."
                ),
            ));
        }
        spans.push((start, start + edit.old.len(), edit.new));
    }
    spans.sort_by_key(|(start, _, _)| *start);
    for (prev, next) in spans.iter().tuple_windows() {
        if prev.1 > next.0 {
            return Err(E::overlapping_edits(display.to_string()));
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
    edits: &[Replacement<'_>],
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
        let n = edits.len();
        let noun = if n == 1 { "edit" } else { "edits" };
        let summary = format!("[dry run] would apply {n} {noun} to `{display}`.");
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
    let warning = super::atomic::write_bytes::<ModifyError>(
        path,
        display,
        &updated_bytes,
        Some(perms),
        mtime_before,
    )
    .await?;

    let (replacements, deletions): (Vec<_>, Vec<_>) =
        edits.iter().partition(|edit| !edit.new.is_empty());
    let mut parts: Vec<String> = Vec::new();
    if !replacements.is_empty() {
        let n = replacements.len();
        parts.push(format!("{n} replacement{}", if n == 1 { "" } else { "s" }));
    }
    if !deletions.is_empty() {
        let n = deletions.len();
        parts.push(format!("{n} deletion{}", if n == 1 { "" } else { "s" }));
    }
    let summary = if parts.is_empty() {
        format!("edited `{display}` (no changes).")
    } else {
        format!("edited `{display}` ({}).", parts.join(", "))
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

    #[derive(Debug, thiserror::Error)]
    enum TestEditError {
        #[error("empty old text")]
        EmptyOldText,
        #[error("{message}")]
        OldTextNotFound { path: String, message: String },
        #[error("{message}")]
        OldTextNotUnique { path: String, message: String },
        #[error("overlapping edits in `{path}`")]
        OverlappingEdits { path: String },
    }

    impl EditFailure for TestEditError {
        fn empty_old_text() -> Self {
            Self::EmptyOldText
        }

        fn old_text_not_found(path: String, message: String) -> Self {
            Self::OldTextNotFound { path, message }
        }

        fn old_text_not_unique(path: String, message: String) -> Self {
            Self::OldTextNotUnique { path, message }
        }

        fn overlapping_edits(path: String) -> Self {
            Self::OverlappingEdits { path }
        }
    }

    #[test]
    fn apply_edits_should_replace_unique_text() {
        let edits = [Replacement {
            old: "beta",
            new: "BETA",
        }];

        let updated = apply_edits::<TestEditError>("alpha beta gamma", &edits, "test.txt").unwrap();

        assert_eq!(updated, "alpha BETA gamma");
    }

    #[test]
    fn apply_edits_should_apply_multiple_non_overlapping_replacements() {
        let edits = [
            Replacement {
                old: "alpha",
                new: "A",
            },
            Replacement {
                old: "gamma",
                new: "G",
            },
        ];

        let updated = apply_edits::<TestEditError>("alpha beta gamma", &edits, "test.txt").unwrap();

        assert_eq!(updated, "A beta G");
    }

    #[test]
    fn apply_edits_should_reject_overlapping_matches() {
        let edits = [
            Replacement {
                old: "abc",
                new: "X",
            },
            Replacement {
                old: "cde",
                new: "Y",
            },
        ];

        let result = apply_edits::<TestEditError>("abcdef", &edits, "test.txt");

        assert!(matches!(
            result,
            Err(TestEditError::OverlappingEdits { .. })
        ));
    }

    #[test]
    fn apply_edits_should_reject_non_unique_old_text() {
        let edits = [Replacement {
            old: "dup",
            new: "",
        }];

        let result = apply_edits::<TestEditError>("dup dup", &edits, "test.txt");

        assert!(matches!(
            result,
            Err(TestEditError::OldTextNotUnique { .. })
        ));
    }

    #[test]
    fn apply_edits_should_suggest_similar_text_when_old_text_is_not_found() {
        let edits = [Replacement {
            old: "fn mian() {",
            new: "",
        }];

        let result = apply_edits::<TestEditError>(
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
        let edits = [Replacement {
            old: "dup target",
            new: "",
        }];

        let result = apply_edits::<TestEditError>(
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
