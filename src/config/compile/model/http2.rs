#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Http2Limits {
    pub max_concurrent_streams: u32,
    pub body_preread_size: u32,
    pub max_header_list_size: u32,
    pub chunk_size: usize,
}

impl Default for Http2Limits {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 128,
            body_preread_size: 64 * 1024,
            // Nginx defaults large_client_header_buffers to 4 x 8k.
            max_header_list_size: 32 * 1024,
            chunk_size: 8 * 1024,
        }
    }
}
