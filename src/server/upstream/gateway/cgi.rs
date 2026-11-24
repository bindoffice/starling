use super::*;
use crate::config::UpstreamBufferingConfig;

#[cfg(test)]
pub(in crate::server) struct CgiResponseDecoder {
    head_decoder: CgiHeadDecoder,
    head: Option<CgiHead>,
    body: Option<BodyBuffer>,
    policy: UpstreamBodyBufferPolicy,
}

#[cfg(test)]
impl CgiResponseDecoder {
    pub(in crate::server) fn with_policy(
        max_header_bytes: usize,
        policy: UpstreamBodyBufferPolicy,
    ) -> Self {
        Self {
            head_decoder: CgiHeadDecoder::new(max_header_bytes),
            head: None,
            body: None,
            policy,
        }
    }

    pub(in crate::server) async fn push(&mut self, data: &[u8]) -> io::Result<()> {
        if self.head.is_some() {
            return self
                .body
                .as_mut()
                .expect("CGI body buffer initialized")
                .append(data)
                .await;
        }

        let Some(start) = self.head_decoder.push(data)? else {
            return Ok(());
        };
        let mut body = BodyBuffer::new(self.policy.options_for_headers(start.head.headers()));
        body.append(&start.body_prefix).await?;
        self.head = Some(start.head);
        self.body = Some(body);
        Ok(())
    }

    pub(in crate::server) async fn finish(self) -> io::Result<Response> {
        let Some(head) = self.head else {
            let start = self.head_decoder.finish()?;
            return start
                .head
                .into_response(ResponseBody::Bytes(start.body_prefix), None);
        };
        let buffered = self
            .body
            .expect("CGI body buffer initialized")
            .finish()
            .await?;
        head.into_response(buffered.body, buffered.temp_file)
    }
}

pub(in crate::server) struct CgiHeadDecoder {
    header: Vec<u8>,
    max_header_bytes: usize,
}

pub(in crate::server) fn cgi_header_limit(config: &UpstreamBufferingConfig) -> usize {
    config.buffer_size.max(config.total_buffers_size())
}

pub(in crate::server) struct CgiHeadStart {
    pub(in crate::server) head: CgiHead,
    pub(in crate::server) body_prefix: Vec<u8>,
}

impl CgiHeadDecoder {
    pub(in crate::server) fn new(max_header_bytes: usize) -> Self {
        Self {
            header: Vec::new(),
            max_header_bytes,
        }
    }

    pub(in crate::server) fn push(&mut self, data: &[u8]) -> io::Result<Option<CgiHeadStart>> {
        self.header.extend_from_slice(data);
        let Some((header_end, separator_length)) = cgi_separator(&self.header) else {
            if self.header.len() > self.max_header_bytes {
                return Err(cgi_header_too_large());
            }
            return Ok(None);
        };
        if header_end.saturating_add(separator_length) > self.max_header_bytes {
            return Err(cgi_header_too_large());
        }

        let bytes = std::mem::take(&mut self.header);
        let head = parse_cgi_head(&bytes[..header_end])?;
        Ok(Some(CgiHeadStart {
            head,
            body_prefix: bytes[header_end + separator_length..].to_vec(),
        }))
    }

    pub(in crate::server) fn finish(self) -> io::Result<CgiHeadStart> {
        Err(invalid_cgi("CGI response ended before headers"))
    }
}

#[cfg(test)]
pub(in crate::server) fn parse_cgi_response(
    stdout: Vec<u8>,
    max_header_bytes: usize,
) -> io::Result<Response> {
    let Some((header_end, separator_length)) = cgi_separator(&stdout) else {
        return Err(invalid_cgi("CGI response ended before headers"));
    };
    if header_end.saturating_add(separator_length) > max_header_bytes {
        return Err(cgi_header_too_large());
    }
    let head = parse_cgi_head(&stdout[..header_end])?;
    let body = stdout[header_end + separator_length..].to_vec();
    head.into_response(ResponseBody::Bytes(body), None)
}

pub(in crate::server) struct CgiHead {
    status: u16,
    headers: Vec<Header>,
    content_length: Option<u64>,
}

pub(in crate::server) fn truncate_prefix_to_content_length(
    prefix: &mut Vec<u8>,
    length: Option<u64>,
) {
    let Some(length) = length.and_then(|length| usize::try_from(length).ok()) else {
        return;
    };
    if prefix.len() > length {
        prefix.truncate(length);
    }
}

impl CgiHead {
    pub(in crate::server) fn status(&self) -> u16 {
        self.status
    }

    pub(in crate::server) fn headers(&self) -> &[Header] {
        &self.headers
    }

    pub(in crate::server) fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub(in crate::server) fn into_response(
        self,
        body: ResponseBody,
        body_temp_file: Option<Arc<TempFileGuard>>,
    ) -> io::Result<Response> {
        let mut response = Response {
            status: self.status,
            headers: self.headers,
            trailers: Vec::new(),
            chunked_expected: false,
            body,
            body_temp_file,
            upstream: None,
            builtin_error: false,
            msie_padding: false,
            allow_ranges: false,
            single_range: false,
        };
        response.set_content_length();
        Ok(response)
    }
}

fn parse_cgi_head(bytes: &[u8]) -> io::Result<CgiHead> {
    let headers_text =
        std::str::from_utf8(bytes).map_err(|_| invalid_cgi("invalid CGI response headers"))?;
    let mut status = None;
    let mut content_type = None;
    let mut headers = Vec::new();
    let mut has_location = false;
    let lines: Vec<_> = headers_text.lines().collect();
    let mut start = 0;
    if let Some(first) = lines.first()
        && let Some(version) = first.trim_end_matches('\r').strip_prefix("HTTP/")
    {
        status = Some(
            version
                .split_whitespace()
                .nth(1)
                .and_then(parse_cgi_status)
                .ok_or_else(|| invalid_cgi("invalid CGI response status line"))?,
        );
        start = 1;
    }
    for line in &lines[start..] {
        let line = line.trim_end_matches('\r');
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().to_string();
        if name.eq_ignore_ascii_case("status") {
            status = value.split_whitespace().next().and_then(parse_cgi_status);
            if status.is_none() {
                return Err(invalid_cgi("invalid CGI response status"));
            }
        } else if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.clone());
            headers.push(Header {
                name: name.to_string(),
                value,
            });
        } else if !matches!(
            name.to_ascii_lowercase().as_str(),
            "connection" | "keep-alive" | "transfer-encoding"
        ) {
            has_location |= name.eq_ignore_ascii_case("location");
            headers.push(Header {
                name: name.to_string(),
                value,
            });
        }
    }
    if content_type.is_none() {
        headers.push(Header {
            name: "Content-Type".into(),
            value: "text/html".into(),
        });
    }
    Ok(CgiHead {
        status: status.unwrap_or(if has_location { 302 } else { 200 }),
        content_length: parse_content_length(&headers),
        headers,
    })
}

fn parse_cgi_status(value: &str) -> Option<u16> {
    let bytes = value.as_bytes();
    if bytes.len() < 3 || !bytes[..3].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let status = value[..3].parse::<u16>().ok()?;
    (100..=999).contains(&status).then_some(status)
}

fn cgi_separator(bytes: &[u8]) -> Option<(usize, usize)> {
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| {
            bytes
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
        })
}

fn cgi_header_too_large() -> io::Error {
    invalid_cgi("CGI response header too large")
}

fn invalid_cgi(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn parse_content_length(headers: &[Header]) -> Option<u64> {
    let mut length = None;
    for header in headers {
        if !header.name.eq_ignore_ascii_case("content-length") {
            continue;
        }
        let value = header.value.trim().parse::<u64>().ok()?;
        if length.is_some_and(|length| length != value) {
            return None;
        }
        length = Some(value);
    }
    length
}
