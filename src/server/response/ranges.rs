use super::*;

mod body;
mod multipart;
mod parse;

use parse::{ByteRange, ParsedRanges, parse_ranges};

pub(in crate::server) fn apply_response_range(
    response: &mut Response,
    request: &Request,
    max_ranges: u64,
) {
    if response.status != 200 || !response.allow_ranges || max_ranges == 0 {
        return;
    }
    if response.body.is_stream() {
        return;
    }
    let content_length = request.method.eq_ignore_ascii_case("HEAD").then(|| {
        response
            .header("Content-Length")
            .and_then(|value| value.trim().parse::<u64>().ok())
    });
    let Some(length) = content_length
        .flatten()
        .or_else(|| response.body.content_length())
    else {
        return;
    };
    let range_requested = request.header("range").is_some();
    let ranges = if response.single_range { 1 } else { max_ranges };
    if let Some(ranged) = apply_range(response, request, length, ranges) {
        *response = ranged;
    }
    if response.status == 200 {
        if range_requested {
            response.remove_header("Content-Range");
        }
        response.set_header("Accept-Ranges", "bytes");
    }
}

pub(in crate::server) fn apply_range(
    response: &mut Response,
    request: &Request,
    length: u64,
    max_ranges: u64,
) -> Option<Response> {
    if max_ranges == 0 || !supports_byte_ranges(&request.version) {
        return None;
    }
    let value = request.header("range").and_then(range_spec)?;
    if let Some(if_range) = request.header("if-range")
        && !if_range_matches(response, if_range)
    {
        return None;
    }
    match parse_ranges(value, length, max_ranges) {
        ParsedRanges::Ignore => None,
        ParsedRanges::Unsatisfiable => {
            let mut invalid = error_response(416);
            invalid.set_header("Content-Range", format!("bytes */{length}"));
            Some(invalid)
        }
        ParsedRanges::Satisfiable(ranges) if ranges.len() == 1 => {
            apply_single_range(response, ranges[0], length)
        }
        ParsedRanges::Satisfiable(ranges) => multipart::apply(response, &ranges, length),
    }
}

fn apply_single_range(response: &mut Response, range: ByteRange, length: u64) -> Option<Response> {
    let range_length = range.end - range.start;
    response.status = 206;
    response.remove_header("Content-Range");
    response.set_header(
        "Content-Range",
        format!("bytes {}-{}/{length}", range.start, range.end - 1),
    );
    response.set_header("Content-Length", range_length.to_string());
    body::slice(&mut response.body, range.start, range_length);
    Some(std::mem::replace(response, Response::empty(200)))
}

fn supports_byte_ranges(version: &str) -> bool {
    version
        .strip_prefix("HTTP/")
        .and_then(|version| version.split('.').next())
        .and_then(|major| major.parse::<u16>().ok())
        .is_some_and(|major| major >= 1)
}

fn range_spec(value: &str) -> Option<&[u8]> {
    let bytes = value.as_bytes();
    (bytes.len() >= 7 && bytes[..6].eq_ignore_ascii_case(b"bytes=")).then_some(&bytes[6..])
}

fn if_range_matches(response: &Response, value: &str) -> bool {
    if value.len() >= 2 && value.ends_with('"') {
        return response.header("ETag") == Some(value);
    }
    let Some(last_modified) = response.header("Last-Modified").and_then(http_timestamp) else {
        return false;
    };
    http_timestamp(value) == Some(last_modified)
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
