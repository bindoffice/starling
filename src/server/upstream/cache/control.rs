use super::*;

pub(super) struct CacheHeaderPolicy {
    pub(super) ignore_cache_control: bool,
    pub(super) ignore_expires: bool,
    pub(super) ignore_x_accel_expires: bool,
}

pub(super) struct CacheHeaders {
    pub(super) valid: Option<Duration>,
    pub(super) stale: CacheStaleWindows,
    pub(super) cacheable: bool,
}

pub(super) fn response_cache_headers(
    visible_headers: &[Header],
    raw_headers: &[Header],
    policy: CacheHeaderPolicy,
    now: u64,
) -> CacheHeaders {
    let cache_control_visible = visible_headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("cache-control"));
    let expires_visible = visible_headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("expires"));
    let mut valid = None;
    let mut cacheable = true;
    let mut no_cache = false;
    let mut expired = false;
    let mut x_accel_seen = false;
    let mut x_accel_processed = false;
    let mut expires_processed = false;
    let mut stale = CacheStaleWindows::default();

    for header in raw_headers {
        if header.name.eq_ignore_ascii_case("cache-control") {
            if cache_control_visible && !policy.ignore_cache_control {
                apply_cache_control(
                    &header.value,
                    valid.is_some() && x_accel_seen,
                    &mut valid,
                    &mut cacheable,
                    &mut no_cache,
                    &mut expired,
                    &mut stale,
                );
            }
            continue;
        }

        if header.name.eq_ignore_ascii_case("expires") {
            if expires_visible && !policy.ignore_expires && !expires_processed {
                expires_processed = true;
                if valid.is_none() {
                    match parse_http_timestamp(&header.value) {
                        Some(timestamp) if timestamp > now => {
                            valid = Some(Duration::from_secs(timestamp - now));
                        }
                        _ => expired = true,
                    }
                }
            }
            continue;
        }

        if header.name.eq_ignore_ascii_case("x-accel-expires") {
            x_accel_seen = true;
            if !policy.ignore_x_accel_expires && !x_accel_processed {
                x_accel_processed = true;
                match parse_accel_expires(&header.value, now) {
                    AccelExpires::Valid(validity) => {
                        valid = Some(validity);
                        no_cache = false;
                        expired = false;
                    }
                    AccelExpires::Clear => {
                        valid = None;
                        no_cache = false;
                        expired = false;
                    }
                    AccelExpires::Zero => cacheable = false,
                    AccelExpires::Ignored => {}
                }
            }
        }
    }

    CacheHeaders {
        valid: valid.filter(|valid| !valid.is_zero()),
        stale,
        cacheable: cacheable && !no_cache && !expired,
    }
}

fn apply_cache_control(
    value: &str,
    x_accel_wins: bool,
    valid: &mut Option<Duration>,
    cacheable: &mut bool,
    no_cache: &mut bool,
    expired: &mut bool,
    stale: &mut CacheStaleWindows,
) {
    if x_accel_wins {
        parse_stale_extensions(value, cacheable, stale);
        return;
    }

    let lower = value.to_ascii_lowercase();
    if lower.contains("no-cache") || lower.contains("no-store") || lower.contains("private") {
        *no_cache = true;
        return;
    }

    let max_age = lower
        .find("s-maxage=")
        .map(|index| index + "s-maxage=".len())
        .or_else(|| lower.find("max-age=").map(|index| index + "max-age=".len()));
    let Some(start) = max_age else {
        parse_stale_extensions(value, cacheable, stale);
        return;
    };
    match parse_delta_seconds(&lower[start..]) {
        Some(0) => *no_cache = true,
        Some(seconds) => {
            *valid = Some(Duration::from_secs(seconds));
            *expired = false;
            parse_stale_extensions(value, cacheable, stale);
        }
        None => *cacheable = false,
    }
}

fn parse_stale_extensions(value: &str, cacheable: &mut bool, stale: &mut CacheStaleWindows) {
    let lower = value.to_ascii_lowercase();
    if let Some(start) = lower.find("stale-while-revalidate=") {
        let Some(seconds) = parse_delta_seconds(&lower[start + "stale-while-revalidate=".len()..])
        else {
            *cacheable = false;
            return;
        };
        stale.updating = Duration::from_secs(seconds);
        stale.error = Duration::from_secs(seconds);
    }
    if let Some(start) = lower.find("stale-if-error=") {
        let Some(seconds) = parse_delta_seconds(&lower[start + "stale-if-error=".len()..]) else {
            *cacheable = false;
            return;
        };
        stale.error = Duration::from_secs(seconds);
    }
}

enum AccelExpires {
    Valid(Duration),
    Clear,
    Zero,
    Ignored,
}

fn parse_accel_expires(value: &str, now: u64) -> AccelExpires {
    let value = value.trim();
    if let Some(timestamp) = value.strip_prefix('@') {
        let Ok(timestamp) = timestamp.parse::<i64>() else {
            return AccelExpires::Ignored;
        };
        if timestamp < 0 {
            return AccelExpires::Ignored;
        }
        if timestamp == 0 {
            return AccelExpires::Clear;
        }
        let timestamp = timestamp as u64;
        return match timestamp.checked_sub(now) {
            Some(0) | None => AccelExpires::Zero,
            Some(seconds) => AccelExpires::Valid(Duration::from_secs(seconds)),
        };
    }

    match value.parse::<i64>() {
        Ok(0) => AccelExpires::Zero,
        Ok(seconds) if seconds > 0 => AccelExpires::Valid(Duration::from_secs(seconds as u64)),
        _ => AccelExpires::Ignored,
    }
}

fn parse_delta_seconds(value: &str) -> Option<u64> {
    let mut seconds = 0_u64;
    for byte in value.bytes() {
        if matches!(byte, b',' | b';' | b' ') {
            break;
        }
        if !byte.is_ascii_digit() {
            return None;
        }
        seconds = seconds
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'));
    }
    Some(seconds)
}

fn parse_http_timestamp(value: &str) -> Option<u64> {
    httpdate::parse_http_date(value.trim())
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

#[cfg(test)]
mod tests;
