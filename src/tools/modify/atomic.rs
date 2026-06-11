use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

pub(super) trait AtomicWriteFailure: Sized {
    fn from_atomic_io(source: std::io::Error, display: &str) -> Self;
}

fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let rand = random_u64();
    format!("{pid}-{seq}-{nanos:x}-{rand:x}")
}

async fn atomic_write_bytes_inner<E>(
    path: &Path,
    display: &str,
    data: &[u8],
    preserve_perms: Option<std::fs::Permissions>,
    expected_mtime: Option<SystemTime>,
) -> Result<Option<String>, E>
where
    E: AtomicWriteFailure,
{
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| E::from_atomic_io(source, display))?;
    }
    let dir = parent
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::constants::TEMP_FILE_FALLBACK_NAME.to_string());
    let tmp = dir.join(format!(
        ".{file_name}.{}-{}.tmp",
        crate::constants::TEMP_FILE_SUFFIX,
        unique_suffix()
    ));

    let write_result = async {
        let mut file = tokio::fs::File::create(&tmp).await?;
        if let Some(ref perms) = preserve_perms {
            file.set_permissions(perms.clone()).await?;
        }
        file.write_all(data).await?;
        file.flush().await?;
        file.sync_all().await?;
        Ok::<(), std::io::Error>(())
    }
    .await;

    if let Err(source) = write_result {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(E::from_atomic_io(source, display));
    }

    let mtime_warning = if let Some(expected) = expected_mtime {
        match tokio::fs::metadata(path).await {
            Ok(ref meta) if meta.modified().ok() == Some(expected) => None,
            Ok(_) => Some("warning: file was modified externally before write".to_string()),
            Err(_) => None,
        }
    } else {
        None
    };

    if let Err(source) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(E::from_atomic_io(source, display));
    }

    Ok(mtime_warning)
}

pub(super) async fn write_text<E>(
    path: &Path,
    display: &str,
    content: &str,
    preserve_perms: Option<std::fs::Permissions>,
    expected_mtime: Option<SystemTime>,
) -> Result<Option<String>, E>
where
    E: AtomicWriteFailure,
{
    atomic_write_bytes_inner(
        path,
        display,
        content.as_bytes(),
        preserve_perms,
        expected_mtime,
    )
    .await
}

pub(super) async fn write_bytes<E>(
    path: &Path,
    display: &str,
    data: &[u8],
    preserve_perms: Option<std::fs::Permissions>,
    expected_mtime: Option<SystemTime>,
) -> Result<Option<String>, E>
where
    E: AtomicWriteFailure,
{
    atomic_write_bytes_inner(path, display, data, preserve_perms, expected_mtime).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    enum TestAtomicError {
        #[error("io error for `{display}`: {source}")]
        Io {
            display: String,
            source: std::io::Error,
        },
    }

    impl AtomicWriteFailure for TestAtomicError {
        fn from_atomic_io(source: std::io::Error, display: &str) -> Self {
            Self::Io {
                display: display.to_string(),
                source,
            }
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("togi-modify-atomic-{}-{name}", std::process::id()));
        dir
    }

    #[tokio::test]
    async fn write_text_should_leave_no_temp_file_behind() {
        let dir = temp_dir("tmpcheck");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.txt");
        std::fs::write(&path, "before").unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().ok();

        write_text::<TestAtomicError>(&path, &path.display().to_string(), "after", None, mtime)
            .await
            .unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after");
        let entries: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec!["file.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_text_should_preserve_permissions_when_requested() {
        let path = temp_dir("perms.txt");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "data").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            let preserved = metadata.permissions();
            let mtime = metadata.modified().ok();

            write_text::<TestAtomicError>(
                &path,
                &path.display().to_string(),
                "DATA",
                Some(preserved),
                mtime,
            )
            .await
            .unwrap();

            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755, "permissions should be preserved");
        }

        #[cfg(not(unix))]
        {
            let metadata = std::fs::metadata(&path).unwrap();
            write_text::<TestAtomicError>(
                &path,
                &path.display().to_string(),
                "DATA",
                Some(metadata.permissions()),
                metadata.modified().ok(),
            )
            .await
            .unwrap();
        }

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "DATA");
        let _ = std::fs::remove_file(&path);
    }
}
