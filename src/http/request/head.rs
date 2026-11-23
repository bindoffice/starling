use super::*;

#[derive(Debug, Clone, Copy)]
pub struct RequestHeadLimits {
    pub max_line_bytes: usize,
    pub max_total_bytes: usize,
}

impl Default for RequestHeadLimits {
    fn default() -> Self {
        Self {
            max_line_bytes: MAX_HEADER_BYTES,
            max_total_bytes: MAX_HEADER_BYTES,
        }
    }
}

impl RequestHeadLimits {
    pub fn from_buffers(
        client_header_buffer_size: usize,
        large_buffer_count: usize,
        large_buffer_size: usize,
    ) -> Self {
        Self {
            max_line_bytes: large_buffer_size,
            max_total_bytes: large_buffer_count
                .saturating_mul(large_buffer_size)
                .saturating_add(client_header_buffer_size),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum InvalidRequestHeader {
    Raw,
    Underscore(usize),
}

impl RequestHead {
    pub(crate) fn apply_header_policy(
        &mut self,
        ignore_invalid_headers: bool,
        underscores_in_headers: bool,
    ) -> io::Result<()> {
        for invalid in &self.invalid_headers {
            match invalid {
                InvalidRequestHeader::Raw if !ignore_invalid_headers => {
                    return Err(invalid_data("invalid header"));
                }
                _ => {}
            }
        }

        if ignore_invalid_headers && !underscores_in_headers {
            let invalid_indices: HashSet<usize> = self
                .invalid_headers
                .iter()
                .filter_map(|invalid| match invalid {
                    InvalidRequestHeader::Underscore(index) => Some(*index),
                    InvalidRequestHeader::Raw => None,
                })
                .collect();
            self.headers = std::mem::take(&mut self.headers)
                .into_iter()
                .enumerate()
                .filter_map(|(index, header)| (!invalid_indices.contains(&index)).then_some(header))
                .collect();
        }
        self.invalid_headers.clear();
        Ok(())
    }
}

pub async fn read_request_head<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
) -> io::Result<Option<RequestHead>> {
    let Some(mut head) =
        read_request_head_with_limits(reader, RequestHeadLimits::default()).await?
    else {
        return Ok(None);
    };
    head.apply_header_policy(false, false)?;
    Ok(Some(head))
}

pub async fn read_request_head_with_limits<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    limits: RequestHeadLimits,
) -> io::Result<Option<RequestHead>> {
    let request_line = loop {
        let Some(request_line) =
            read_line_limited(reader, limits.max_line_bytes, "request line too long").await?
        else {
            return Ok(None);
        };
        if request_line == "\r\n" || request_line == "\n" {
            continue;
        }
        break request_line;
    };
    let request_line = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| invalid_data("invalid request line"))?;
    let target = parts
        .next()
        .ok_or_else(|| invalid_data("invalid request line"))?;
    let version = parts
        .next()
        .ok_or_else(|| invalid_data("invalid request line"))?;
    if parts.next().is_some() {
        return Err(invalid_data("invalid request line"));
    }
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Err(invalid_data("unsupported HTTP version"));
    }
    let valid_target = if method == "CONNECT" {
        parse_connect_authority(target).is_some()
    } else if let Some(authority) = absolute_target_authority(target) {
        valid_host(authority)
    } else {
        target.starts_with('/')
    };
    if !method.bytes().all(is_token_byte)
        || !valid_target
        || target.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(invalid_data("invalid request line"));
    }

    let mut headers = Vec::new();
    let mut invalid_headers = Vec::new();
    let mut total = request_line.len() + 2;
    loop {
        let Some(line) = read_line_limited(
            reader,
            limits.max_line_bytes,
            "request header line too long",
        )
        .await?
        else {
            return Err(invalid_data("client closed connection prematurely"));
        };
        total += line.len();
        if total > limits.max_total_bytes {
            return Err(invalid_data("request headers too large"));
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.starts_with([' ', '\t']) {
            invalid_headers.push(InvalidRequestHeader::Raw);
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            invalid_headers.push(InvalidRequestHeader::Raw);
            continue;
        };
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            invalid_headers.push(InvalidRequestHeader::Raw);
            continue;
        }
        let index = headers.len();
        headers.push(Header {
            name: name.to_string(),
            value: value.trim().to_string(),
        });
        if name.as_bytes().contains(&b'_') {
            invalid_headers.push(InvalidRequestHeader::Underscore(index));
        }
    }

    let mut content_length = None;
    let mut content_length_seen = false;
    let mut chunked = false;
    let mut transfer_encoding_seen = false;
    let mut connection_values = Vec::new();
    let mut expect_continue = false;
    let mut expect_seen = false;
    for header in &headers {
        if header.name.eq_ignore_ascii_case("content-length") {
            if content_length_seen {
                return Err(invalid_data("duplicate Content-Length header"));
            }
            content_length_seen = true;
            let parsed = header
                .value
                .parse::<u64>()
                .map_err(|_| invalid_data("invalid Content-Length"))?;
            content_length = Some(parsed);
        } else if header.name.eq_ignore_ascii_case("transfer-encoding") {
            if transfer_encoding_seen {
                return Err(invalid_data("duplicate Transfer-Encoding header"));
            }
            transfer_encoding_seen = true;
            if version == "HTTP/1.0" {
                return Err(invalid_data("Transfer-Encoding is not allowed in HTTP/1.0"));
            }
            let codings: Vec<_> = header
                .value
                .split(',')
                .map(|value| value.trim().to_ascii_lowercase())
                .collect();
            if codings.as_slice() == ["chunked"] {
                chunked = true;
            } else {
                return Err(invalid_data("unsupported transfer encoding"));
            }
        } else if header.name.eq_ignore_ascii_case("connection") {
            connection_values.push(header.value.to_ascii_lowercase());
        } else if header.name.eq_ignore_ascii_case("expect") {
            if expect_seen {
                return Err(invalid_data("duplicate Expect header"));
            }
            expect_seen = true;
            if header.value.eq_ignore_ascii_case("100-continue") {
                expect_continue = true;
            }
        }
    }
    if chunked && content_length.is_some() {
        return Err(invalid_data(
            "both Content-Length and Transfer-Encoding are present",
        ));
    }

    let connection_tokens: HashSet<&str> = connection_values
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    let keep_alive = if version == "HTTP/1.1" {
        !connection_tokens.contains("close")
    } else {
        connection_tokens.contains("keep-alive")
    };
    let host_headers = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("host"))
        .count();
    if version == "HTTP/1.1" && host_headers == 0 {
        return Err(invalid_data("missing Host header"));
    }
    if host_headers > 1 {
        return Err(invalid_data("duplicate Host header"));
    }
    if let Some(host) = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("host"))
        && !valid_host(&host.value)
    {
        return Err(invalid_data("invalid Host header"));
    }

    Ok(Some(RequestHead {
        method: method.to_string(),
        target: target.to_string(),
        version: version.to_string(),
        headers,
        invalid_headers,
        keep_alive,
        content_length,
        chunked,
        expect_continue,
    }))
}

pub(crate) fn parse_connect_authority(value: &str) -> Option<(&str, u16)> {
    if let Some(rest) = value.strip_prefix('[') {
        let close = rest.find(']')?;
        let literal = &rest[..close];
        if literal.is_empty()
            || !literal
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
        {
            return None;
        }
        let port = rest[close + 1..].strip_prefix(':')?;
        if !valid_port(port) {
            return None;
        }
        return Some((&value[..close + 2], port.parse().ok()?));
    }

    let (host, port) = value.rsplit_once(':')?;
    if host.is_empty() || host.contains(':') || !valid_host(host) || !valid_port(port) {
        return None;
    }
    Some((host, port.parse().ok()?))
}

async fn read_line_limited<R: AsyncBufReadExt + Unpin>(
    reader: &mut R,
    limit: usize,
    too_long: &'static str,
) -> io::Result<Option<String>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return String::from_utf8(line)
                .map(Some)
                .map_err(|_| invalid_data("request header is not valid UTF-8"));
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |position| position + 1);
        if line.len().saturating_add(consumed) > limit {
            return Err(invalid_data(too_long));
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            return String::from_utf8(line)
                .map(Some)
                .map_err(|_| invalid_data("request header is not valid UTF-8"));
        }
    }
}
