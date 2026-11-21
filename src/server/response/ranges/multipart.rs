use super::super::*;
use super::ByteRange;
use crate::http::BodySegment;
use std::sync::atomic::{AtomicU64, Ordering};

static BOUNDARY_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn apply(
    response: &mut Response,
    ranges: &[ByteRange],
    length: u64,
) -> Option<Response> {
    let boundary = next_boundary();
    let content_type = response.header("Content-Type").map(str::to_string);
    let mut segments = Vec::with_capacity(ranges.len() + 1);
    let mut content_length = 0_u64;

    for range in ranges {
        let mut prefix = format!("\r\n--{boundary}\r\n");
        if let Some(content_type) = &content_type {
            prefix.push_str("Content-Type: ");
            prefix.push_str(content_type);
            prefix.push_str("\r\n");
        }
        prefix.push_str(&format!(
            "Content-Range: bytes {}-{}/{length}\r\n\r\n",
            range.start,
            range.end - 1
        ));
        content_length = content_length
            .saturating_add(prefix.len() as u64)
            .saturating_add(range.end - range.start);
        segments.push(BodySegment {
            prefix: prefix.into_bytes(),
            start: range.start,
            end: range.end,
        });
    }

    let final_boundary = format!("\r\n--{boundary}--\r\n");
    content_length = content_length.saturating_add(final_boundary.len() as u64);
    segments.push(BodySegment {
        prefix: final_boundary.into_bytes(),
        start: length,
        end: length,
    });

    response.status = 206;
    response.remove_header("Content-Range");
    response.set_header(
        "Content-Type",
        format!("multipart/byteranges; boundary={boundary}"),
    );
    response.set_header("Content-Length", content_length.to_string());
    let source = std::mem::replace(&mut response.body, ResponseBody::Empty);
    response.body = ResponseBody::Segments {
        source: Box::new(source),
        segments,
    };
    Some(std::mem::replace(response, Response::empty(200)))
}

fn next_boundary() -> String {
    let value = BOUNDARY_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{value:020}")
}
