use super::*;
use std::io::Read as _;
use std::net::Ipv6Addr;

pub fn file_response(path: &Path, content_type: String) -> io::Result<Response> {
    let file = std::fs::File::open(path)?;
    let metadata = file.metadata()?;
    file_response_with_metadata(file, &metadata, content_type)
}

pub(crate) fn file_response_with_metadata(
    mut file: std::fs::File,
    metadata: &std::fs::Metadata,
    content_type: String,
) -> io::Result<Response> {
    const SMALL_FILE_MEMORY_LIMIT: u64 = 64 * 1024;

    let modified = metadata.modified().ok();
    let body = if metadata.len() <= SMALL_FILE_MEMORY_LIMIT {
        let mut body = Vec::with_capacity(metadata.len() as usize);
        file.read_to_end(&mut body)?;
        ResponseBody::Bytes(body)
    } else {
        ResponseBody::File {
            file: File::from_std(file),
            offset: 0,
            length: metadata.len(),
        }
    };
    let mut response = Response {
        status: 200,
        headers: vec![
            Header {
                name: "Content-Type".into(),
                value: content_type,
            },
            Header {
                name: "Content-Length".into(),
                value: body.len().to_string(),
            },
        ],
        trailers: Vec::new(),
        chunked_expected: false,
        body,
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
        allow_ranges: false,
        single_range: false,
    };
    response.enable_ranges(false);
    if let Some(modified) = modified {
        response.add_header("Last-Modified", httpdate::fmt_http_date(modified));
    }
    Ok(response)
}

pub(in crate::http) fn is_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

pub(crate) fn valid_host(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if let Some(value) = value.strip_prefix('[') {
        let Some((literal, rest)) = value.split_once(']') else {
            return false;
        };
        if !valid_ip_literal(literal) {
            return false;
        }
        return rest.is_empty()
            || rest
                .strip_prefix(':')
                .is_some_and(|port| port.is_empty() || valid_port(port));
    }

    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (value, None),
    };
    if host.contains("..") {
        return false;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host.contains(':')
        || host.contains("..")
        || host.ends_with('.')
        || host.starts_with('.')
        || !host.bytes().all(valid_host_byte)
    {
        return false;
    }
    port.is_none_or(|port| port.is_empty() || valid_port(port))
}

fn valid_ip_literal(literal: &str) -> bool {
    if literal.is_empty() {
        return false;
    }
    if literal
        .as_bytes()
        .first()
        .is_some_and(|byte| matches!(byte, b'v' | b'V'))
    {
        return valid_ipvfuture(literal);
    }
    literal.parse::<Ipv6Addr>().is_ok()
}

fn valid_ipvfuture(literal: &str) -> bool {
    let Some((version, address)) = literal[1..].split_once('.') else {
        return false;
    };
    !version.is_empty()
        && version.bytes().all(|byte| byte.is_ascii_hexdigit())
        && !address.is_empty()
        && address.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'.'
                        | b'_'
                        | b'~'
                        | b'!'
                        | b'$'
                        | b'&'
                        | b'\''
                        | b'('
                        | b')'
                        | b'*'
                        | b'+'
                        | b','
                        | b';'
                        | b'='
                        | b':'
                )
        })
}

pub(in crate::http) fn valid_host_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b'%'
        )
}

pub(in crate::http) fn valid_port(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u16>().is_ok()
}

pub(in crate::http) fn invalid_data(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(in crate::http) fn unsupported(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, message)
}
