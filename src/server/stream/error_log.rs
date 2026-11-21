use super::*;

pub(in crate::server) fn build_stream_loggers(
    config: &RuntimeConfig,
) -> io::Result<HashMap<usize, Arc<Logger>>> {
    let Some(stream) = &config.stream else {
        return Ok(HashMap::new());
    };
    let mut loggers = HashMap::new();
    for server in &stream.servers {
        if server.error_logs.is_empty() {
            continue;
        }
        loggers.insert(
            server.id,
            Arc::new(Logger::new(&server.error_logs, &config.prefix)?),
        );
    }
    Ok(loggers)
}

pub(in crate::server) async fn stream_logger(
    state: &SharedState,
    server: &crate::config::StreamServer,
) -> Arc<Logger> {
    state
        .stream_loggers
        .read()
        .await
        .get(&server.id)
        .cloned()
        .unwrap_or_else(|| state.logger.clone())
}

pub(in crate::server) async fn stream_route_logger(
    state: &SharedState,
    route: &StreamRoute,
) -> Arc<Logger> {
    let server_id = {
        let config = state.config.read().await;
        config
            .stream
            .as_ref()
            .and_then(|stream| stream_server_for_route(stream, route))
            .map(|server| server.id)
    };
    let Some(server_id) = server_id else {
        return state.logger.clone();
    };
    state
        .stream_loggers
        .read()
        .await
        .get(&server_id)
        .cloned()
        .unwrap_or_else(|| state.logger.clone())
}

pub(in crate::server) async fn reopen_stream_loggers(state: &SharedState) {
    for logger in state.stream_loggers.read().await.values() {
        logger.reopen();
    }
}
