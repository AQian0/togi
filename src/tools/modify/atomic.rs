use super::{Modify, ModifyError};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::io::AsyncWriteExt;

pub(super) async fn write_bytes(
    path: &Path,
    display: &str,
    data: &[u8],
    preserve_perms: Option<std::fs::Permissions>,
    expected_mtime: Option<SystemTime>,
) -> Result<Option<String>, ModifyError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|source| Modify::map_io(source, display))?;
    }
    let dir = parent
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| crate::shared::constants::TEMP_FILE_FALLBACK_NAME.to_string());
    let tmp = dir.join(format!(
        ".{file_name}.{}-{}.tmp",
        crate::shared::constants::TEMP_FILE_SUFFIX,
        uuid::Uuid::new_v4().simple()
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
        return Err(Modify::map_io(source, display));
    }

    let mtime_warning = if let Some(expected) = expected_mtime {
        match tokio::fs::metadata(path).await {
            Ok(ref meta) if meta.modified().ok() == Some(expected) => None,
            Ok(_) => Some(crate::t!("modify-mtime-warning")),
            Err(_) => None,
        }
    } else {
        None
    };

    if let Err(source) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(Modify::map_io(source, display));
    }

    Ok(mtime_warning)
}

#[cfg(test)]
mod tests {
    use super::*;

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

        write_bytes(&path, &path.display().to_string(), b"after", None, mtime)
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

            write_bytes(
                &path,
                &path.display().to_string(),
                b"DATA",
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
            write_bytes(
                &path,
                &path.display().to_string(),
                b"DATA",
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
