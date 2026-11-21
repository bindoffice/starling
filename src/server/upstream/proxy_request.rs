use crate::http::RequestBodyStream;

#[derive(Debug)]
pub(in crate::server) struct ProxyRequestMessage {
    pub(in crate::server) method: String,
    pub(in crate::server) path: String,
    pub(in crate::server) headers: Vec<(String, String)>,
    pub(in crate::server) body: Vec<u8>,
    pub(in crate::server) body_stream: Option<RequestBodyStream>,
    pub(in crate::server) body_chunked: bool,
    pub(in crate::server) body_limit: u64,
}
