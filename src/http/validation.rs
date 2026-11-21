use super::*;

pub fn file_response(path: PathBuf, content_type: String, length: u64) -> io::Result<Response> {
    let file = std::fs::File::open(&path)?;
    let modified = file.metadata()?.modified().ok();
    let mut response = Response {
        status: 200,
        headers: vec![
            Header {
                name: "Content-Type".into(),
                value: content_type,
            },
            Header {
                name: "Content-Length".into(),
                value: length.to_string(),
            },
            Header {
                name: "Accept-Ranges".into(),
                value: "bytes".into(),
            },
        ],
        trailers: Vec::new(),
        body: ResponseBody::File {
            file: File::from_std(file),
            offset: 0,
            length,
        },
        body_temp_file: None,
        upstream: None,
        builtin_error: false,
        msie_padding: false,
    };
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

pub(in crate::http) fn valid_host(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if let Some(value) = value.strip_prefix('[') {
        let Some((literal, rest)) = value.split_once(']') else {
            return false;
        };
        if literal.is_empty()
            || !literal
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || matches!(byte, b':' | b'.'))
        {
            return false;
        }
        return rest.is_empty() || rest.strip_prefix(':').is_some_and(valid_port);
    }

    let (host, port) = match value.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (value, None),
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    if host.is_empty()
        || host.contains(':')
        || host.contains("..")
        || host.starts_with('.')
        || !host.bytes().all(valid_host_byte)
    {
        return false;
    }
    port.is_none_or(valid_port)
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
