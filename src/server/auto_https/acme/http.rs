use super::*;
use crate::config::TlsCipherConfig;
use crate::server::tls::{ClientTlsOptions, build_client_tls_config};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;

const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(in crate::server) struct AcmeHttpResponse {
    pub(in crate::server) status: u16,
    pub(in crate::server) headers: HashMap<String, String>,
    pub(in crate::server) body: Vec<u8>,
}

impl AcmeHttpResponse {
    pub(in crate::server) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

pub(in crate::server) struct AcmeHttpClient {
    tls: Option<TlsConnector>,
}

impl AcmeHttpClient {
    pub(in crate::server) fn new(ca_file: Option<&Path>) -> io::Result<Self> {
        let ciphers = TlsCipherConfig::default();
        let tls = TlsConnector::from(build_client_tls_config(ClientTlsOptions {
            verify: true,
            server_name: true,
            session_reuse: true,
            trusted_certificate: ca_file,
            verify_depth: 10,
            crl: None,
            client_certificate: None,
            client_certificate_key: None,
            client_certificate_passwords: &[],
            protocols: TlsProtocols::default(),
            ciphers: &ciphers,
            alpn_protocols: &[],
        })?);
        Ok(Self { tls: Some(tls) })
    }

    pub(in crate::server) async fn request(
        &self,
        method: &str,
        url: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> io::Result<AcmeHttpResponse> {
        tokio::time::timeout(
            REQUEST_TIMEOUT,
            self.request_without_timeout(method, url, content_type, body),
        )
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("ACME HTTP request to {url} timed out"),
            )
        })?
    }

    async fn request_without_timeout(
        &self,
        method: &str,
        url: &str,
        content_type: Option<&str>,
        body: &[u8],
    ) -> io::Result<AcmeHttpResponse> {
        let url = ParsedUrl::parse(url)?;
        let mut stream = connect(&url, self.tls.as_ref()).await?;
        let mut request = format!(
            "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: */*\r\n",
            url.target, url.authority
        );
        if let Some(content_type) = content_type {
            request.push_str(&format!("Content-Type: {content_type}\r\n"));
        }
        if method != "GET" && method != "HEAD" {
            request.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes()).await?;
        if method != "GET" && method != "HEAD" {
            stream.write_all(body).await?;
        }
        stream.flush().await?;
        let mut response = Vec::new();
        let mut limited = stream.take((MAX_RESPONSE_BYTES + 1) as u64);
        limited.read_to_end(&mut response).await?;
        if response.len() > MAX_RESPONSE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ACME HTTP response is too large",
            ));
        }
        parse_response(&response)
    }
}

async fn connect(
    url: &ParsedUrl,
    tls: Option<&TlsConnector>,
) -> io::Result<Box<dyn AsyncReadWrite + Send>> {
    let address = format!("{}:{}", url.connect_host, url.port);
    let stream = TcpStream::connect(&address).await?;
    stream.set_nodelay(true)?;
    if url.secure {
        let tls = tls.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "ACME HTTPS client has no TLS configuration",
            )
        })?;
        let server_name = ServerName::try_from(url.host.clone()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid ACME server name \"{}\"", url.host),
            )
        })?;
        let stream = tls.connect(server_name, stream).await?;
        Ok(Box::new(stream))
    } else {
        Ok(Box::new(stream))
    }
}

trait AsyncReadWrite: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AsyncReadWrite for T {}

fn parse_response(response: &[u8]) -> io::Result<AcmeHttpResponse> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid ACME HTTP response"))?;
    let head = std::str::from_utf8(&response[..header_end]).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "ACME HTTP response headers are not UTF-8",
        )
    })?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid ACME HTTP status"))?;
    let mut headers = HashMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let body = &response[header_end + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|value| value.to_ascii_lowercase().contains("chunked"))
    {
        decode_chunked(body)?
    } else if let Some(length) = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
    {
        body.get(..length).unwrap_or(body).to_vec()
    } else {
        body.to_vec()
    };
    Ok(AcmeHttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(mut body: &[u8]) -> io::Result<Vec<u8>> {
    let mut decoded = Vec::new();
    loop {
        let line_end = body
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid chunked body"))?;
        let size = std::str::from_utf8(&body[..line_end])
            .ok()
            .and_then(|line| line.split(';').next())
            .and_then(|line| usize::from_str_radix(line.trim(), 16).ok())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size"))?;
        body = &body[line_end + 2..];
        if size == 0 {
            return Ok(decoded);
        }
        let chunk = body
            .get(..size)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "truncated chunked body"))?;
        decoded.extend_from_slice(chunk);
        body = body.get(size + 2..).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "truncated chunk terminator")
        })?;
    }
}

struct ParsedUrl {
    secure: bool,
    host: String,
    connect_host: String,
    port: u16,
    authority: String,
    target: String,
}

impl ParsedUrl {
    fn parse(value: &str) -> io::Result<Self> {
        let (secure, rest) = if let Some(rest) = value.strip_prefix("https://") {
            (true, rest)
        } else if let Some(rest) = value.strip_prefix("http://") {
            (false, rest)
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unsupported ACME URL \"{value}\""),
            ));
        };
        let slash = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..slash];
        let target = if slash == rest.len() {
            "/".to_string()
        } else {
            rest[slash..].to_string()
        };
        let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
            let end = rest.find(']').ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid IPv6 ACME URL")
            })?;
            let host = &rest[..end];
            let port = rest[end + 1..]
                .strip_prefix(':')
                .and_then(|port| port.parse::<u16>().ok())
                .unwrap_or(if secure { 443 } else { 80 });
            (host.to_string(), port)
        } else if let Some((host, port)) = authority.rsplit_once(':') {
            let port = port.parse::<u16>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid ACME URL port")
            })?;
            (host.to_string(), port)
        } else {
            (authority.to_string(), if secure { 443 } else { 80 })
        };
        if host.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ACME URL has no host",
            ));
        }
        let connect_host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.clone()
        };
        Ok(Self {
            secure,
            host,
            connect_host,
            port,
            authority: authority.to_string(),
            target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_https_urls() {
        let https = ParsedUrl::parse("https://acme.example.test:8443/dir?x=1").unwrap();
        assert!(https.secure);
        assert_eq!(https.host, "acme.example.test");
        assert_eq!(https.port, 8443);
        assert_eq!(https.target, "/dir?x=1");

        let http = ParsedUrl::parse("http://127.0.0.1:14000/dir").unwrap();
        assert!(!http.secure);
        assert_eq!(http.connect_host, "127.0.0.1");
        assert_eq!(http.port, 14000);
    }

    #[test]
    fn decodes_chunked_responses() {
        let response =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\ntest\r\n3\r\n123\r\n0\r\n\r\n";
        let response = parse_response(response).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"test123");
    }
}
