use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, SeekFrom};

use crate::SERVER_SOFTWARE;

pub const MAX_HEADER_BYTES: usize = 64 * 1024;
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Request {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<Header>,
    pub body: Vec<u8>,
    pub body_stream: Option<RequestBodyStream>,
    pub remote_addr: String,
    pub local_addr: String,
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
        if self.method == "CONNECT"
            && let Some((host, _)) = parse_connect_authority(&self.target)
        {
            return normalize_host(host);
        }
        let host = self.absolute_authority().unwrap_or_else(|| self.host());
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

    pub fn request_port(&self) -> Option<u16> {
        if self.method == "CONNECT" {
            return parse_connect_authority(&self.target).map(|(_, port)| port);
        }
        let authority = self.absolute_authority()?;
        parse_connect_authority(authority).map(|(_, port)| port)
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

    fn absolute_authority(&self) -> Option<&str> {
        absolute_target_authority(&self.target)
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

#[derive(Debug, Clone)]
pub struct Header {
    pub name: String,
    pub value: String,
}

#[derive(Debug)]
pub struct RequestHead {
    pub method: String,
    pub target: String,
    pub version: String,
    pub headers: Vec<Header>,
    pub(crate) invalid_headers: Vec<InvalidRequestHeader>,
    pub keep_alive: bool,
    pub content_length: Option<u64>,
    pub chunked: bool,
    pub expect_continue: bool,
}

mod rate;
mod request;
mod response;
mod temp_file;
mod upstream;
mod validation;

pub(crate) use rate::ReadRate;
pub use request::*;
pub use response::*;
pub use temp_file::*;
pub use upstream::*;
pub use validation::*;

#[cfg(test)]
mod tests;
