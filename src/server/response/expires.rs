use super::*;

pub(in crate::server) fn apply_expires(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    let Some(rule) = &context.expires else {
        return;
    };
    if !matches!(
        response.status,
        200 | 201 | 204 | 206 | 301 | 302 | 303 | 304 | 307 | 308
    ) {
        return;
    }
    let Some(value) = resolve_value(&rule.value, variables) else {
        return;
    };
    let now = unix_timestamp(SystemTime::now());
    match value {
        ExpiresValue::Off => {}
        ExpiresValue::Epoch => set_fixed(response, "Thu, 01 Jan 1970 00:00:01 GMT", "no-cache"),
        ExpiresValue::Max => set_fixed(
            response,
            "Thu, 31 Dec 2037 23:55:55 GMT",
            "max-age=315360000",
        ),
        ExpiresValue::Access(ExpiresOffset::Fixed(seconds)) => {
            if seconds == 0 {
                set_expiration(response, now, 0);
            } else if let Some(expires) = now.checked_add(seconds) {
                set_expiration(response, expires, seconds);
            }
        }
        ExpiresValue::Access(ExpiresOffset::Daily(seconds)) => {
            if let Some(expires) = next_daily_timestamp(seconds) {
                set_expiration(response, expires, expires.saturating_sub(now));
            }
        }
        ExpiresValue::Modified(ExpiresOffset::Fixed(seconds)) => {
            if seconds == 0 {
                set_expiration(response, now, 0);
                return;
            }
            let last_modified = response
                .header("Last-Modified")
                .and_then(|value| httpdate::parse_http_date(value).ok())
                .map(unix_timestamp)
                .unwrap_or(now);
            if let Some(expires) = last_modified.checked_add(seconds) {
                set_expiration(response, expires, expires.saturating_sub(now));
            }
        }
        ExpiresValue::Access(ExpiresOffset::Variable(_))
        | ExpiresValue::Modified(ExpiresOffset::Variable(_))
        | ExpiresValue::Modified(ExpiresOffset::Daily(_)) => {}
    }
}

fn resolve_value(value: &ExpiresValue, variables: &Variables) -> Option<ExpiresValue> {
    match value {
        ExpiresValue::Access(ExpiresOffset::Variable(expression)) => {
            let value = eval_variables(expression, variables);
            ExpiresValue::parse_literal(&value, false)
        }
        ExpiresValue::Modified(ExpiresOffset::Variable(expression)) => {
            let value = eval_variables(expression, variables);
            ExpiresValue::parse_literal(&value, true)
        }
        value => Some(value.clone()),
    }
}

fn set_fixed(response: &mut Response, expires: &str, cache_control: &str) {
    response.set_header("Expires", expires);
    response.remove_header("Cache-Control");
    response.set_header("Cache-Control", cache_control);
}

fn set_expiration(response: &mut Response, timestamp: i64, max_age: i64) {
    let Some(time) = system_time(timestamp) else {
        return;
    };
    response.set_header("Expires", httpdate::fmt_http_date(time));
    response.remove_header("Cache-Control");
    if max_age < 0 {
        response.set_header("Cache-Control", "no-cache");
    } else {
        response.set_header("Cache-Control", format!("max-age={max_age}"));
    }
}

fn next_daily_timestamp(seconds: u32) -> Option<i64> {
    let now = unix_timestamp(SystemTime::now());
    let now_time = libc::time_t::try_from(now).ok()?;
    // SAFETY: libc::tm consists of integer and pointer fields, so its all-zero
    // representation is valid.
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers reference live, correctly initialized storage for
    // the duration of the call; the null result is checked.
    if unsafe { libc::localtime_r(&now_time, &mut value) }.is_null() {
        return None;
    }
    value.tm_hour = (seconds / 3600) as libc::c_int;
    value.tm_min = ((seconds % 3600) / 60) as libc::c_int;
    value.tm_sec = (seconds % 60) as libc::c_int;
    // SAFETY: value is a live, initialized libc::tm populated by localtime_r.
    let mut next = unsafe { libc::mktime(&mut value) };
    if next == -1 {
        return None;
    }
    if next <= now_time {
        value.tm_mday += 1;
        // SAFETY: value remains a live, initialized libc::tm after normalization.
        next = unsafe { libc::mktime(&mut value) };
    }
    if next == -1 {
        return None;
    }
    // Keep this conversion for platforms where libc::time_t is not i64.
    #[allow(clippy::useless_conversion)]
    i64::try_from(next).ok()
}

fn unix_timestamp(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        Err(error) => -i64::try_from(error.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn system_time(timestamp: i64) -> Option<SystemTime> {
    if timestamp >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(timestamp as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(timestamp.unsigned_abs()))
    }
}
