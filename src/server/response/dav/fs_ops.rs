use super::*;

pub(super) fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

pub(super) async fn create_dir(path: &Path, mode: u32) -> io::Result<()> {
    tokio::fs::create_dir(path).await?;
    set_mode(path, dir_mode(mode))
}

pub(super) async fn create_parent_dirs(path: &Path, mode: u32) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || tokio::fs::metadata(parent).await.is_ok() {
        return Ok(());
    }
    tokio::fs::create_dir_all(parent).await?;
    set_mode(parent, dir_mode(mode))
}

pub(super) async fn remove(path: &Path, metadata: &std::fs::Metadata) -> io::Result<()> {
    if metadata.is_dir() {
        tokio::fs::remove_dir_all(path).await
    } else {
        tokio::fs::remove_file(path).await
    }
}

pub(super) async fn copy(source: &Path, destination: &Path) -> io::Result<()> {
    Box::pin(copy_inner(source, destination)).await
}

async fn copy_inner(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = tokio::fs::metadata(source).await?;
    if metadata.is_dir() {
        let mode = dir_mode(file_mode(&metadata));
        create_dir(destination, mode).await?;
        let mut entries = tokio::fs::read_dir(source).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let source = entry.path();
            let destination = destination.join(name);
            Box::pin(copy_inner(&source, &destination)).await?;
        }
        let _ = set_modified(destination, metadata.modified().ok());
        return Ok(());
    }
    tokio::fs::copy(source, destination).await?;
    if let Ok(metadata) = tokio::fs::metadata(source).await {
        let _ = set_mode(destination, file_mode(&metadata));
        let _ = set_modified(destination, metadata.modified().ok());
    }
    Ok(())
}

pub(super) fn set_modified(path: &Path, modified: Option<std::time::SystemTime>) -> io::Result<()> {
    let Some(modified) = modified else {
        return Ok(());
    };
    let file = std::fs::File::open(path)?;
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
}

pub(super) fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode()
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        0o600
    }
}

pub(super) fn dir_mode(mode: u32) -> u32 {
    mode | ((mode & 0o444) >> 2)
}
