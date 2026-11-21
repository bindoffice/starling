use super::*;

pub(in crate::server) fn allows_cache_store(
    request: &Request,
    max_range_offset: Option<u64>,
) -> bool {
    let Some(max_range_offset) = max_range_offset else {
        return true;
    };
    let Some(range) = request.header("range") else {
        return true;
    };
    if max_range_offset == 0 {
        return false;
    }

    let unit_len = "bytes=".len();
    if range.len() < unit_len + 1 || !range.as_bytes()[..unit_len].eq_ignore_ascii_case(b"bytes=") {
        return true;
    }

    let value = range[unit_len..].trim_start_matches(' ');
    if value.starts_with('-') {
        return false;
    }
    let digits = value
        .as_bytes()
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    if digits == 0 {
        return true;
    }

    value[..digits]
        .parse::<i64>()
        .map_or(true, |offset| (offset as u64) < max_range_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(range: Option<&str>) -> Request {
        Request {
            method: "GET".into(),
            target: "/".into(),
            version: "HTTP/1.1".into(),
            headers: range
                .map(|range| Header {
                    name: "Range".into(),
                    value: range.into(),
                })
                .into_iter()
                .collect(),
            body_stream: None,
            body: Vec::new(),
            remote_addr: "127.0.0.1:1".into(),
            local_addr: "127.0.0.1:80".into(),
            keep_alive: false,
        }
    }

    #[test]
    fn matches_nginx_max_range_offset_boundaries() {
        assert!(allows_cache_store(&request(None), Some(0)));
        assert!(!allows_cache_store(&request(Some("bytes=0-")), Some(0)));
        assert!(!allows_cache_store(&request(Some("items=0-")), Some(0)));

        assert!(allows_cache_store(&request(Some("bytes=3-")), Some(4)));
        assert!(allows_cache_store(&request(Some("BYTES= 3-7")), Some(4)));
        assert!(!allows_cache_store(&request(Some("bytes=4-")), Some(4)));
        assert!(!allows_cache_store(&request(Some("bytes=-2")), Some(4)));
        assert!(!allows_cache_store(&request(Some("bytes=  -2")), Some(4)));
    }

    #[test]
    fn ignores_unlimited_non_byte_and_malformed_ranges() {
        let ranged = request(Some("bytes=100-"));
        assert!(allows_cache_store(&ranged, None));
        assert!(allows_cache_store(&request(Some("items=100-")), Some(4)));
        assert!(allows_cache_store(&request(Some("bytes=")), Some(4)));
        assert!(allows_cache_store(&request(Some("bytes=broken")), Some(4)));
        assert!(allows_cache_store(
            &request(Some("bytes=999999999999999999999999-")),
            Some(4)
        ));
        assert!(allows_cache_store(
            &request(Some("bytes=9223372036854775808-")),
            Some(4)
        ));
    }
}
