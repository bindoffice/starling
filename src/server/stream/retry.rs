use super::*;

pub(in crate::server) fn append_stream_upstream_addr(log: &mut StreamLogContext, address: &str) {
    if !log.upstream_addr.is_empty() {
        log.upstream_addr.push_str(", ");
    }
    log.upstream_addr.push_str(address);
}

pub(in crate::server) fn can_retry_stream_upstream(
    server: &crate::config::StreamServer,
    started: Instant,
    attempts: usize,
) -> bool {
    server.proxy_next_upstream
        && (server.proxy_next_upstream_tries == 0 || attempts < server.proxy_next_upstream_tries)
        && (server.proxy_next_upstream_timeout.is_zero()
            || started.elapsed() < server.proxy_next_upstream_timeout)
}
