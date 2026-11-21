use super::*;
use sha1::{Digest, Sha1};

pub(super) fn cache_key_hash(key: &str) -> String {
    format!("{:x}", Sha1::digest(key.as_bytes()))
}

pub(super) fn cache_file_hash(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name.len() == 40 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(name.to_ascii_lowercase())
    } else {
        None
    }
}

pub(super) fn cache_entry_path(root: &Path, levels: [usize; 3], hash: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    let mut offset = 0_usize;
    for level in levels {
        if level == 0 {
            break;
        }
        let end = offset.saturating_add(level).min(hash.len());
        path.push(&hash[offset..end]);
        offset = end;
    }
    path.push(&hash[offset..]);
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_keys_and_builds_nginx_style_paths() {
        let hash = cache_key_hash("GET /");
        assert_eq!(hash.len(), 40);
        let path = cache_entry_path(Path::new("/cache"), [1, 2, 0], &hash);
        assert_eq!(
            path,
            Path::new("/cache")
                .join(&hash[..1])
                .join(&hash[1..3])
                .join(&hash[3..])
        );
    }
}
