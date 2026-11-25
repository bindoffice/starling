use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, SeekFrom};

use crate::SERVER_SOFTWARE;

pub(crate) mod buffer_pool;

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Arc<Vec<Header>>,
    pub body: Vec<u8>,
    pub body_stream: Option<RequestBodyStream>,
    pub remote_addr: Arc<str>,
    pub local_addr: Arc<str>,
    pub keep_alive: bool,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub fn headers(&self, name: &str) -> impl Iterator<Item = &str> {
        self.headers
            .iter()
            .filter(move |header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub fn host(&self) -> &str {
        self.header("host").unwrap_or_default()
    }

    pub fn host_without_port(&self) -> Cow<'_, str> {
        host_without_port(&self.method, &self.target, &self.headers)
    }

    pub fn request_port(&self) -> Option<u16> {
        request_port_from_parts(&self.method, &self.target, &self.headers)
    }

    pub fn path_and_query(&self) -> (&str, Option<&str>) {
        if let Some(rest) = absolute_target_rest(&self.target) {
            let fragment = rest.find('#').unwrap_or(rest.len());
            let target = &rest[..fragment];
            let query_at = target.find('?');
            let path_end = query_at.unwrap_or(target.len());
            let path = target[..path_end]
                .find('/')
                .map(|start| &target[start..path_end])
                .unwrap_or("/");
            let query = query_at.map(|index| &target[index + 1..]);
            return (path, query);
        }
        let target = if let Some(fragment) = self.target.find('#') {
            &self.target[..fragment]
        } else {
            self.target.as_str()
        };
        match target.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (target, None),
        }
    }

    pub fn has_request_body(&self) -> bool {
        !self.body.is_empty()
            || self.body_stream.is_some()
            || self.header("content-length").is_some()
            || self.header("transfer-encoding").is_some()
    }

    pub fn has_nonzero_request_body(&self) -> bool {
        self.buffered_body_len() > 0
            || self.header("transfer-encoding").is_some()
            || self
                .header("content-length")
                .and_then(|value| value.trim().parse::<u64>().ok())
                .is_some_and(|length| length > 0)
    }

    pub fn body_file(&self) -> Option<Arc<RequestBodyFile>> {
        self.body_stream.as_ref().and_then(RequestBodyStream::file)
    }

    pub fn buffered_body_len(&self) -> u64 {
        self.body_file()
            .map(|file| file.len())
            .unwrap_or(self.body.len() as u64)
    }
}

pub(crate) fn request_port_from_parts(
    method: &str,
    target: &str,
    headers: &[Header],
) -> Option<u16> {
    if method == "CONNECT" {
        return parse_connect_authority(target).map(|(_, port)| port);
    }
    let authority = absolute_target_authority(target).or_else(|| {
        headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case("host"))
            .map(|header| header.value.as_str())
    })?;
    parse_connect_authority(authority).map(|(_, port)| port)
}

fn absolute_target_rest(target: &str) -> Option<&str> {
    target
        .get(..7)
        .filter(|scheme| scheme.eq_ignore_ascii_case("http://"))
        .map(|_| &target[7..])
        .or_else(|| {
            target
                .get(..8)
                .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
                .map(|_| &target[8..])
        })
}

pub(in crate::http) fn absolute_target_authority(target: &str) -> Option<&str> {
    let rest = absolute_target_rest(target)?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&rest[..end])
}

fn normalize_host(host: &str) -> Cow<'_, str> {
    let host = host.strip_suffix('.').unwrap_or(host);
    lowercase_host(host)
}

fn lowercase_host(host: &str) -> Cow<'_, str> {
    if host.bytes().any(|byte| byte.is_ascii_uppercase()) {
        Cow::Owned(host.to_ascii_lowercase())
    } else {
        Cow::Borrowed(host)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderName(Cow<'static, str>);

impl HeaderName {
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }

    pub(crate) const fn from_static(name: &'static str) -> Self {
        Self(Cow::Borrowed(name))
    }
}

impl std::ops::Deref for HeaderName {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl AsRef<str> for HeaderName {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Display for HeaderName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<&str> for HeaderName {
    fn from(name: &str) -> Self {
        canonical_header_name(name)
            .map(|name| Self(Cow::Borrowed(name)))
            .unwrap_or_else(|| Self(Cow::Owned(name.to_owned())))
    }
}

impl From<String> for HeaderName {
    fn from(name: String) -> Self {
        canonical_header_name(&name)
            .map(|name| Self(Cow::Borrowed(name)))
            .unwrap_or(Self(Cow::Owned(name)))
    }
}

impl From<&String> for HeaderName {
    fn from(name: &String) -> Self {
        Self::from(name.as_str())
    }
}

impl From<HeaderName> for String {
    fn from(name: HeaderName) -> Self {
        name.0.into_owned()
    }
}

impl PartialEq<str> for HeaderName {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for HeaderName {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<HeaderName> for str {
    fn eq(&self, other: &HeaderName) -> bool {
        self == other.as_str()
    }
}

fn canonical_header_name(name: &str) -> Option<&'static str> {
    let bytes = name.as_bytes();
    let first = bytes.first()?.to_ascii_lowercase();
    match (bytes.len(), first) {
        (2, b't') if name.eq_ignore_ascii_case("TE") => Some("TE"),
        (3, b'p') if name.eq_ignore_ascii_case("P3P") => Some("P3P"),
        (3, b'v') if name.eq_ignore_ascii_case("Via") => Some("Via"),
        (4, b'd') if name.eq_ignore_ascii_case("Date") => Some("Date"),
        (4, b'e') if name.eq_ignore_ascii_case("ETag") => Some("ETag"),
        (4, b'h') if name.eq_ignore_ascii_case("Host") => Some("Host"),
        (4, b'l') if name.eq_ignore_ascii_case("Link") => Some("Link"),
        (4, b'v') if name.eq_ignore_ascii_case("Vary") => Some("Vary"),
        (5, b'a') if name.eq_ignore_ascii_case("Allow") => Some("Allow"),
        (5, b'r') if name.eq_ignore_ascii_case("Range") => Some("Range"),
        (6, b'a') if name.eq_ignore_ascii_case("Accept") => Some("Accept"),
        (6, b'c') if name.eq_ignore_ascii_case("Cookie") => Some("Cookie"),
        (6, b'e') if name.eq_ignore_ascii_case("Expect") => Some("Expect"),
        (6, b's') if name.eq_ignore_ascii_case("Server") => Some("Server"),
        (7, b'e') if name.eq_ignore_ascii_case("Expires") => Some("Expires"),
        (7, b'r') if name.eq_ignore_ascii_case("Referer") => Some("Referer"),
        (7, b't') if name.eq_ignore_ascii_case("Trailer") => Some("Trailer"),
        (7, b'u') if name.eq_ignore_ascii_case("Upgrade") => Some("Upgrade"),
        (8, b'l') if name.eq_ignore_ascii_case("Location") => Some("Location"),
        (9, b'x') if name.eq_ignore_ascii_case("X-Real-IP") => Some("X-Real-IP"),
        (10, b'c') if name.eq_ignore_ascii_case("Connection") => Some("Connection"),
        (10, b'k') if name.eq_ignore_ascii_case("Keep-Alive") => Some("Keep-Alive"),
        (10, b's') if name.eq_ignore_ascii_case("Set-Cookie") => Some("Set-Cookie"),
        (10, b'u') if name.eq_ignore_ascii_case("User-Agent") => Some("User-Agent"),
        (12, b'c') if name.eq_ignore_ascii_case("Content-Type") => Some("Content-Type"),
        (13, b'a') if name.eq_ignore_ascii_case("Accept-Ranges") => Some("Accept-Ranges"),
        (13, b'a') if name.eq_ignore_ascii_case("Authorization") => Some("Authorization"),
        (13, b'c') if name.eq_ignore_ascii_case("Cache-Control") => Some("Cache-Control"),
        (13, b'c') if name.eq_ignore_ascii_case("Content-Range") => Some("Content-Range"),
        (13, b'i') if name.eq_ignore_ascii_case("If-None-Match") => Some("If-None-Match"),
        (13, b'l') if name.eq_ignore_ascii_case("Last-Modified") => Some("Last-Modified"),
        (14, b'c') if name.eq_ignore_ascii_case("Content-Length") => Some("Content-Length"),
        (15, b'a') if name.eq_ignore_ascii_case("Accept-Encoding") => Some("Accept-Encoding"),
        (15, b'a') if name.eq_ignore_ascii_case("Accept-Language") => Some("Accept-Language"),
        (15, b'x') if name.eq_ignore_ascii_case("X-Accel-Charset") => Some("X-Accel-Charset"),
        (15, b'x') if name.eq_ignore_ascii_case("X-Forwarded-For") => Some("X-Forwarded-For"),
        (16, b'c') if name.eq_ignore_ascii_case("Content-Encoding") => Some("Content-Encoding"),
        (16, b'c') if name.eq_ignore_ascii_case("Content-Language") => Some("Content-Language"),
        (16, b'c') if name.eq_ignore_ascii_case("Content-Location") => Some("Content-Location"),
        (16, b'p') if name.eq_ignore_ascii_case("Proxy-Connection") => Some("Proxy-Connection"),
        (16, b'w') if name.eq_ignore_ascii_case("WWW-Authenticate") => Some("WWW-Authenticate"),
        (16, b'x') if name.eq_ignore_ascii_case("X-Accel-Redirect") => Some("X-Accel-Redirect"),
        (17, b'i') if name.eq_ignore_ascii_case("If-Modified-Since") => Some("If-Modified-Since"),
        (17, b't') if name.eq_ignore_ascii_case("Transfer-Encoding") => Some("Transfer-Encoding"),
        (17, b'x') if name.eq_ignore_ascii_case("X-Accel-Buffering") => Some("X-Accel-Buffering"),
        (17, b'x') if name.eq_ignore_ascii_case("X-Forwarded-Proto") => Some("X-Forwarded-Proto"),
        (18, b'x') if name.eq_ignore_ascii_case("X-Accel-Limit-Rate") => Some("X-Accel-Limit-Rate"),
        (19, b'c') if name.eq_ignore_ascii_case("Content-Disposition") => {
            Some("Content-Disposition")
        }
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct Header {
    pub name: HeaderName,
    pub value: String,
}

const RAW_HEADER_PREFIX: &str = "\0starling-raw:";
const RAW_HEADER_SUFFIX: char = '\0';

pub(crate) fn header_value_from_bytes(bytes: &[u8]) -> String {
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_owned();
    }

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(
        RAW_HEADER_PREFIX
            .len()
            .saturating_add(bytes.len().saturating_mul(2))
            .saturating_add(1),
    );
    value.push_str(RAW_HEADER_PREFIX);
    for byte in bytes {
        value.push(HEX[usize::from(byte >> 4)] as char);
        value.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    value.push(RAW_HEADER_SUFFIX);
    value
}

pub(crate) fn header_value_bytes(value: &str) -> Cow<'_, [u8]> {
    if !value.as_bytes().contains(&0) {
        return Cow::Borrowed(value.as_bytes());
    }
    let Some(decoded) = decode_raw_header_values(value) else {
        return Cow::Borrowed(value.as_bytes());
    };
    Cow::Owned(decoded)
}

fn decode_raw_header_values(value: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start) = remaining.find(RAW_HEADER_PREFIX) {
        output.extend_from_slice(&remaining.as_bytes()[..start]);
        let encoded_with_suffix = &remaining[start + RAW_HEADER_PREFIX.len()..];
        let end = encoded_with_suffix.find(RAW_HEADER_SUFFIX)?;
        let encoded = &encoded_with_suffix[..end];
        if !encoded.len().is_multiple_of(2) {
            return None;
        }
        for pair in encoded.as_bytes().as_chunks::<2>().0 {
            output.push((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?);
        }
        remaining = &encoded_with_suffix[end + RAW_HEADER_SUFFIX.len_utf8()..];
    }
    output.extend_from_slice(remaining.as_bytes());
    Some(output)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug)]
pub struct RequestHead {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<Header>,
    pub request_length: u64,
    pub(crate) invalid_headers: Vec<InvalidRequestHeader>,
    pub keep_alive: bool,
    pub content_length: Option<u64>,
    pub chunked: bool,
    pub expect_continue: bool,
    pub invalid_host: bool,
}

impl RequestHead {
    pub fn host_without_port(&self) -> Cow<'_, str> {
        host_without_port(&self.method, &self.target, &self.headers)
    }
}

fn host_without_port<'a>(method: &str, target: &'a str, headers: &'a [Header]) -> Cow<'a, str> {
    if method == "CONNECT"
        && let Some((host, _)) = parse_connect_authority(target)
    {
        return normalize_host(host);
    }
    let header_host = headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("host"))
        .map(|header| header.value.as_str())
        .unwrap_or_default();
    let host = absolute_target_authority(target).unwrap_or(header_host);
    if let Some(rest) = host.strip_prefix('[') {
        let literal = rest
            .split_once(']')
            .map(|(address, _)| &host[..address.len() + 2])
            .unwrap_or(host);
        return lowercase_host(literal);
    }
    let host = host.split_once(':').map(|(name, _)| name).unwrap_or(host);
    normalize_host(host)
}

mod rate;
mod request;
mod response;
mod temp_file;
mod upstream;
mod validation;

pub(crate) use rate::ReadRate;
pub use request::*;
pub(crate) use response::write_prepared_response_with_scratch;
pub use response::*;
pub use temp_file::*;
pub use upstream::*;
pub use validation::*;

#[cfg(test)]
mod tests;
