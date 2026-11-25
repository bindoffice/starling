use super::*;
use crate::config::ServerTokens;
use std::sync::Arc;

mod body;
mod early_hints;
mod stream;
mod tokens;
mod writer;

pub use body::*;
pub(crate) use early_hints::write_early_hints;
pub(crate) use stream::{ResponseStream, ResponseStreamItem};
pub(crate) use tokens::prepare_response_headers;
pub use writer::{ResponseWriteOptions, write_response};
pub(crate) use writer::{
    prepare_response_for_write, response_head_length, response_uses_chunked_encoding,
    write_prepared_response_head, write_prepared_response_with_scratch,
};

const RESPONSE_HEADER_CAPACITY: usize = 8;

pub struct Response {
    pub status: u16,
    pub headers: Vec<Header>,
    pub trailers: Vec<Header>,
    pub(crate) chunked_expected: bool,
    pub body: ResponseBody,
    // Keeps the backing temp path alive until the response body has been sent.
    #[allow(dead_code)]
    pub(crate) body_temp_file: Option<Arc<TempFileGuard>>,
    pub upstream: Option<Box<UpstreamControl>>,
    pub(crate) builtin_error: bool,
    pub(crate) msie_padding: bool,
    pub(crate) allow_ranges: bool,
    pub(crate) single_range: bool,
}

#[derive(Default)]
pub struct UpstreamControl {
    pub redirect: Option<String>,
    pub limit_rate: Option<u64>,
    pub buffering: Option<AccelBuffering>,
    pub charset: Option<String>,
    pub early_hints: Vec<Vec<Header>>,
    pub headers: Vec<Header>,
    pub trailers: Vec<Header>,
}

impl UpstreamControl {
    fn is_empty(&self) -> bool {
        self.redirect.is_none()
            && self.limit_rate.is_none()
            && self.buffering.is_none()
            && self.charset.is_none()
            && self.early_hints.is_empty()
            && self.headers.is_empty()
            && self.trailers.is_empty()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ResponseRate {
    pub rate: u64,
    pub after: u64,
}

impl Response {
    pub fn new(status: u16) -> Self {
        Self {
            status,
            headers: Vec::with_capacity(RESPONSE_HEADER_CAPACITY),
            trailers: Vec::new(),
            chunked_expected: false,
            body: ResponseBody::Empty,
            body_temp_file: None,
            upstream: None,
            builtin_error: false,
            msie_padding: false,
            allow_ranges: false,
            single_range: false,
        }
    }

    pub fn bytes(status: u16, content_type: &str, body: impl Into<Vec<u8>>) -> Self {
        let mut response = Self::new(status);
        response.body = ResponseBody::Bytes(body.into());
        response.add_header_static("Content-Type", content_type);
        response.set_content_length();
        response
    }

    pub fn html(status: u16, body: impl Into<String>) -> Self {
        Self::bytes(status, "text/html", body.into().into_bytes())
    }

    pub fn empty(status: u16) -> Self {
        let mut response = Self::new(status);
        response.set_header_static("Content-Length", "0");
        response
    }

    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(header) = self
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(name))
        {
            header.value = value;
        } else {
            self.headers.push(Header {
                name: name.into(),
                value,
            });
        }
    }

    pub fn add_header(&mut self, name: &str, value: impl Into<String>) {
        self.headers.push(Header {
            name: name.into(),
            value: value.into(),
        });
    }

    pub(crate) fn set_header_static(&mut self, name: &'static str, value: impl Into<String>) {
        let value = value.into();
        if let Some(header) = self
            .headers
            .iter_mut()
            .find(|header| header.name.eq_ignore_ascii_case(name))
        {
            header.value = value;
        } else {
            self.headers.push(Header {
                name: HeaderName::from_static(name),
                value,
            });
        }
    }

    pub(crate) fn add_header_static(&mut self, name: &'static str, value: impl Into<String>) {
        self.headers.push(Header {
            name: HeaderName::from_static(name),
            value: value.into(),
        });
    }

    pub fn remove_header(&mut self, name: &str) {
        self.headers
            .retain(|header| !header.name.eq_ignore_ascii_case(name));
    }

    pub(crate) fn enable_ranges(&mut self, single_range: bool) {
        self.allow_ranges = true;
        self.single_range = single_range;
    }

    pub(crate) fn disable_ranges(&mut self) {
        self.allow_ranges = false;
        self.single_range = false;
    }

    pub(crate) fn expect_chunked(&mut self) {
        self.chunked_expected = true;
    }

    pub fn upstream_control(&self) -> Option<&UpstreamControl> {
        self.upstream.as_deref()
    }

    pub fn upstream_control_mut(&mut self) -> &mut UpstreamControl {
        self.upstream.get_or_insert_default().as_mut()
    }

    pub fn take_upstream_redirect(&mut self) -> Option<String> {
        let redirect = self.upstream.as_mut()?.redirect.take();
        if self
            .upstream
            .as_ref()
            .is_some_and(|control| control.is_empty())
        {
            self.upstream = None;
        }
        redirect
    }

    pub fn inherit_upstream_control_to(&self, target: &mut Response) {
        let Some(source) = self.upstream_control() else {
            return;
        };
        if source.is_empty() {
            return;
        }
        if target.early_hints().is_empty() {
            target.set_early_hints(source.early_hints.clone());
        }
        let control = target.upstream_control_mut();
        if control.redirect.is_none() {
            control.redirect = source.redirect.clone();
        }
        if control.limit_rate.is_none() {
            control.limit_rate = source.limit_rate;
        }
        if control.buffering.is_none() {
            control.buffering = source.buffering;
        }
        if control.charset.is_none() {
            control.charset = source.charset.clone();
        }
        if control.headers.is_empty() {
            control.headers = source.headers.clone();
        }
        if control.trailers.is_empty() {
            control.trailers = source.trailers.clone();
        }
    }

    pub fn early_hints(&self) -> &[Vec<Header>] {
        self.upstream
            .as_deref()
            .map(|control| control.early_hints.as_slice())
            .unwrap_or_default()
    }

    pub fn set_early_hints(&mut self, early_hints: Vec<Vec<Header>>) {
        if early_hints.is_empty() {
            return;
        }
        self.upstream_control_mut().early_hints = early_hints;
    }

    pub fn inherit_early_hints_from(&mut self, source: &Response) {
        if self.early_hints().is_empty() {
            self.set_early_hints(source.early_hints().to_vec());
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|header| header.name.eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str())
    }

    pub fn upstream_headers(&self) -> &[Header] {
        self.upstream_control()
            .map(|control| control.headers.as_slice())
            .unwrap_or_default()
    }

    pub fn upstream_trailers(&self) -> &[Header] {
        self.upstream_control()
            .map(|control| control.trailers.as_slice())
            .unwrap_or_default()
    }

    pub fn set_content_length(&mut self) {
        if let Some(length) = self.body.content_length() {
            self.set_header_static("Content-Length", length.to_string());
        } else {
            self.remove_header("Content-Length");
        }
    }

    pub fn body_len(&self) -> u64 {
        self.body.len()
    }

    pub(crate) fn preserve_on_client_abort(&self) -> bool {
        self.body.preserve_on_client_abort()
    }

    pub(crate) fn mark_preserve_on_client_abort(&mut self) {
        self.body.mark_preserve_on_client_abort();
    }

    pub fn into_bytes(self) -> Vec<u8> {
        match self.body {
            ResponseBody::Empty => Vec::new(),
            ResponseBody::Bytes(body) => body,
            ResponseBody::File { .. }
            | ResponseBody::FileWithPrefix { .. }
            | ResponseBody::Segments { .. }
            | ResponseBody::Stream(_) => Vec::new(),
        }
    }
}

pub(crate) fn sanitize_header_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character == '\r' || character == '\n' {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub(crate) fn is_forbidden_trailer_name(name: &str) -> bool {
    [
        "connection",
        "content-encoding",
        "content-length",
        "content-range",
        "content-type",
        "host",
        "keep-alive",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ]
    .into_iter()
    .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

pub fn error_response(status: u16) -> Response {
    let mut response = Response::html(status, tokens::error_page_body(status, ServerTokens::On));
    response.builtin_error = true;
    response
}

pub fn reason_phrase(status: u16) -> &'static str {
    match status {
        100 => "Continue",
        101 => "Switching Protocols",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Moved Temporarily",
        303 => "See Other",
        304 => "Not Modified",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Time-out",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Request Entity Too Large",
        414 => "Request-URI Too Large",
        415 => "Unsupported Media Type",
        416 => "Requested Range Not Satisfiable",
        421 => "Misdirected Request",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Temporarily Unavailable",
        504 => "Gateway Time-out",
        505 => "HTTP Version Not Supported",
        507 => "Insufficient Storage",
        _ => "Unknown Status",
    }
}
