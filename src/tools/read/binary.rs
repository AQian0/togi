use super::{Read, ReadError, io};
use crate::common::format_size;
use crate::constants;
use std::fmt::Write;
use std::path::Path;

/// 生成经典 hexdump 格式：
/// 每行 `HEXDUMP_BYTES_PER_ROW` 字节，偏移 + hex + ASCII 预览。
/// `base_offset` 用于大文件场景，使偏移从文件的实际位置开始计数。
pub(super) fn render_hexdump(data: &[u8], max_bytes: usize, base_offset: u64) -> String {
    const ROW: usize = constants::HEXDUMP_BYTES_PER_ROW;
    let len = data.len().min(max_bytes);
    let truncated = len < data.len();
    let mut out = String::with_capacity(len * 5 + 256);
    for (row_start, chunk) in data[..len].chunks(ROW).enumerate() {
        let offset = base_offset as usize + row_start * ROW;
        write!(out, "{offset:08x}  ").unwrap();
        for (j, &b) in chunk.iter().enumerate() {
            if j == ROW / 2 {
                out.push(' ');
            }
            write!(out, "{b:02x} ").unwrap();
        }
        let missing = ROW - chunk.len();
        for j in 0..missing {
            if chunk.len() + j == ROW / 2 {
                out.push(' ');
            }
            out.push_str("   ");
        }
        out.push(' ');
        out.push('|');
        for &b in chunk {
            if b.is_ascii_graphic() || b == b' ' {
                out.push(b as char);
            } else {
                out.push('.');
            }
        }
        out.push('|');
        out.push('\n');
    }
    if truncated {
        write!(
            out,
            "\n(showing {len} of {} bytes; use `shell` with `xxd`, `file`, or `hexdump` for full content)",
            data.len()
        )
        .unwrap();
    }
    out
}

pub(super) fn render_base64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}

pub(super) struct BinaryReadRequest<'a> {
    pub(super) path: &'a Path,
    pub(super) display: &'a str,
    pub(super) file_size: u64,
    pub(super) offset_bytes: u64,
    pub(super) remaining_bytes: u64,
    pub(super) is_large: bool,
    pub(super) encoding: &'a str,
    pub(super) limit_bytes: Option<u64>,
}

pub(super) async fn read_binary(request: BinaryReadRequest<'_>) -> Result<String, ReadError> {
    let BinaryReadRequest {
        path,
        display,
        file_size,
        offset_bytes,
        remaining_bytes,
        is_large,
        encoding,
        limit_bytes,
    } = request;

    match encoding {
        "hex" => {
            let preview = if remaining_bytes == 0 {
                Vec::new()
            } else {
                io::read_chunk(
                    path,
                    offset_bytes,
                    remaining_bytes.min(constants::HEXDUMP_MAX_BYTES as u64),
                )
                .await
                .map_err(|source| Read::map_io(source, display.to_string()))?
            };
            let hex = render_hexdump(&preview, constants::HEXDUMP_MAX_BYTES, offset_bytes);
            let range = if offset_bytes > 0 {
                format!(" (from byte {offset_bytes})")
            } else {
                String::new()
            };
            Ok(format!(
                "(binary) `{display}` — {size}{range}\n\n{hex}",
                size = format_size(file_size),
            ))
        }
        "base64" => {
            let default_limit = if is_large {
                constants::DEFAULT_MAX_READ_BYTES
            } else {
                remaining_bytes
            };
            let limit = limit_bytes.unwrap_or(default_limit).min(remaining_bytes);
            let data = if limit == 0 {
                Vec::new()
            } else {
                io::read_chunk(path, offset_bytes, limit)
                    .await
                    .map_err(|source| Read::map_io(source, display.to_string()))?
            };
            let b64 = render_base64(&data);
            let warning = if limit < remaining_bytes {
                format!(
                    "\n(warning: only {} of the remaining {} were base64-encoded; \
                     pass `offset_bytes`/`limit_bytes` or use `shell` with `base64` for more)",
                    format_size(limit),
                    format_size(remaining_bytes),
                )
            } else {
                String::new()
            };
            let range = if offset_bytes > 0 {
                format!(" from byte {offset_bytes}")
            } else {
                String::new()
            };
            Ok(format!(
                "(binary) `{display}` — {size}, base64{range}:\n\n{b64}{warning}",
                size = format_size(file_size),
            ))
        }
        enc => Err(ReadError::InvalidEncoding {
            enc: enc.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hexdump_small() {
        let data = b"Hello\0World";
        let out = render_hexdump(data, 512, 0);
        assert!(out.contains("00000000"));
        assert!(out.contains("48 65 6c 6c 6f 00 57"));
    }

    #[test]
    fn hexdump_truncates() {
        let data = vec![0u8; 1024];
        let out = render_hexdump(&data, 16, 0);
        assert!(out.contains("showing 16 of 1024 bytes"));
    }

    #[test]
    fn hexdump_respects_base_offset() {
        let data = b"0123456789ABCDEF";
        let out = render_hexdump(data, 512, 0x1000);
        assert!(
            out.contains("00001000"),
            "should show base offset 0x1000, got: {out}"
        );
    }

    #[test]
    fn base64_encodes() {
        let out = render_base64(b"hello");
        assert_eq!(out, "aGVsbG8=");
    }
}
