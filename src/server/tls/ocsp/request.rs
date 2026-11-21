use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use super::certificate::CertificateIdentity;
use super::response::{self, ValidatedResponse};
use super::url::ResponderUrl;
use crate::server::resolver::Resolver;

const MAX_OCSP_RESPONSE: u64 = 1024 * 1024;

pub(super) async fn fetch(
    identity: Arc<CertificateIdentity>,
    responder: ResponderUrl,
    resolver: Resolver,
    timeout: Duration,
) -> io::Result<ValidatedResponse> {
    let request = identity.request()?;
    let request = build_http_request(&request, &responder);
    let addresses = resolver.resolve(&responder.host).await?;
    let mut last_error = None;
    for address in addresses {
        let address = SocketAddr::new(address, responder.port);
        match tokio::time::timeout(timeout, fetch_address(address, &request)).await {
            Ok(Ok(response)) => match response::validate(&identity, &response) {
                Ok(response) => return Ok(response),
                Err(error) => last_error = Some(error),
            },
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

fn build_http_request(request: &[u8], responder: &ResponderUrl) -> Vec<u8> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(request);
    let mut path = responder.path.clone();
    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(&percent_encode_component(encoded.as_bytes()));
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

async fn fetch_address(address: SocketAddr, request: &[u8]) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(address).await?;
    stream.write_all(request).await?;
    let mut response = Vec::new();
    stream
        .take(MAX_OCSP_RESPONSE)
        .read_to_end(&mut response)
        .await?;
    parse_http_response(&response)
}

fn parse_http_response(response: &[u8]) -> io::Result<Vec<u8>> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| invalid_data("incomplete OCSP HTTP response"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| invalid_data("invalid OCSP HTTP headers"))?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| invalid_data("invalid OCSP HTTP status line"))?;
    if status != 200 {
        return Err(invalid_data(format!(
            "OCSP responder returned HTTP {status}"
        )));
    }
    let mut content_type = None;
    let mut content_length = None;
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
        }
    }
    if !content_type.is_some_and(|value| value.starts_with("application/ocsp-response")) {
        return Err(invalid_data("invalid OCSP response Content-Type"));
    }
    let body = &response[header_end + 4..];
    if let Some(length) = content_length {
        return body
            .get(..length)
            .map(ToOwned::to_owned)
            .ok_or_else(|| invalid_data("truncated OCSP response body"));
    }
    Ok(body.to_vec())
}

fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ocsp_http_response() {
        let response = b"HTTP/1.0 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: 3\r\n\r\nabc";
        assert_eq!(parse_http_response(response).unwrap(), b"abc");
    }

    #[test]
    fn rejects_wrong_content_type() {
        let response = b"HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nabc";
        assert!(parse_http_response(response).is_err());
    }
}
