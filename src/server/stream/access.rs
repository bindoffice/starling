use super::*;

pub(in crate::server) fn reject_stream_access(
    server: &crate::config::StreamServer,
    remote: &ListenAddress,
    log: &mut StreamLogContext,
) -> bool {
    if access_allowed(&server.access_rules, &remote.variable_address()) {
        return false;
    }
    log.status = 403;
    log.logger.log(
        LogLevel::Error,
        format!("stream access forbidden by rule from {remote}"),
    );
    true
}
