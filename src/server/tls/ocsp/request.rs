use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

use super::super::{ClientTlsOptions, build_client_tls_config};
use super::certificate::CertificateIdentity;
use super::response::{self, ValidatedResponse};
use super::url::{ResponderUrl, Scheme};
use crate::config::{TlsCipherConfig, TlsProtocols};
use crate::server::resolver::Resolver;

const MAX_OCSP_RESPONSE: usize = 1024 * 1024;
const MAX_REDIRECTS: usize = 5;

pub(super) async fn fetch(
    identity: Arc<CertificateIdentity>,
    mut responder: ResponderUrl,
    trusted_certificate: Option<PathBuf>,
    resolver: Resolver,
    timeout: Duration,
) -> io::Result<ValidatedResponse> {
    let request = identity.request()?;
    let deadline = Instant::now() + timeout;
    for redirects in 0..=MAX_REDIRECTS {
        let http_request = build_http_request(&request, &responder);
        let http = fetch_hop(
            &responder,
            &http_request,
            trusted_certificate.as_deref(),
            &resolver,
            deadline,
        )
        .await?;
        if matches!(http.status, 301 | 302 | 303 | 307 | 308) {
            if redirects == MAX_REDIRECTS {
                return Err(invalid_data("too many OCSP responder redirects"));
            }
            let location = http
                .location
                .ok_or_else(|| invalid_data("OCSP redirect has no Location header"))?;
            responder = responder.redirect(&location)?;
            continue;
        }
        if http.status != 200 {
            return Err(invalid_data(format!(
                "OCSP responder returned HTTP {}",
                http.status
            )));
        }
        if !http
            .content_type
            .is_some_and(|value| value.starts_with("application/ocsp-response"))
        {
            return Err(invalid_data("invalid OCSP response Content-Type"));
        }
        return response::validate(&identity, &http.body);
    }
    unreachable!("redirect loop is bounded")
}

fn build_http_request(request: &[u8], responder: &ResponderUrl) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(request);
    let path = responder.request_path(&percent_encode_component(encoded.as_bytes()));
    format!(
        "GET {path} HTTP/1.0\r\nHost: {}\r\nAccept: application/ocsp-response\r\nConnection: close\r\n\r\n",
        responder.host_header()
    )
    .into_bytes()
}

fn percent_encode_component(value: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

async fn fetch_hop(
    responder: &ResponderUrl,
    request: &[u8],
    trusted_certificate: Option<&std::path::Path>,
    resolver: &Resolver,
    deadline: Instant,
) -> io::Result<HttpResponse> {
    let addresses = resolver.resolve(&responder.host).await?;
    let connector = match responder.scheme {
        Scheme::Http => None,
        Scheme::Https => Some(tokio_rustls::TlsConnector::from(build_client_tls_config(
            ClientTlsOptions {
                verify: true,
                server_name: true,
                session_reuse: true,
                trusted_certificate,
                verify_depth: 8,
                crl: None,
                client_certificate: None,
                client_certificate_key: None,
                client_certificate_passwords: &[],
                protocols: TlsProtocols::default(),
                ciphers: &TlsCipherConfig::default(),
                alpn_protocols: &[],
            },
        )?)),
    };
    let mut last_error = None;
    for address in addresses {
        let address = SocketAddr::new(address, responder.port);
        let remaining = remaining(deadline, &address)?;
        match tokio::time::timeout(
            remaining,
            fetch_address(address, responder, connector.as_ref(), request),
        )
        .await
        {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => {
                last_error = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("OCSP request to {address} timed out"),
                ));
            }
        }
    }
    Err(last_error
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no OCSP responder address")))
}

async fn fetch_address(
    address: SocketAddr,
    responder: &ResponderUrl,
    connector: Option<&tokio_rustls::TlsConnector>,
    request: &[u8],
) -> io::Result<HttpResponse> {
    let stream = TcpStream::connect(address).await?;
    match connector {
        Some(connector) => {
            let server_name =
                tokio_rustls::rustls::pki_types::ServerName::try_from(responder.host.clone())
                    .map_err(|_| invalid_data("invalid OCSP HTTPS responder name"))?;
            let mut stream = connector.connect(server_name, stream).await?;
            read_http_response(&mut stream, request).await
        }
        None => {
            let mut stream = stream;
            read_http_response(&mut stream, request).await
        }
    }
}

async fn read_http_response<S>(stream: &mut S, request: &[u8]) -> io::Result<HttpResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(request).await?;
    let mut response = Vec::new();
    let read = stream
        .take((MAX_OCSP_RESPONSE + 1) as u64)
        .read_to_end(&mut response)
        .await;
    if let Err(error) = read
        && (error.kind() != io::ErrorKind::UnexpectedEof || response.is_empty())
    {
        return Err(error);
    }
    if response.len() > MAX_OCSP_RESPONSE {
        return Err(invalid_data("OCSP response exceeds size limit"));
    }
    parse_http_response(&response)
}

fn remaining(deadline: Instant, address: &SocketAddr) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                format!("OCSP request to {address} timed out"),
            )
        })
}

struct HttpResponse {
    status: u16,
    content_type: Option<String>,
    location: Option<String>,
    body: Vec<u8>,
}

fn parse_http_response(response: &[u8]) -> io::Result<HttpResponse> {
    let (header_end, separator_len) = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|position| (position, 4))
        .or_else(|| {
            response
                .windows(2)
                .position(|window| window == b"\n\n")
                .map(|position| (position, 2))
        })
        .ok_or_else(|| invalid_data("incomplete OCSP HTTP response"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| invalid_data("invalid OCSP HTTP headers"))?;
    let mut lines = headers.lines();
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| invalid_data("invalid OCSP HTTP status line"))?;
    let mut content_type = None;
    let mut content_length = None;
    let mut location = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.trim().to_ascii_lowercase());
        } else if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| invalid_data("invalid OCSP Content-Length"))?,
            );
        } else if name.eq_ignore_ascii_case("location") {
            location = Some(value.trim().to_string());
        }
    }
    let body = &response[header_end + separator_len..];
    if let Some(length) = content_length {
        return Ok(HttpResponse {
            status,
            content_type,
            location,
            body: body
                .get(..length)
                .ok_or_else(|| invalid_data("truncated OCSP response body"))?
                .to_vec(),
        });
    }
    Ok(HttpResponse {
        status,
        content_type,
        location,
        body: body.to_vec(),
    })
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolverConfig;
    use std::path::Path;
    use tokio::net::TcpListener;
    use tokio_rustls::TlsAcceptor;
    use tokio_rustls::rustls::ServerConfig;

    fn fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
    }

    #[test]
    fn accepts_ocsp_http_response() {
        let response = b"HTTP/1.0 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: 3\r\n\r\nabc";
        let response = parse_http_response(response).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"abc");
    }

    #[test]
    fn accepts_ocsp_http_response_with_lf_line_endings() {
        let response =
            b"HTTP/1.1 200 OK\nContent-Type: application/ocsp-response\nContent-Length: 3\n\nabc";
        let response = parse_http_response(response).unwrap();
        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"abc");
    }

    #[test]
    fn parses_redirect_location() {
        let response =
            parse_http_response(b"HTTP/1.1 302 Found\r\nLocation: /ocsp-v2\r\n\r\n").unwrap();
        assert_eq!(response.status, 302);
        assert_eq!(response.location.as_deref(), Some("/ocsp-v2"));
    }

    #[tokio::test]
    async fn follows_redirects_and_fetches_a_valid_response() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocsp");
        let chain = crate::server::tls::load_certificates(&fixture.join("server.crt")).unwrap();
        let identity = Arc::new(
            super::super::certificate::CertificateIdentity::load(
                &chain,
                Some(&fixture.join("ca.crt")),
                true,
            )
            .unwrap()
            .unwrap(),
        );
        let ocsp_response = std::fs::read(fixture.join("response.der")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response_for_server = ocsp_response.clone();
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let length = stream.read(&mut buffer).await.unwrap();
                    assert_ne!(length, 0);
                    request.extend_from_slice(&buffer[..length]);
                }
                if index == 0 {
                    assert!(request.starts_with(b"GET /old/"));
                    stream
                        .write_all(
                            b"HTTP/1.1 302 Found\r\nLocation: /new\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .unwrap();
                } else {
                    assert!(request.starts_with(b"GET /new/"));
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
                        response_for_server.len()
                    );
                    stream.write_all(headers.as_bytes()).await.unwrap();
                    stream.write_all(&response_for_server).await.unwrap();
                }
            }
        });
        let fetched = fetch(
            identity,
            ResponderUrl::parse(&format!("http://{address}/old")).unwrap(),
            None,
            Resolver::new(
                ResolverConfig {
                    servers: vec!["127.0.0.1:1".parse().unwrap()],
                    valid: None,
                    ipv6: false,
                },
                Duration::from_secs(1),
            ),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(fetched.der, ocsp_response);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn verifies_the_https_responder_certificate_chain() {
        let fixture = fixture();
        let certificates =
            crate::server::tls::load_certificates(&fixture.join("test.crt")).unwrap();
        let key = crate::server::tls::load_private_key(&fixture.join("test.key"), &[]).unwrap();
        let server_config = Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certificates, key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let ocsp_response = std::fs::read(fixture.join("ocsp/response.der")).unwrap();
        let response_for_server = ocsp_response.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = TlsAcceptor::from(server_config)
                .accept(stream)
                .await
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&buffer[..length]);
            }
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
                response_for_server.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&response_for_server).await.unwrap();
        });

        let responder =
            ResponderUrl::parse(&format!("https://localhost:{}/ocsp", address.port())).unwrap();
        let connector = tokio_rustls::TlsConnector::from(
            build_client_tls_config(ClientTlsOptions {
                verify: true,
                server_name: true,
                session_reuse: false,
                trusted_certificate: Some(Path::new(&fixture.join("ca.crt"))),
                verify_depth: 2,
                crl: None,
                client_certificate: None,
                client_certificate_key: None,
                client_certificate_passwords: &[],
                protocols: TlsProtocols::default(),
                ciphers: &TlsCipherConfig::default(),
                alpn_protocols: &[],
            })
            .unwrap(),
        );
        let request = build_http_request(b"request", &responder);
        let response = fetch_address(address, &responder, Some(&connector), &request)
            .await
            .unwrap();
        assert_eq!(response.body, ocsp_response);
        server.await.unwrap();
    }
}
