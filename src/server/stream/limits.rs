use super::*;

const LIMIT_CONN_KEY_MAX_LEN: usize = 255;

pub(in crate::server) fn acquire_stream_connection_limits(
    server: &crate::config::StreamServer,
    state: &SharedState,
    log: &mut StreamLogContext,
) -> Result<ConnectionLimitGuard, ()> {
    let mut guard = ConnectionLimitGuard::new(state.limits.clone());
    let mut applied = false;
    for limit in &server.limit_conns {
        let key = eval_stream_log_variables_bytes(&limit.key, log);
        if key.len() > LIMIT_CONN_KEY_MAX_LEN {
            let display_key = String::from_utf8_lossy(&key);
            log.logger.log(
                LogLevel::Error,
                format!(
                    "the value of the \"{}\" key is more than 255 bytes: \"{display_key}\"",
                    limit.key
                ),
            );
            continue;
        }
        if key.is_empty() {
            continue;
        }
        applied = true;
        if guard
            .acquire(&limit.zone, &key, limit.connections)
            .unwrap_or(false)
        {
            continue;
        }

        guard.release_all();
        log.logger.log(
            server.limit_conn_log_level,
            format!(
                "limiting connections{} by zone \"{}\"",
                if server.limit_conn_dry_run {
                    ", dry run,"
                } else {
                    ""
                },
                limit.zone
            ),
        );
        if server.limit_conn_dry_run {
            log.limit_conn_status = "REJECTED_DRY_RUN".into();
            return Ok(guard);
        }
        log.limit_conn_status = "REJECTED".into();
        log.status = 503;
        return Err(());
    }
    if applied {
        log.limit_conn_status = "PASSED".into();
    }
    Ok(guard)
}
