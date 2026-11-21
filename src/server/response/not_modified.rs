use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::server) enum ConditionalResult {
    Full,
    NotModified,
    PreconditionFailed,
}

pub(in crate::server) fn evaluate_conditional_request(
    response: &Response,
    request: &Request,
    context: &EffectiveConfig,
) -> ConditionalResult {
    if response.status != 200 {
        return ConditionalResult::Full;
    }

    let last_modified = response.header("Last-Modified").and_then(http_timestamp);
    let etag = response.header("ETag");

    if let Some(value) = request.header("if-unmodified-since")
        && !test_if_unmodified(last_modified, value)
    {
        return ConditionalResult::PreconditionFailed;
    }
    if let Some(value) = request.header("if-match")
        && !matches_etag(etag, value, false)
    {
        return ConditionalResult::PreconditionFailed;
    }

    let if_modified_since = request.header("if-modified-since");
    let if_none_match = request.header("if-none-match");
    if if_modified_since.is_none() && if_none_match.is_none() {
        return ConditionalResult::Full;
    }
    if let Some(value) = if_modified_since
        && test_if_modified(last_modified, value, context.if_modified_since)
    {
        return ConditionalResult::Full;
    }
    if let Some(value) = if_none_match
        && !matches_etag(etag, value, true)
    {
        return ConditionalResult::Full;
    }
    ConditionalResult::NotModified
}

pub(in crate::server) fn finalize_not_modified(response: &mut Response) {
    response.status = 304;
    response.body = ResponseBody::Empty;
    for name in [
        "Content-Type",
        "Content-Length",
        "Content-Range",
        "Accept-Ranges",
        "Content-Encoding",
    ] {
        response.remove_header(name);
    }
}

fn test_if_unmodified(last_modified: Option<i64>, value: &str) -> bool {
    let Some(last_modified) = last_modified else {
        return false;
    };
    http_timestamp(value).unwrap_or(-1) >= last_modified
}

fn test_if_modified(last_modified: Option<i64>, value: &str, mode: IfModifiedSince) -> bool {
    let Some(last_modified) = last_modified else {
        return true;
    };
    if mode == IfModifiedSince::Off {
        return true;
    }
    let since = http_timestamp(value).unwrap_or(-1);
    if since == last_modified {
        return false;
    }
    mode == IfModifiedSince::Exact || since < last_modified
}

fn matches_etag(etag: Option<&str>, value: &str, weak: bool) -> bool {
    let value = value.trim();
    if value == "*" {
        return true;
    }
    let Some(etag) = etag else {
        return false;
    };
    let etag = if weak { strip_weak(etag) } else { etag };
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        let candidate = if weak {
            strip_weak(candidate)
        } else {
            candidate
        };
        candidate == etag
    })
}

fn strip_weak(value: &str) -> &str {
    value.strip_prefix("W/").unwrap_or(value)
}

fn http_timestamp(value: &str) -> Option<i64> {
    let time = httpdate::parse_http_date(value).ok()?;
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => i64::try_from(duration.as_secs()).ok(),
        Err(error) => i64::try_from(error.duration().as_secs())
            .ok()
            .and_then(i64::checked_neg),
    }
}
