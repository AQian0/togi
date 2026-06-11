use std::path::Path;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// 限量读取文件片段，避免为了探测文件类型或预览内容而把大文件完整载入内存。
pub(super) async fn read_chunk(
    path: &Path,
    offset_bytes: u64,
    limit_bytes: u64,
) -> Result<Vec<u8>, std::io::Error> {
    let mut file = tokio::fs::File::open(path).await?;
    if offset_bytes > 0 {
        file.seek(std::io::SeekFrom::Start(offset_bytes)).await?;
    }
    let cap = limit_bytes.min(usize::MAX as u64) as usize;
    let mut buf = Vec::with_capacity(cap);
    let mut limited = file.take(limit_bytes);
    limited.read_to_end(&mut buf).await?;
    Ok(buf)
}

/// 读取文件头部用于二进制检测。最多读取 `BINARY_DETECTION_SAMPLE_SIZE` 字节。
pub(super) async fn read_head(path: &Path, file_size: u64) -> Result<Vec<u8>, std::io::Error> {
    read_chunk(
        path,
        0,
        file_size.min(crate::constants::BINARY_DETECTION_SAMPLE_SIZE as u64),
    )
    .await
}
