use super::{Read, ReadError};
use crate::common::{format_size, streaming_read_text, truncation_notice};
use std::fmt::Write;
use std::path::Path;

/// 将文本内容渲染为带行号的分页友好输出。
pub(super) fn render(content: &str) -> String {
    let total = content.lines().count();
    if total == 0 {
        return "(empty file)".to_string();
    }
    let width = total.to_string().len();
    let mut out = String::with_capacity(content.len() + total * (width + 3));
    for (i, line) in content.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        write!(out, "{:>width$} | {line}", i + 1).unwrap();
    }
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// 流式读取文本文件（大文件路径），返回渲染后的输出。
pub(super) async fn read_streaming(
    path: &Path,
    display: &str,
    file_size: u64,
    offset_bytes: u64,
    limit_bytes: u64,
) -> Result<String, ReadError> {
    let (content, _, was_truncated) = streaming_read_text(path, offset_bytes, limit_bytes)
        .await
        .map_err(|source| Read::map_io(source, display.to_string()))?;

    let rendered = render(&content);
    let header = if offset_bytes > 0 {
        format!(
            "`{display}` — {size} (reading from byte {offset})\n\n",
            size = format_size(file_size),
            offset = offset_bytes,
        )
    } else {
        format!(
            "`{display}` — {size} (first {read_size})\n\n",
            size = format_size(file_size),
            read_size = format_size(limit_bytes.min(file_size)),
        )
    };

    let unread_bytes = file_size.saturating_sub(offset_bytes);
    let notice = if was_truncated || offset_bytes.saturating_add(limit_bytes) < file_size {
        truncation_notice(content.len() as u64, unread_bytes, "text")
    } else {
        String::new()
    };

    Ok(format!("{header}{rendered}{notice}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_numbers_every_line() {
        let out = render("alpha\nbeta\ngamma\n");
        assert!(out.contains("1 | alpha"));
        assert!(out.contains("2 | beta"));
        assert!(out.contains("3 | gamma"));
    }

    #[test]
    fn render_reports_empty_file() {
        assert_eq!(render(""), "(empty file)");
    }
}
