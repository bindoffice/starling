use super::*;

pub(in crate::server) fn capture_upstream_control(response: &mut Response, ignore: &[String]) {
    let headers = response.headers.clone();
    let trailers = response.trailers.clone();
    let ignored = |name: &str| upstream_header_ignored(ignore, name);

    let redirect = (!ignored("X-Accel-Redirect"))
        .then(|| response.header("X-Accel-Redirect"))
        .flatten()
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    if !ignored("X-Accel-Redirect") {
        response.remove_header("X-Accel-Redirect");
    }

    let limit_rate = (!ignored("X-Accel-Limit-Rate"))
        .then(|| first_header_value(response, "X-Accel-Limit-Rate"))
        .flatten()
        .and_then(parse_accel_limit_rate);
    let buffering = (!ignored("X-Accel-Buffering"))
        .then(|| parse_accel_buffering(&response.headers))
        .flatten();
    let charset = (!ignored("X-Accel-Charset"))
        .then(|| last_header_value(response, "X-Accel-Charset"))
        .flatten()
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let control = response.upstream_control_mut();
    control.redirect = redirect;
    control.limit_rate = limit_rate;
    control.buffering = buffering;
    control.charset = charset;
    control.headers = headers;
    control.trailers = trailers;
}

pub(in crate::server) fn ignores_accel_buffering(ignore: &[String]) -> bool {
    upstream_header_ignored(ignore, "X-Accel-Buffering")
}

pub(in crate::server) fn upstream_header_ignored(ignore: &[String], name: &str) -> bool {
    ignore
        .iter()
        .any(|ignored| ignored.eq_ignore_ascii_case(name))
}

fn first_header_value<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn last_header_value<'a>(response: &'a Response, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .rev()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn parse_accel_limit_rate(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

pub(in crate::server) fn response_rate_with_upstream(
    mut rate: ResponseRate,
    response: &Response,
) -> ResponseRate {
    if let Some(upstream) = response
        .upstream_control()
        .and_then(|control| control.limit_rate)
    {
        rate.rate = upstream;
    }
    rate
}

#[cfg(test)]
pub(in crate::server) fn apply_upstream_charset(response: &mut Response) {
    let Some(charset) = response
        .upstream_control()
        .and_then(|control| control.charset.clone())
    else {
        return;
    };
    let Some(content_type) = response.header("Content-Type").map(str::to_string) else {
        return;
    };
    let mime_type = content_type.split(';').next().unwrap_or_default().trim();
    response.set_header("Content-Type", format!("{mime_type}; charset={charset}"));
}
