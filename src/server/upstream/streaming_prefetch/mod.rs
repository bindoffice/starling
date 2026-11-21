mod chunked;
mod content_length;
mod until_eof;

pub(super) use chunked::{ChunkedPrefetch, prefetch_chunked, spawn_prefetched_chunked_stream};
pub(super) use content_length::{prefetch_content_length, spawn_prefetched_content_length_stream};
pub(super) use until_eof::{
    UntilEofPrefetch, prefetch_until_eof, spawn_prefetched_until_eof_stream,
};
