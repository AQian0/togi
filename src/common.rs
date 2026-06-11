//! 跨模块公共辅助函数：路径解析、参数解析、IO 错误分类、大文件支持。

use crate::constants;
use crate::error::{ErrorKind as TogiErrorKind, TogiError};
use rig::tool::ToolError;
use serde_json::{Map, Value};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// 人类可读文件大小。
#[must_use]
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {unit}", unit = UNITS[unit])
    }
}

/// IO 错误分类，供各工具的 `map_io` 复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IoErrorClass {
    NotFound,
    PermissionDenied,
    NotUtf8,
    Other,
}

/// 将 `std::io::Error` 归类为统一的错误类别。
#[must_use]
pub(crate) fn classify_io_error(source: &std::io::Error) -> IoErrorClass {
    match source.kind() {
        ErrorKind::NotFound => IoErrorClass::NotFound,
        ErrorKind::PermissionDenied => IoErrorClass::PermissionDenied,
        ErrorKind::InvalidData => IoErrorClass::NotUtf8,
        _ => IoErrorClass::Other,
    }
}

/// 工具路径解析错误。
#[derive(Debug, thiserror::Error)]
pub(crate) enum ToolPathError {
    #[error("`path` must not be empty.")]
    EmptyPath,
    #[error("`cwd` was not injected.")]
    MissingCwd,
}

impl TogiError for ToolPathError {
    fn code(&self) -> &'static str {
        match self {
            Self::EmptyPath => "tool.empty_path",
            Self::MissingCwd => "tool.missing_cwd",
        }
    }

    fn kind(&self) -> TogiErrorKind {
        match self {
            Self::EmptyPath => TogiErrorKind::InvalidArgument,
            Self::MissingCwd => TogiErrorKind::MissingRuntimeInjection,
        }
    }
}

/// 将绝对或相对（基于注入的 `cwd`）路径解析为绝对路径。
pub(crate) fn resolve_tool_path(
    cwd: Option<&Path>,
    raw_path: &str,
) -> Result<PathBuf, ToolPathError> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return Err(ToolPathError::EmptyPath);
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = cwd.ok_or(ToolPathError::MissingCwd)?;
    Ok(cwd.join(path))
}

/// 文件过大，拒绝加载。
#[derive(Debug, thiserror::Error)]
#[error(
    "`{path}` is {size} which exceeds the maximum allowed size of {max}. \
     Use the `shell` tool with commands like `head`, `tail`, `sed`, or `xxd` \
     to work with this file instead.",
    max = crate::common::format_size(*max)
)]
pub struct FileTooLargeError {
    pub path: String,
    pub size: u64,
    pub max: u64,
}

impl TogiError for FileTooLargeError {
    fn code(&self) -> &'static str {
        "tool.file_too_large"
    }

    fn kind(&self) -> TogiErrorKind {
        TogiErrorKind::TooLarge
    }
}

/// 检查文件大小，超过 `MAX_FILE_SIZE` 返回 `FileTooLargeError`，
/// 超过 `LARGE_FILE_THRESHOLD` 返回 `Some(size)` 作为警告标记。
pub(crate) fn check_file_size(path: &str, size: u64) -> Result<Option<u64>, FileTooLargeError> {
    if size > constants::MAX_FILE_SIZE {
        return Err(FileTooLargeError {
            path: path.to_string(),
            size,
            max: constants::MAX_FILE_SIZE,
        });
    }
    if size > constants::LARGE_FILE_THRESHOLD {
        Ok(Some(size))
    } else {
        Ok(None)
    }
}

/// 统一的截断提示。
#[must_use]
pub(crate) fn truncation_notice(shown: u64, total: u64, unit: &str) -> String {
    format!(
        "\n(showing {} of {} {unit}; use the `shell` tool with `tail`, `head`, \
         or `sed` to inspect beyond this range)",
        format_size(shown),
        format_size(total),
    )
}

/// 流式读取文本文件的一部分，返回文本内容和是否被截断。
///
/// 从文件中读取 `offset_bytes..offset_bytes+max_bytes` 范围的字节，
/// 自动处理 UTF-8 边界对齐（向后退到合法边界）。
/// 返回 `(text, total_file_size, was_truncated)`。
pub(crate) async fn streaming_read_text(
    path: &Path,
    offset_bytes: u64,
    max_bytes: u64,
) -> Result<(String, u64, bool), std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    let metadata = file.metadata().await?;
    let file_size = metadata.len();

    if offset_bytes >= file_size {
        return Ok((String::new(), file_size, false));
    }

    file.seek(std::io::SeekFrom::Start(offset_bytes)).await?;

    // 读取 max_bytes + 额外余量用于 UTF-8 边界对齐
    let read_limit = (max_bytes as usize + constants::UTF8_ALIGNMENT_BUFFER)
        .min((file_size - offset_bytes) as usize);
    let mut buf = vec![0u8; read_limit];
    let n = file.read(&mut buf).await?;
    buf.truncate(n);

    let was_truncated = (offset_bytes + n as u64) < file_size;

    // UTF-8 边界对齐：如果缓冲区末尾切断了多字节字符，向后退
    let valid_len = if n > 0 {
        let mut end = n;
        while end > 0 && std::str::from_utf8(&buf[..end]).is_err() {
            end -= 1;
        }
        end
    } else {
        0
    };

    let text = if valid_len > 0 {
        String::from_utf8(buf[..valid_len].to_vec())
            .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?
    } else {
        String::new()
    };

    Ok((text, file_size, was_truncated))
}

/// 通过采样的启发式判断字节序列是否为二进制内容。
/// 规则：包含 null 字节，或不可打印字符（排除 \n \r \t）占比超过阈值。
#[must_use]
pub(crate) fn is_binary(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let check_len = data.len().min(constants::BINARY_DETECTION_SAMPLE_SIZE);
    let slice = &data[..check_len];
    if slice.contains(&0) {
        return true;
    }
    let non_printable = slice
        .iter()
        .filter(|&&b| b != b'\n' && b != b'\r' && b != b'\t' && !(0x20..=0x7E).contains(&b))
        .count();
    non_printable as f64 / check_len as f64 > constants::BINARY_NON_PRINTABLE_RATIO
}

/// 将工具 JSON 参数字符串解析为 `Map<String, Value>`。
///
/// 空字符串和 `null` 统一返回空 Map。
pub(crate) fn parse_args_object(args: &str) -> Result<Map<String, Value>, ToolError> {
    let trimmed = args.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Map::new());
    }
    serde_json::from_str::<Map<String, Value>>(trimmed).map_err(ToolError::JsonError)
}

/// 将 diff 拼接到摘要文本后，无 diff 时根据文件是否已存在给出不同提示。
#[must_use]
pub(crate) fn append_diff(summary: String, diff: Option<String>, existed: bool) -> String {
    match diff {
        Some(diff) => format!("{summary}\n{diff}"),
        None if existed => format!("{summary}\n(no changes)"),
        None => summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_kb() {
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
    }

    #[test]
    fn format_size_mb() {
        assert_eq!(format_size(1048576), "1.0 MB");
    }

    #[test]
    fn append_diff_with_diff() {
        let result = append_diff("summary".into(), Some("diff".into()), true);
        assert_eq!(result, "summary\ndiff");
    }

    #[test]
    fn append_diff_without_diff_existed() {
        let result = append_diff("summary".into(), None, true);
        assert_eq!(result, "summary\n(no changes)");
    }

    #[test]
    fn append_diff_without_diff_new_file() {
        let result = append_diff("summary".into(), None, false);
        assert_eq!(result, "summary");
    }
}
