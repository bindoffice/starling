use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamBodyKind {
    None,
    ContentLength(u64),
    Chunked,
    UntilEof,
}

#[derive(Debug)]
pub struct UpstreamResponseHead {
    pub status: u16,
    pub version: String,
    pub headers: Vec<Header>,
    pub early_hints: Vec<Vec<Header>>,
    pub body_kind: UpstreamBodyKind,
    pub header_time: Duration,
}

pub async fn read_upstream_response_head<R: AsyncBufReadExt + AsyncReadExt + Unpin>(
    reader: &mut R,
    max_header_bytes: usize,
) -> io::Result<UpstreamResponseHead> {
    let started = Instant::now();
    let mut early_hints = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upstream closed connection without a response",
            ));
        }
        let mut header_bytes = line.len();
        if header_bytes > max_header_bytes {
            return Err(invalid_data("upstream response header too large"));
        }
        let mut parts = line.trim_end_matches(['\r', '\n']).splitn(3, ' ');
        let version = parts
            .next()
            .filter(|value| value.starts_with("HTTP/1."))
            .ok_or_else(|| invalid_data("invalid upstream status line"))?
            .to_string();
        let status = parts
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| invalid_data("invalid upstream status line"))?;
        let mut headers = Vec::with_capacity(12);
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                return Err(invalid_data("upstream closed response headers"));
            }
            header_bytes = header_bytes.saturating_add(line.len());
            if header_bytes > max_header_bytes {
                return Err(invalid_data("upstream response header too large"));
            }
            if line == "\r\n" || line == "\n" {
                break;
            }
            let line = line.trim_end_matches(['\r', '\n']);
            let Some((name, value)) = line.split_once(':') else {
                return Err(invalid_data("invalid upstream header"));
            };
            headers.push(Header {
                name: name.into(),
                value: value.trim().to_string(),
            });
        }
        if (100..200).contains(&status) && status != 101 {
            if status == 103 {
                early_hints.push(headers);
            }
            continue;
        }
        let body_kind = upstream_body_kind(status, &headers);
        return Ok(UpstreamResponseHead {
            status,
            version,
            headers,
            early_hints,
            body_kind,
            header_time: started.elapsed(),
        });
    }
}

pub fn upstream_body_kind(status: u16, headers: &[Header]) -> UpstreamBodyKind {
    if status == 204 || status == 304 || (100..200).contains(&status) {
        return UpstreamBodyKind::None;
    }
    let chunked = headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case("transfer-encoding")
            && header
                .value
                .split(',')
                .next_back()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("chunked"))
    });
    if chunked {
        return UpstreamBodyKind::Chunked;
    }
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-length"))
        .and_then(|header| header.value.parse::<u64>().ok())
        .map(UpstreamBodyKind::ContentLength)
        .unwrap_or(UpstreamBodyKind::UntilEof)
}
