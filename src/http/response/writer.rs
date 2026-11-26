use super::*;

mod body;

/// Inline bodies up to this size into the head buffer so a tiny response leaves
/// in a single write. Larger bodies stay zero-copy and keep the split write.
const INLINE_BODY_LIMIT: usize = 16 * 1024;

pub struct ResponseWriteOptions<'a> {
    pub request_method: &'a str,
    pub request_version: &'a str,
    pub keep_alive: bool,
    pub keep_alive_header: Option<u64>,
    pub server_tokens: ServerTokens,
    pub chunked_transfer_encoding: bool,
    pub rate: ResponseRate,
}

pub async fn write_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &mut Response,
    options: ResponseWriteOptions<'_>,
) -> io::Result<()> {
    let chunked = prepare_response_for_write(response, &options);
    write_prepared_response(writer, response, options, chunked).await
}

pub(crate) async fn write_prepared_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &mut Response,
    options: ResponseWriteOptions<'_>,
    chunked: bool,
) -> io::Result<()> {
    let mut scratch = Vec::new();
    write_prepared_response_with_scratch(writer, response, options, chunked, &mut scratch).await
}

pub(crate) async fn write_prepared_response_with_scratch<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &mut Response,
    options: ResponseWriteOptions<'_>,
    chunked: bool,
    scratch: &mut Vec<u8>,
) -> io::Result<()> {
    write_early_hints(writer, response.early_hints(), options.request_version).await?;
    build_response_head_into(response, scratch);

    // A small in-memory body can ride along with the head in one write instead
    // of a second syscall. Only when unframed, unrated and not a HEAD request.
    if !chunked && options.rate.rate == 0 && !options.request_method.eq_ignore_ascii_case("HEAD") {
        let inlinable = matches!(
            &response.body,
            ResponseBody::Bytes(body) if body.len() <= INLINE_BODY_LIMIT
        );
        if inlinable {
            if let ResponseBody::Bytes(body) =
                std::mem::replace(&mut response.body, ResponseBody::Empty)
            {
                scratch.extend_from_slice(&body);
            }
            writer.write_all(scratch).await?;
            writer.flush().await?;
            return Ok(());
        }
    }

    writer.write_all(scratch).await?;

    if options.request_method.eq_ignore_ascii_case("HEAD") {
        writer.flush().await?;
        return Ok(());
    }
    if chunked {
        let mut trailers =
            body::write_chunked_body(writer, options.rate, &mut response.body).await?;
        trailers.extend(response.trailers.iter().cloned());
        body::write_chunked_trailers(writer, &trailers).await?;
        writer.flush().await?;
        return Ok(());
    }
    body::write_body(writer, options.rate, &mut response.body).await?;
    writer.flush().await?;
    Ok(())
}

pub(crate) async fn write_prepared_response_head<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &Response,
    request_version: &str,
    scratch: &mut Vec<u8>,
) -> io::Result<()> {
    write_early_hints(writer, response.early_hints(), request_version).await?;
    build_response_head_into(response, scratch);
    writer.write_all(scratch).await?;
    writer.flush().await
}

pub(crate) fn prepare_response_for_write(
    response: &mut Response,
    options: &ResponseWriteOptions<'_>,
) -> bool {
    prepare_response_headers(response, options.server_tokens);
    let chunked = response_uses_chunked_encoding(
        response,
        options.request_method,
        options.request_version,
        options.chunked_transfer_encoding,
    );
    if chunked {
        response.remove_header("Content-Length");
        response.set_header_static("Transfer-Encoding", "chunked");
    }
    if response.status == 101 {
        response.remove_header("Content-Length");
        response.remove_header("Transfer-Encoding");
        response.remove_header("Keep-Alive");
        response.set_header_static("Connection", "upgrade");
        return false;
    }
    if options.request_version == "HTTP/1.0" && !options.keep_alive {
        response.set_header_static("Connection", "close");
    } else if options.keep_alive {
        response.set_header_static("Connection", "keep-alive");
    } else {
        response.set_header_static("Connection", "close");
    }
    if options.keep_alive {
        if let Some(timeout) = options.keep_alive_header {
            response.set_header_static("Keep-Alive", format!("timeout={timeout}"));
        } else {
            response.remove_header("Keep-Alive");
        }
    } else {
        response.remove_header("Keep-Alive");
    }

    chunked
}

pub(crate) fn response_head_length(response: &Response) -> u64 {
    let mut length = b"HTTP/1.1 ".len()
        + decimal_length(response.status)
        + 1
        + reason_phrase(response.status).len()
        + 2;
    for header in &response.headers {
        length = length
            .saturating_add(header.name.len())
            .saturating_add(2)
            .saturating_add(header_value_bytes(&header.value).len())
            .saturating_add(2);
    }
    length.saturating_add(2) as u64
}

fn decimal_length(mut value: u16) -> usize {
    let mut digits = 1;
    while value >= 10 {
        value /= 10;
        digits += 1;
    }
    digits
}

fn build_response_head_into(response: &Response, head: &mut Vec<u8>) {
    head.clear();
    head.reserve(response.headers.len().saturating_mul(32).saturating_add(64));
    head.extend_from_slice(b"HTTP/1.1 ");
    extend_decimal(head, response.status);
    head.push(b' ');
    head.extend_from_slice(reason_phrase(response.status).as_bytes());
    head.extend_from_slice(b"\r\n");
    for header in &response.headers {
        head.extend_from_slice(header.name.as_bytes());
        head.extend_from_slice(b": ");
        extend_sanitized_header_value(&mut *head, &header_value_bytes(&header.value));
        head.extend_from_slice(b"\r\n");
    }
    head.extend_from_slice(b"\r\n");
}

fn extend_decimal(output: &mut Vec<u8>, mut value: u16) {
    let mut digits = [0; 5];
    let mut index = digits.len();
    loop {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    output.extend_from_slice(&digits[index..]);
}

fn extend_sanitized_header_value(output: &mut Vec<u8>, value: &[u8]) {
    if !value.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
        output.extend_from_slice(value);
        return;
    }
    output.extend(value.iter().map(|byte| match byte {
        b'\r' | b'\n' => b' ',
        _ => *byte,
    }));
}

pub(crate) fn response_uses_chunked_encoding(
    response: &Response,
    request_method: &str,
    request_version: &str,
    chunked_transfer_encoding: bool,
) -> bool {
    request_version == "HTTP/1.1"
        && chunked_transfer_encoding
        && !request_method.eq_ignore_ascii_case("HEAD")
        && response.status >= 200
        && !matches!(response.status, 204 | 304)
        && (response.chunked_expected
            || !response.trailers.is_empty()
            || response.body.content_length().is_none()
            || response.body.stream_may_have_trailers())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_header_values_without_allocating_when_no_sanitizing_is_needed() {
        let mut output = b"Header: ".to_vec();

        extend_sanitized_header_value(&mut output, b"safe value");

        assert_eq!(output, b"Header: safe value");
    }

    #[test]
    fn replaces_line_breaks_in_header_values_with_spaces() {
        let mut output = Vec::new();

        extend_sanitized_header_value(&mut output, b"first\r\nsecond\nthird");

        assert_eq!(output, b"first  second third");
    }

    #[test]
    fn builds_the_response_head_without_generic_formatting() {
        let mut response = Response::empty(204);
        response.set_header("Date", "Thu, 01 Jan 1970 00:00:00 GMT");
        let mut output = Vec::new();

        build_response_head_into(&response, &mut output);

        assert_eq!(
            output,
            b"HTTP/1.1 204 No Content\r\n\
              Content-Length: 0\r\n\
              Date: Thu, 01 Jan 1970 00:00:00 GMT\r\n\
              \r\n"
        );
    }
}
