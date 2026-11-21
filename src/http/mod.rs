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

    pub fn host_without_port(&self) -> &str {
        if self.method == "CONNECT"
            && let Some((host, _)) = parse_connect_authority(&self.target)
        {
            return host;
        }
        let host = self.host();
        if let Some(rest) = host.strip_prefix('[') {
            return rest
                .split_once(']')
                .map(|(address, _)| &host[..address.len() + 2])
                .unwrap_or(host);
        }
        host.split_once(':').map(|(name, _)| name).unwrap_or(host)
    }

    pub fn request_port(&self) -> Option<u16> {
        if self.method == "CONNECT" {
            return parse_connect_authority(&self.target).map(|(_, port)| port);
        }
        let target = self
            .target
            .strip_prefix("http://")
            .or_else(|| self.target.strip_prefix("https://"))?;
        let authority = target
            .split_once('/')
            .map_or(target, |(authority, _)| authority);
        parse_connect_authority(authority).map(|(_, port)| port)
    }

    pub fn path_and_query(&self) -> (&str, Option<&str>) {
        let target = if self.target.starts_with("http://") || self.target.starts_with("https://") {
            self.target
                .find("://")
                .and_then(|scheme_end| {
                    self.target[scheme_end + 3..]
                        .find('/')
                        .map(|path| &self.target[scheme_end + 3 + path..])
                })
                .unwrap_or("/")
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
