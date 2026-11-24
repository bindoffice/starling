use super::*;

pub(in crate::server) fn record_stream_timing(
    guard: &mut Option<UpstreamPeerGuard>,
    connect: Duration,
    relay: StreamRelayOutcome,
) {
    let Some(guard) = guard.as_mut() else {
        return;
    };
    if !relay.success {
        guard.mark_failed();
        return;
    }
    let first_byte = relay.first_byte.map(|first_byte| connect + first_byte);
    guard.record_timing(UpstreamAttemptTiming {
        connect,
        first_byte,
        header: first_byte.unwrap_or(connect + relay.response),
        response: connect + relay.response,
    });
}

pub(in crate::server) fn update_stream_log_after_relay(
    log: &mut StreamLogContext,
    connect: Duration,
    relay: StreamRelayOutcome,
) {
    log.status = relay.status;
    log.bytes_sent = relay.bytes_sent;
    log.bytes_received = relay.bytes_received;
    log.upstream_bytes_sent = relay.bytes_received;
    log.upstream_bytes_received = relay.bytes_sent;
    log.upstream_connect_time = Some(connect);
    log.upstream_first_byte_time = relay.first_byte.map(|first_byte| connect + first_byte);
    log.upstream_session_time = Some(connect + relay.response);
}

pub(in crate::server) fn log_stream_access(
    config: &RuntimeConfig,
    server: &crate::config::StreamServer,
    log: &StreamLogContext,
) {
    let Some(stream) = &config.stream else {
        return;
    };
    let access_log = &server.access_log;
    if access_log.off {
        return;
    }
    for target in &access_log.logs {
        if let Some(condition) = &target.condition
            && !log_filter_allows(&eval_stream_log_variables(condition, log))
        {
            continue;
        }
        let Some(format) = stream
            .log_formats
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(&target.format_name))
            .map(|(_, format)| format)
        else {
            continue;
        };
        let mut stack = Vec::new();
        let mut line = render_log_format(&format.template, format.escape, |name| {
            Some(eval_stream_variable(name, log, &mut stack))
        });
        line.push('\n');
        write_access_log_target(
            target,
            line.as_bytes(),
            server.open_log_file_cache.as_ref(),
            |template| eval_stream_log_variables(template, log),
        );
    }
}
