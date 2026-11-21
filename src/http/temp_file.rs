use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::config::TempPathConfig;

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

const TEMP_FILE_PREFIX: &str = ".starling-tmp-";

#[derive(Debug)]
pub struct TempFileGuard {
    path: PathBuf,
    remove_on_drop: bool,
}

impl TempFileGuard {
    pub fn new(path: PathBuf, remove_on_drop: bool) -> Self {
        Self {
            path,
            remove_on_drop,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.remove_on_drop {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn create_temp_file(
    temp_path: &TempPathConfig,
    label: &str,
) -> io::Result<(PathBuf, std::fs::File)> {
    for _ in 0..1024 {
        let id = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
        let name = temp_file_name(id);
        let path = hashed_temp_path(temp_path, &name);
        let parent = path.parent().unwrap_or(&temp_path.path);
        std::fs::create_dir_all(parent)?;

        let mut options = std::fs::OpenOptions::new();
        options.write(true).read(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("unable to allocate a {label} temp file"),
    ))
}

fn temp_file_name(id: u64) -> String {
    format!("{TEMP_FILE_PREFIX}{}-{id:010}", std::process::id())
}

pub(crate) fn temp_file_owner(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?;
    let (pid, sequence) = name.strip_prefix(TEMP_FILE_PREFIX)?.split_once('-')?;
    if sequence.len() != 10 || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    pid.parse().ok()
}

fn hashed_temp_path(temp_path: &TempPathConfig, name: &str) -> PathBuf {
    let mut path = temp_path.path.clone();
    let mut end = name.len();
    for level in temp_path.levels {
        if level == 0 || end < level {
            break;
        }
        end -= level;
        path.push(&name[end..end + level]);
    }
    path.push(name);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_carry_the_process_owner() {
        let name = temp_file_name(42);
        assert_eq!(
            temp_file_owner(std::ffi::OsStr::new(&name)),
            Some(std::process::id())
        );
        assert!(name.ends_with("-0000000042"));
    }

    #[test]
    fn rejects_non_starling_and_malformed_names() {
        for name in [
            "0000000042",
            ".starling-tmp-",
            ".starling-tmp-123-",
            ".starling-tmp-123-000000042",
            ".starling-tmp-123-000000004x",
            ".starling-tmp-owner-0000000042",
        ] {
            assert_eq!(temp_file_owner(std::ffi::OsStr::new(name)), None);
        }
    }
}
