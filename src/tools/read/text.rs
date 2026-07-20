use super::{Read, ReadError, io};
use crate::shared::util::{format_size, streaming_read_text_with_encoding, truncation_notice};
use std::fmt::Write;
use std::path::Path;

/// 将文本内容渲染为带行号的分页友好输出。
#[must_use]
pub(super) fn render(content: &str) -> String {
    let total = content.lines().count();
    if total == 0 {
        return crate::t!("read-empty-file");
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
    encoding: Option<&'static encoding_rs::Encoding>,
) -> Result<String, ReadError> {
    let encoding = encoding.unwrap_or(encoding_rs::UTF_8);
    let mut effective_offset = offset_bytes;
    if offset_bytes == 0
        && let Ok(head) = io::read_chunk(path, 0, 4).await
        && let Some((bom_encoding, bom_len)) = crate::shared::text_encoding::encoding_for_bom(&head)
        && std::ptr::eq(bom_encoding, encoding)
    {
        effective_offset = bom_len as u64;
    }
    let (content, _, was_truncated) =
        streaming_read_text_with_encoding(path, effective_offset, limit_bytes, encoding)
            .await
            .map_err(|source| Read::map_io(source, display.to_string()))?;

    let rendered = render(&content);
    let header = if offset_bytes > 0 {
        format!(
            "{}\n\n",
            crate::t!(
                "read-header-offset",
                display = display.to_string(),
                size = format_size(file_size),
                offset = offset_bytes
            )
        )
    } else {
        format!(
            "{}\n\n",
            crate::t!(
                "read-header-first",
                display = display.to_string(),
                size = format_size(file_size),
                read_size = format_size(limit_bytes.min(file_size))
            )
        )
    };

    let unread_bytes = file_size.saturating_sub(offset_bytes);
    let notice = if was_truncated || offset_bytes.saturating_add(limit_bytes) < file_size {
        truncation_notice(
            content.len() as u64,
            unread_bytes,
            &crate::t!("common-unit-text"),
        )
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
        assert_eq!(render(""), crate::t!("read-empty-file"));
    }
}
