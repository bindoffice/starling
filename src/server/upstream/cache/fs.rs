use super::*;

#[derive(Debug)]
pub(super) struct CacheFile {
    pub(super) path: PathBuf,
    pub(super) length: u64,
    pub(super) modified: SystemTime,
}

pub(super) async fn scan_files(root: &Path) -> io::Result<Vec<CacheFile>> {
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        let mut entries = tokio::fs::read_dir(&directory).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let name = entry.file_name();
            if file_type.is_dir() {
                if name != "temp" {
                    directories.push(entry.path());
                }
                continue;
            }
            if !file_type.is_file() || name.to_string_lossy().starts_with('.') {
                continue;
            }
            let metadata = entry.metadata().await?;
            files.push(CacheFile {
                path: entry.path(),
                length: metadata.len(),
                modified: metadata.modified().unwrap_or(UNIX_EPOCH),
            });
        }
    }
    Ok(files)
}
