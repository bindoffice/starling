use super::*;

fn header(name: &str, value: &str) -> Header {
    Header {
        name: name.into(),
        value: value.into(),
    }
}

fn policy() -> CacheHeaderPolicy {
    CacheHeaderPolicy {
        ignore_cache_control: false,
        ignore_expires: false,
        ignore_x_accel_expires: false,
    }
}

fn parse(headers: &[Header], policy: CacheHeaderPolicy, now: u64) -> CacheHeaders {
    response_cache_headers(headers, headers, policy, now)
}

#[test]
fn prefers_s_maxage_and_rejects_invalid_or_zero_max_age() {
    let parsed = parse(
        &[header(
            "Cache-Control",
            "public, max-age=10, s-maxage=30, stale-if-error=60",
        )],
        policy(),
        100,
    );
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, Some(Duration::from_secs(30)));

    for value in ["max-age=broken", "max-age=0", "no-cache=\"Set-Cookie\""] {
        let parsed = parse(&[header("Cache-Control", value)], policy(), 100);
        assert!(!parsed.cacheable, "{value}");
    }
}

#[test]
fn matches_nginx_x_accel_expires_and_expires_precedence() {
    let parsed = parse(
        &[
            header("Cache-Control", "no-cache"),
            header("X-Accel-Expires", "30"),
        ],
        policy(),
        100,
    );
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, Some(Duration::from_secs(30)));

    let parsed = parse(&[header("X-Accel-Expires", "broken")], policy(), 100);
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, None);

    let parsed = parse(
        &[
            header("Expires", "Thu, 01 Jan 1970 00:01:40 GMT"),
            header("X-Accel-Expires", "0"),
        ],
        policy(),
        100,
    );
    assert!(!parsed.cacheable);
}

#[test]
fn parses_absolute_accel_expiry_and_ignores_header_policy() {
    let parsed = parse(&[header("X-Accel-Expires", "@160")], policy(), 100);
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, Some(Duration::from_secs(60)));

    let cleared = parse(
        &[
            header("Cache-Control", "no-cache"),
            header("X-Accel-Expires", "@0"),
        ],
        policy(),
        100,
    );
    assert!(cleared.cacheable);
    assert_eq!(cleared.valid, None);

    let ignored = parse(
        &[
            header("Cache-Control", "no-store"),
            header("Expires", "bad"),
            header("X-Accel-Expires", "0"),
        ],
        CacheHeaderPolicy {
            ignore_cache_control: true,
            ignore_expires: true,
            ignore_x_accel_expires: true,
        },
        100,
    );
    assert!(ignored.cacheable);
    assert_eq!(ignored.valid, None);
}

#[test]
fn hidden_cache_headers_do_not_affect_cache_policy() {
    let parsed = response_cache_headers(
        &[header("Content-Type", "text/plain")],
        &[
            header("Cache-Control", "no-store"),
            header("Expires", "bad"),
        ],
        policy(),
        100,
    );
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, None);
    assert_eq!(parsed.stale, CacheStaleWindows::default());
}

#[test]
fn parses_stale_extensions_independently_of_max_age() {
    let parsed = parse(
        &[header(
            "Cache-Control",
            "max-age=10, stale-while-revalidate=30, stale-if-error=60",
        )],
        policy(),
        100,
    );
    assert!(parsed.cacheable);
    assert_eq!(parsed.stale.updating, Duration::from_secs(30));
    assert_eq!(parsed.stale.error, Duration::from_secs(60));

    let without_freshness = parse(
        &[header("Cache-Control", "stale-while-revalidate=15")],
        policy(),
        100,
    );
    assert!(without_freshness.cacheable);
    assert_eq!(without_freshness.valid, None);
    assert_eq!(without_freshness.stale.updating, Duration::from_secs(15));
    assert_eq!(without_freshness.stale.error, Duration::from_secs(15));

    let invalid = parse(
        &[header("Cache-Control", "max-age=10, stale-if-error=broken")],
        policy(),
        100,
    );
    assert!(!invalid.cacheable);
}

#[test]
fn later_max_age_clears_an_expired_expires_header() {
    let parsed = parse(
        &[
            header("Expires", "Thu, 01 Jan 1970 00:01:40 GMT"),
            header("Cache-Control", "max-age=30"),
        ],
        policy(),
        100,
    );
    assert!(parsed.cacheable);
    assert_eq!(parsed.valid, Some(Duration::from_secs(30)));
}
