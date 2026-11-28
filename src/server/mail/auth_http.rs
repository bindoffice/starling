use super::*;

const MAX_AUTH_HTTP_RESPONSE: usize = 64 * 1024;

pub(super) struct AuthHttpRequest<'a> {
    pub method: &'a str,
    pub user: &'a [u8],
    pub password: &'a [u8],
    pub salt: &'a [u8],
    pub protocol: MailProtocol,
    pub login_attempt: usize,
    pub client_ip: &'a str,
    pub client_host: Option<&'a str>,
    pub proxy_addresses: Option<&'a ProxyProtocolAddresses>,
    pub smtp_helo: &'a [u8],
    pub smtp_from: &'a [u8],
    pub smtp_to: &'a [u8],
    pub tls: Option<&'a TlsConnectionInfo>,
    pub client_certificate_verify: Option<&'a str>,
}

#[derive(Debug)]
pub(super) struct AuthHttpResponse {
    pub status: AuthHttpStatus,
    pub server: Option<String>,
    pub port: Option<u16>,
    pub user: Option<Vec<u8>>,
    pub password: Option<Vec<u8>>,
    pub wait: Duration,
}

#[derive(Debug)]
pub(super) enum AuthHttpStatus {
    Ok,
    Wait,
    Failed {
        code: Option<String>,
        message: String,
    },
}

enum AuthHttpStream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl AuthHttpStream {
    async fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.write_all(data).await,
            #[cfg(unix)]
            Self::Unix(stream) => stream.write_all(data).await,
        }
    }

    async fn read(&mut self, data: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(data).await,
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(data).await,
        }
    }
}

pub(super) async fn query_auth_http(
    config: &MailAuthHttpConfig,
    request: &AuthHttpRequest<'_>,
) -> io::Result<AuthHttpResponse> {
    let request_bytes = build_request(config, request)?;
    let operation = async {
        let mut stream = connect_auth_http(config).await?;
        stream.write_all(&request_bytes).await?;
        let response = read_response(&mut stream).await?;
        parse_response(&response)
    };
    tokio::time::timeout(config.timeout, operation)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "auth_http timed out"))?
}

async fn connect_auth_http(config: &MailAuthHttpConfig) -> io::Result<AuthHttpStream> {
    match &config.address {
        MailAuthHttpAddress::Inet(addresses) => {
            let mut last_error = None;
            for address in addresses {
                match TcpStream::connect(address).await {
                    Ok(stream) => {
                        let _ = stream.set_nodelay(true);
                        return Ok(AuthHttpStream::Tcp(stream));
                    }
                    Err(error) => last_error = Some(error),
                }
            }
            Err(last_error.unwrap_or_else(|| {
                io::Error::new(
                    io::ErrorKind::AddrNotAvailable,
                    "auth_http has no resolved address",
                )
            }))
        }
        MailAuthHttpAddress::Unix(path) => {
            #[cfg(unix)]
            {
                UnixStream::connect(path).await.map(AuthHttpStream::Unix)
            }
            #[cfg(not(unix))]
            {
                let _ = path;
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "UNIX auth_http is unsupported on this platform",
                ))
            }
        }
    }
}

fn build_request(
    config: &MailAuthHttpConfig,
    request: &AuthHttpRequest<'_>,
) -> io::Result<Vec<u8>> {
    let user = escape_auth_value(request.user);
    let password = escape_auth_value(request.password);
    let mut out = Vec::with_capacity(512 + user.len() + password.len());
    out.extend_from_slice(b"GET ");
    out.extend_from_slice(config.uri.as_bytes());
    out.extend_from_slice(b" HTTP/1.0\r\nHost: ");
    out.extend_from_slice(config.host_header.as_bytes());
    out.extend_from_slice(b"\r\nAuth-Method: ");
    out.extend_from_slice(request.method.as_bytes());
    out.extend_from_slice(b"\r\nAuth-User: ");
    out.extend_from_slice(&user);
    out.extend_from_slice(b"\r\nAuth-Pass: ");
    out.extend_from_slice(&password);
    out.extend_from_slice(b"\r\n");
    if matches!(request.method, "apop" | "cram-md5") && !request.salt.is_empty() {
        out.extend_from_slice(b"Auth-Salt: ");
        out.extend_from_slice(request.salt);
        if !request.salt.ends_with(b"\r\n") {
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"Auth-Protocol: ");
    out.extend_from_slice(request.protocol.as_str().as_bytes());
    out.extend_from_slice(b"\r\nAuth-Login-Attempt: ");
    out.extend_from_slice(request.login_attempt.to_string().as_bytes());
    out.extend_from_slice(b"\r\nClient-IP: ");
    out.extend_from_slice(request.client_ip.as_bytes());
    out.extend_from_slice(b"\r\n");
    if let Some(host) = request.client_host {
        out.extend_from_slice(b"Client-Host: ");
        out.extend_from_slice(host.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if let Some(addresses) = request.proxy_addresses {
        if let Some(address) = addresses.proxy_protocol_addr {
            out.extend_from_slice(b"Proxy-Protocol-Addr: ");
            out.extend_from_slice(address.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        if let Some(port) = addresses.proxy_protocol_port {
            out.extend_from_slice(b"Proxy-Protocol-Port: ");
            out.extend_from_slice(port.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        if let Some(address) = addresses.proxy_protocol_server_addr {
            out.extend_from_slice(b"Proxy-Protocol-Server-Addr: ");
            out.extend_from_slice(address.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
        if let Some(port) = addresses.proxy_protocol_server_port {
            out.extend_from_slice(b"Proxy-Protocol-Server-Port: ");
            out.extend_from_slice(port.to_string().as_bytes());
            out.extend_from_slice(b"\r\n");
        }
    }
    if request.method == MailAuthMethod::None.as_str() {
        out.extend_from_slice(b"Auth-SMTP-Helo: ");
        out.extend_from_slice(request.smtp_helo);
        out.extend_from_slice(b"\r\nAuth-SMTP-From: ");
        out.extend_from_slice(request.smtp_from);
        out.extend_from_slice(b"\r\nAuth-SMTP-To: ");
        out.extend_from_slice(request.smtp_to);
        out.extend_from_slice(b"\r\n");
    }
    if let Some(tls) = request.tls {
        out.extend_from_slice(b"Auth-SSL: on\r\nAuth-SSL-Protocol: ");
        out.extend_from_slice(tls.protocol.as_bytes());
        out.extend_from_slice(b"\r\nAuth-SSL-Cipher: ");
        out.extend_from_slice(tls.cipher.as_bytes());
        out.extend_from_slice(b"\r\n");
        if let Some(verify) = request.client_certificate_verify {
            out.extend_from_slice(b"Auth-SSL-Verify: ");
            out.extend_from_slice(verify.as_bytes());
            out.extend_from_slice(b"\r\n");
            if let Some(certificate) = &tls.client_certificate {
                out.extend_from_slice(b"Auth-SSL-Subject: ");
                out.extend_from_slice(certificate.subject_dn.as_bytes());
                out.extend_from_slice(b"\r\nAuth-SSL-Issuer: ");
                out.extend_from_slice(certificate.issuer_dn.as_bytes());
                out.extend_from_slice(b"\r\nAuth-SSL-Serial: ");
                out.extend(certificate.serial.bytes().filter(|byte| *byte != b':'));
                out.extend_from_slice(b"\r\nAuth-SSL-Fingerprint: ");
                out.extend_from_slice(certificate.fingerprint.as_bytes());
                out.extend_from_slice(b"\r\n");
                if config.pass_client_cert {
                    out.extend_from_slice(b"Auth-SSL-Cert: ");
                    out.extend_from_slice(&escape_auth_value(certificate.certificate.as_bytes()));
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
    }
    for (name, value) in &config.headers {
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    Ok(out)
}

fn escape_auth_value(value: &[u8]) -> Vec<u8> {
    let mut escaped = Vec::with_capacity(value.len());
    for byte in value {
        if matches!(*byte, b' ' | b'%' | 0..=31 | 127) {
            escaped.push(b'%');
            escaped.push(hex_digit(byte >> 4));
            escaped.push(hex_digit(byte & 0x0f));
        } else {
            escaped.push(*byte);
        }
    }
    escaped
}

fn hex_digit(value: u8) -> u8 {
    match value {
        0..=9 => b'0' + value,
        _ => b'A' + value - 10,
    }
}

async fn read_response(stream: &mut AuthHttpStream) -> io::Result<Vec<u8>> {
    let mut response = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        response.extend_from_slice(&chunk[..read]);
        if response.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if response.len() >= MAX_AUTH_HTTP_RESPONSE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "auth_http response headers are too large",
            ));
        }
    }
    Ok(response)
}

fn parse_response(response: &[u8]) -> io::Result<AuthHttpResponse> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "auth_http response has no complete header",
            )
        })?;
    let mut lines = response[..header_end].split(|byte| *byte == b'\n');
    let status_line = trim_cr(lines.next().unwrap_or_default());
    if !status_line.starts_with(b"HTTP/") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "auth_http response has an invalid status line",
        ));
    }

    let mut auth_status = None;
    let mut server = None;
    let mut port = None;
    let mut user = None;
    let mut password = None;
    let mut wait = Duration::ZERO;
    let mut error_code = None;
    let mut error_message = Vec::new();
    for line in lines {
        let line = trim_cr(line);
        if line.is_empty() {
            break;
        }
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "auth_http response has an invalid header",
            ));
        };
        let name = &line[..colon];
        let value = &line[colon + 1..];
        let value = trim_ascii(value);
        if name.eq_ignore_ascii_case(b"Auth-Status") {
            auth_status = Some(value.to_vec());
        } else if name.eq_ignore_ascii_case(b"Auth-Server") {
            server = Some(
                std::str::from_utf8(value)
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "auth_http server is not valid UTF-8",
                        )
                    })?
                    .to_string(),
            );
        } else if name.eq_ignore_ascii_case(b"Auth-Port") {
            let parsed = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|port| *port != 0);
            if parsed.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "auth_http response has an invalid Auth-Port",
                ));
            }
            port = parsed;
        } else if name.eq_ignore_ascii_case(b"Auth-User") {
            user = Some(value.to_vec());
        } else if name.eq_ignore_ascii_case(b"Auth-Pass") {
            password = Some(value.to_vec());
        } else if name.eq_ignore_ascii_case(b"Auth-Wait") {
            if let Some(seconds) = std::str::from_utf8(value)
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
            {
                wait = Duration::from_secs(seconds);
            }
        } else if name.eq_ignore_ascii_case(b"Auth-Error-Code") {
            error_code = Some(String::from_utf8_lossy(value).into_owned());
        } else if name.eq_ignore_ascii_case(b"Auth-Error-Message") {
            error_message = value.to_vec();
        }
    }

    let status = auth_status.unwrap_or_default();
    let status = if status == b"OK" {
        AuthHttpStatus::Ok
    } else if status == b"WAIT" {
        AuthHttpStatus::Wait
    } else {
        let message = if error_message.is_empty() {
            if status.is_empty() {
                "authentication failed".into()
            } else {
                String::from_utf8_lossy(&status).into_owned()
            }
        } else {
            String::from_utf8_lossy(&error_message).into_owned()
        };
        AuthHttpStatus::Failed {
            code: error_code,
            message,
        }
    };
    Ok(AuthHttpResponse {
        status,
        server,
        port,
        user,
        password,
        wait,
    })
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_http_response_headers() {
        let response = parse_response(
            b"HTTP/1.1 204 No Content\r\n\
              Auth-Status: OK\r\n\
              Auth-Server: 127.0.0.1\r\n\
              Auth-Port: 2525\r\n\
              Auth-Pass: secret\r\n\
              Auth-Wait: 2\r\n\r\n",
        )
        .unwrap();
        assert!(matches!(response.status, AuthHttpStatus::Ok));
        assert_eq!(response.server.as_deref(), Some("127.0.0.1"));
        assert_eq!(response.port, Some(2525));
        assert_eq!(response.password.as_deref(), Some(b"secret".as_slice()));
        assert_eq!(response.wait, Duration::from_secs(2));
    }

    #[test]
    fn escapes_http_auth_values() {
        assert_eq!(escape_auth_value(b"a b%c+d=e"), b"a%20b%25c+d=e");
    }
}
