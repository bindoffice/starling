use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(in crate::server) fn temporary_path(parent: &Path, label: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(".starling-{label}-{}-{id}", std::process::id()))
}

fn temporary_path_parts(name: &std::ffi::OsStr) -> Option<(&str, u32)> {
    let name = name.to_str()?;
    let (label, rest) = name.strip_prefix(".starling-")?.split_once('-')?;
    if !matches!(label, "cache" | "store") {
        return None;
    }
    let (pid, sequence) = rest.split_once('-')?;
    if sequence.is_empty() || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((label, pid.parse().ok()?))
}

pub(in crate::server) async fn remove_stale_temporary_files(
    root: &Path,
    label: &str,
) -> io::Result<u64> {
    remove_stale_temporary_files_with(root, label, true, |owner| {
        owner == std::process::id() || crate::server::temp_cleanup::process_is_alive(owner)
    })
    .await
}

pub(in crate::server) async fn remove_stale_temporary_files_in_directory(
    directory: &Path,
    label: &str,
) -> io::Result<u64> {
    remove_stale_temporary_files_with(directory, label, false, |owner| {
        owner == std::process::id() || crate::server::temp_cleanup::process_is_alive(owner)
    })
    .await
}

async fn remove_stale_temporary_files_with(
    root: &Path,
    label: &str,
    recursive: bool,
    mut owner_is_alive: impl FnMut(u32) -> bool + Send,
) -> io::Result<u64> {
    match tokio::fs::symlink_metadata(root).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }

    let mut removed = 0;
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                if recursive {
                    directories.push(entry.path());
                }
                continue;
            }
            if !file_type.is_file()
                || !temporary_path_parts(&entry.file_name())
                    .is_some_and(|(found, owner)| found == label && !owner_is_alive(owner))
            {
                continue;
            }
            match tokio::fs::remove_file(entry.path()).await {
                Ok(()) => removed += 1,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(removed)
}

pub(in crate::server) async fn replace_file(temporary: &Path, path: &Path) -> io::Result<()> {
    if let Err(error) = tokio::fs::rename(temporary, path).await {
        if cfg!(windows) && path.exists() {
            tokio::fs::remove_file(path).await?;
            tokio::fs::rename(temporary, path).await?;
        } else {
            return Err(error);
        }
    }
    Ok(())
}

pub(in crate::server) struct TemporaryFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TemporaryFileGuard {
    pub(in crate::server) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(in crate::server) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_file_guard_removes_armed_files() {
        let parent = std::env::temp_dir().join(format!(
            "starling-temp-guard-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&parent).unwrap();
        let path = temporary_path(&parent, "guard-test");
        std::fs::write(&path, b"temporary").unwrap();

        {
            let _guard = TemporaryFileGuard::new(path.clone());
        }
        assert!(!path.exists());

        std::fs::write(&path, b"committed").unwrap();
        {
            let mut guard = TemporaryFileGuard::new(path.clone());
            guard.disarm();
        }
        assert!(path.exists());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn temporary_names_identify_cache_and_store_owners_only() {
        assert_eq!(
            temporary_path_parts(std::ffi::OsStr::new(".starling-cache-123-4")),
            Some(("cache", 123))
        );
        assert_eq!(
            temporary_path_parts(std::ffi::OsStr::new(".starling-store-456-7")),
            Some(("store", 456))
        );
        for name in [
            ".starling-tmp-123-4",
            ".starling-dav-123-4",
            ".starling-cache-owner-4",
            ".starling-cache-123-",
            ".starling-cache-123-4x",
            ".starling-cache-123-4-extra",
        ] {
            assert_eq!(temporary_path_parts(std::ffi::OsStr::new(name)), None);
        }
    }

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "starling-upstream-cleanup-{label}-{}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn removes_only_the_requested_label_whose_owner_is_gone() {
        let root = test_root("owners");
        let nested = root.join("aa").join("bb");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let stale = root.join(".starling-cache-424242-1");
        let active = root.join(".starling-cache-424243-1");
        let other_label = root.join(".starling-store-424242-1");
        let published = root.join("424242");
        tokio::fs::write(&stale, b"stale").await.unwrap();
        tokio::fs::write(&active, b"active").await.unwrap();
        tokio::fs::write(&other_label, b"store").await.unwrap();
        tokio::fs::write(&published, b"cache entry").await.unwrap();
        let nested_stale = nested.join(".starling-cache-424244-1");
        tokio::fs::write(&nested_stale, b"stale").await.unwrap();

        let removed =
            remove_stale_temporary_files_with(&root, "cache", true, |owner| owner == 424243)
                .await
                .unwrap();
        assert_eq!(removed, 2);
        assert!(!stale.exists());
        assert!(active.exists());
        assert!(other_label.exists());
        assert!(published.exists());
        assert!(!nested_stale.exists());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }

    #[tokio::test]
    async fn ignores_missing_roots_and_does_not_follow_directory_symlinks() {
        let root = test_root("symlink");
        let outside = test_root("outside");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::create_dir_all(&outside).await.unwrap();
        let outside_file = outside.join(".starling-store-424242-1");
        tokio::fs::write(&outside_file, b"keep").await.unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, root.join("linked")).unwrap();
        #[cfg(not(unix))]
        tokio::fs::create_dir_all(root.join("linked"))
            .await
            .unwrap();

        let removed = remove_stale_temporary_files_with(&root, "store", true, |_owner| false)
            .await
            .unwrap();
        assert_eq!(removed, 0);
        assert!(outside_file.exists());

        let missing = test_root("missing");
        assert_eq!(
            remove_stale_temporary_files_with(&missing, "store", true, |_owner| false)
                .await
                .unwrap(),
            0
        );
        tokio::fs::remove_dir_all(root).await.unwrap();
        tokio::fs::remove_dir_all(outside).await.unwrap();
    }

    #[tokio::test]
    async fn non_recursive_cleanup_removes_only_direct_children() {
        let root = test_root("direct");
        let nested = root.join("nested");
        tokio::fs::create_dir_all(&nested).await.unwrap();
        let direct = root.join(".starling-store-424242-1");
        let nested_stale = nested.join(".starling-store-424243-1");
        tokio::fs::write(&direct, b"direct").await.unwrap();
        tokio::fs::write(&nested_stale, b"nested").await.unwrap();

        let removed = remove_stale_temporary_files_with(&root, "store", false, |_owner| false)
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert!(!direct.exists());
        assert!(nested_stale.exists());
        tokio::fs::remove_dir_all(root).await.unwrap();
    }
}
