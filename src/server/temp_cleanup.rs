use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::*;
use crate::http::temp_file_owner;

pub(in crate::server) fn cleanup_stale_temp_files(config: &RuntimeConfig, logger: &Logger) {
    for root in configured_temp_roots(config) {
        match cleanup_temp_root(&root) {
            Ok(0) => {}
            Ok(removed) => logger.log(
                LogLevel::Info,
                format!(
                    "removed {removed} stale temporary file(s) from {}",
                    root.display()
                ),
            ),
            Err(error) => logger.log(
                LogLevel::Warn,
                format!(
                    "could not clean temporary directory {}: {error}",
                    root.display()
                ),
            ),
        }
    }
}

fn configured_temp_roots(config: &RuntimeConfig) -> BTreeSet<PathBuf> {
    let mut roots = BTreeSet::new();
    let Some(http) = &config.http else {
        return roots;
    };
    for server in &http.servers {
        collect_context_temp_roots(&server.context, &mut roots);
        for location in &server.locations {
            collect_location_temp_roots(location, &mut roots);
        }
    }
    roots
}

fn collect_location_temp_roots(location: &LocationConfig, roots: &mut BTreeSet<PathBuf>) {
    collect_context_temp_roots(&location.context, roots);
    for nested in &location.nested {
        collect_location_temp_roots(nested, roots);
    }
}

fn collect_context_temp_roots(context: &EffectiveConfig, roots: &mut BTreeSet<PathBuf>) {
    roots.insert(context.client_body.temp_path.path.clone());
    roots.insert(context.proxy_buffering.temp_path.path.clone());
    roots.insert(context.fastcgi_buffering.temp_path.path.clone());
    roots.insert(context.uwsgi_buffering.temp_path.path.clone());
    roots.insert(context.scgi_buffering.temp_path.path.clone());
}

fn cleanup_temp_root(root: &Path) -> io::Result<usize> {
    cleanup_temp_root_with(root, |owner| {
        // Startup cleanup runs before this process creates any temp files. A
        // same-PID file therefore belongs to an older process that was killed.
        owner != std::process::id() && process_is_alive(owner)
    })
}

fn cleanup_temp_root_with(
    root: &Path,
    mut owner_is_alive: impl FnMut(u32) -> bool,
) -> io::Result<usize> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    }

    let mut removed = 0;
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                directories.push(entry.path());
            } else if file_type.is_file()
                && temp_file_owner(&entry.file_name()).is_some_and(|owner| !owner_is_alive(owner))
            {
                match fs::remove_file(entry.path()) {
                    Ok(()) => removed += 1,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    Ok(removed)
}

#[cfg(unix)]
pub(in crate::server) fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
pub(in crate::server) fn process_is_alive(_pid: u32) -> bool {
    // Without a reliable liveness check, retain all files from other owners.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "starling-temp-cleanup-{label}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn removes_only_generated_temp_files_including_nested_levels() {
        let root = test_root("generated");
        let nested = root.join("aa").join("bb");
        fs::create_dir_all(&nested).unwrap();
        let first = root.join(format!(".starling-tmp-{}-0000000001", std::process::id()));
        let second = nested.join(format!(".starling-tmp-{}-0000000042", std::process::id()));
        fs::write(&first, b"stale").unwrap();
        fs::write(&second, b"stale").unwrap();
        fs::write(root.join("0000000001"), b"nginx-compatible numeric name").unwrap();
        fs::write(root.join("README"), b"keep").unwrap();
        fs::write(root.join("00000001"), b"not-a-generated-name").unwrap();

        assert_eq!(cleanup_temp_root(&root).unwrap(), 2);
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(root.join("0000000001").exists());
        assert!(root.join("README").exists());
        assert!(root.join("00000001").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_files_owned_by_live_processes() {
        let root = test_root("live-owner");
        fs::create_dir_all(&root).unwrap();
        let live = root.join(".starling-tmp-424242-0000000001");
        let stale = root.join(".starling-tmp-424243-0000000001");
        fs::write(&live, b"active").unwrap();
        fs::write(&stale, b"stale").unwrap();

        let removed = cleanup_temp_root_with(&root, |owner| owner == 424242).unwrap();
        assert_eq!(removed, 1);
        assert!(live.exists());
        assert!(!stale.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_non_directory_roots_are_ignored() {
        let root = test_root("missing");
        assert_eq!(cleanup_temp_root(&root).unwrap(), 0);

        fs::write(&root, b"not a directory").unwrap();
        assert_eq!(cleanup_temp_root(&root).unwrap(), 0);
        fs::remove_file(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let root = test_root("symlink");
        let outside = test_root("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let outside_file = outside.join(format!(".starling-tmp-{}-0000000001", std::process::id()));
        fs::write(&outside_file, b"keep").unwrap();
        symlink(&outside, root.join("linked")).unwrap();

        assert_eq!(cleanup_temp_root(&root).unwrap(), 0);
        assert!(outside_file.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
