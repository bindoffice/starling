use super::*;

pub(in crate::server) async fn check_path_access(
    path: &Path,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<(), u16> {
    let config = &context.disable_symlinks;
    if config.mode == DisableSymlinksMode::Off {
        return Ok(());
    }

    let from = config
        .from
        .as_deref()
        .map(|value| PathBuf::from(eval_variables(value, variables)));
    let mut base = PathBuf::new();
    let mut relative = path;
    if let Some(from) = from.as_deref() {
        if path == from {
            return Ok(());
        }
        if let Ok(stripped) = path.strip_prefix(from)
            && !stripped.as_os_str().is_empty()
        {
            base = from.to_path_buf();
            relative = stripped;
        }
    }

    let mut current = base;
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => metadata,
            Err(error) => return Err(symlink_status(&error)),
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }
        let allowed = match config.mode {
            DisableSymlinksMode::Off => true,
            DisableSymlinksMode::On => false,
            DisableSymlinksMode::IfNotOwner => {
                symlink_target_has_same_owner(&current, &metadata).await
            }
        };
        if !allowed {
            return Err(403);
        }
    }
    Ok(())
}

pub(in crate::server) fn check_path_access_sync(
    path: &Path,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Result<(), u16> {
    let config = &context.disable_symlinks;
    if config.mode == DisableSymlinksMode::Off {
        return Ok(());
    }

    let from = config
        .from
        .as_deref()
        .map(|value| PathBuf::from(eval_variables(value, variables)));
    let mut base = PathBuf::new();
    let mut relative = path;
    if let Some(from) = from.as_deref() {
        if path == from {
            return Ok(());
        }
        if let Ok(stripped) = path.strip_prefix(from)
            && !stripped.as_os_str().is_empty()
        {
            base = from.to_path_buf();
            relative = stripped;
        }
    }

    let mut current = base;
    for component in relative.components() {
        current.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) => return Err(symlink_status(&error)),
        };
        if !metadata.file_type().is_symlink() {
            continue;
        }
        let allowed = match config.mode {
            DisableSymlinksMode::Off => true,
            DisableSymlinksMode::On => false,
            DisableSymlinksMode::IfNotOwner => {
                symlink_target_has_same_owner_sync(&current, &metadata)
            }
        };
        if !allowed {
            return Err(403);
        }
    }
    Ok(())
}

fn symlink_status(error: &io::Error) -> u16 {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => 404,
        io::ErrorKind::PermissionDenied => 403,
        _ if matches!(
            error.raw_os_error(),
            Some(libc::ELOOP) | Some(libc::EMLINK) | Some(libc::ENAMETOOLONG)
        ) =>
        {
            if error.raw_os_error() == Some(libc::ENAMETOOLONG) {
                404
            } else {
                403
            }
        }
        _ => 500,
    }
}

async fn symlink_target_has_same_owner(path: &Path, metadata: &std::fs::Metadata) -> bool {
    let Ok(target) = tokio::fs::metadata(path).await else {
        return false;
    };
    same_owner(metadata, &target)
}

fn symlink_target_has_same_owner_sync(path: &Path, metadata: &std::fs::Metadata) -> bool {
    let Ok(target) = std::fs::metadata(path) else {
        return false;
    };
    same_owner(metadata, &target)
}

#[cfg(unix)]
fn same_owner(link: &std::fs::Metadata, target: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    link.uid() == target.uid()
}

#[cfg(not(unix))]
fn same_owner(_link: &std::fs::Metadata, _target: &std::fs::Metadata) -> bool {
    false
}
