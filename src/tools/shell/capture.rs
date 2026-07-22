use crate::shared::constants;
use tokio::io::{AsyncRead, AsyncReadExt};

pub(super) struct StreamCapture {
    pub(super) data: Vec<u8>,
    pub(super) total: usize,
}

pub(super) async fn read_limited<R>(reader: R, cap: usize) -> StreamCapture
where
    R: AsyncRead + Unpin,
{
    let mut reader = tokio::io::BufReader::new(reader);
    let mut buf = [0u8; constants::SHELL_READ_BUFFER_SIZE];
    let mut data = Vec::with_capacity(cap.min(constants::SHELL_READ_BUFFER_SIZE));
    let mut total = 0usize;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                total = total.saturating_add(n);
                let remaining = cap.saturating_sub(data.len());
                if remaining > 0 {
                    data.extend_from_slice(&buf[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    StreamCapture { data, total }
}
