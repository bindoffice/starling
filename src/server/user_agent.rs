pub(super) fn is_msie6(user_agent: &str) -> bool {
    let Some(position) = legacy_msie_position(user_agent) else {
        return false;
    };
    match user_agent.as_bytes()[position + 5] {
        b'4' | b'5' => true,
        b'6' => !user_agent.as_bytes()[position + 8..]
            .windows(3)
            .any(|bytes| bytes == b"SV1"),
        _ => false,
    }
}

pub(super) fn is_safari(user_agent: &str) -> bool {
    !user_agent.contains("Opera")
        && !is_msie(user_agent)
        && !user_agent.contains("Gecko/")
        && !user_agent.contains("Chrome/")
        && user_agent.contains("Safari/")
        && user_agent.contains("Mac OS X")
}

pub(super) fn is_msie(user_agent: &str) -> bool {
    !user_agent.contains("Opera")
        && user_agent
            .find("MSIE ")
            .is_some_and(|position| position + 7 < user_agent.len())
}

pub(super) fn is_chrome(user_agent: &str) -> bool {
    !user_agent.contains("Opera")
        && !is_msie(user_agent)
        && !user_agent.contains("Gecko/")
        && user_agent.contains("Chrome/")
}

fn legacy_msie_position(user_agent: &str) -> Option<usize> {
    if user_agent.contains("Opera") {
        return None;
    }
    let position = user_agent.find("MSIE ")?;
    let bytes = user_agent.as_bytes();
    (bytes.get(position + 6) == Some(&b'.') && position + 7 < bytes.len()).then_some(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_msie6_like_nginx() {
        assert!(is_msie6(
            "Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1)"
        ));
        assert!(is_msie6(
            "Mozilla/4.0 (compatible; MSIE 5.5; Windows NT 5.0)"
        ));
        assert!(!is_msie6(
            "Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1; SV1)"
        ));
        assert!(!is_msie6(
            "Mozilla/4.0 (compatible; MSIE 6.0; Windows NT 5.1) Opera 8.0"
        ));
        assert!(is_msie6("MSIE 6.\u{e9}"));
        assert!(!is_msie6("Mozilla/5.0 (X11; Linux x86_64)"));
    }

    #[test]
    fn detects_safari_like_nginx() {
        assert!(is_safari(
            "Mozilla/5.0 (Macintosh; U; Intel Mac OS X 10_6_8) AppleWebKit/533 Safari/5.0"
        ));
        assert!(!is_safari(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537 Chrome/120 Safari/537"
        ));
        assert!(!is_safari(
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Safari/537"
        ));
        assert!(!is_safari(
            "Mozilla/5.0 (Macintosh) AppleWebKit/533 Safari/6.0 Opera/12"
        ));
    }

    #[test]
    fn detects_msie_and_chrome_like_nginx() {
        let msie = "Mozilla/4.0 (compatible; MSIE 8.0; Windows NT 6.1)";
        assert!(is_msie(msie));
        assert!(!is_chrome(msie));
        assert!(!is_msie(
            "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/120 Safari/537.36"
        ));
        assert!(is_chrome(
            "Mozilla/5.0 (Windows NT 10.0) AppleWebKit/537.36 Chrome/120 Safari/537.36"
        ));
        assert!(!is_chrome(
            "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Chrome/120"
        ));
        assert!(!is_chrome(
            "Mozilla/5.0 (Windows NT 10.0) Chrome/120 Safari/537.36 Opera/12"
        ));
        assert!(is_msie("MSIE abcdef"));
        assert!(!is_msie("MSIE ab"));
    }
}
