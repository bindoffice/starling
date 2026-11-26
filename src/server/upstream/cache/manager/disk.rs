use super::*;

pub(super) fn available_space(path: &Path) -> io::Result<Option<u64>> {
    #[cfg(unix)]
    {
        let Ok(stats) = rustix::fs::statvfs(path) else {
            return Ok(None);
        };
        let bytes = u128::from(stats.f_bavail) * u128::from(stats.f_frsize);
        Ok(Some(u64::try_from(bytes).unwrap_or(u64::MAX)))
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn required_space_matches_nginx_strict_free_check() {
        assert_eq!(super::super::min_free_eviction(10, 10), 1);
        assert_eq!(super::super::min_free_eviction(4, 10), 7);
        assert_eq!(super::super::min_free_eviction(11, 10), 0);
    }
}
