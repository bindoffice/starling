use super::*;

pub(in crate::server) type HttpErrorLoggers =
    Arc<HashMap<Vec<crate::config::LogTarget>, Arc<Logger>>>;

pub(in crate::server) fn build_http_error_loggers(
    config: &RuntimeConfig,
) -> io::Result<HttpErrorLoggers> {
    let Some(http) = &config.http else {
        return Ok(Arc::new(HashMap::new()));
    };
    let mut targets = HashSet::new();
    if !http.error_logs.is_empty() {
        targets.insert(http.error_logs.clone());
    }
    for server in &http.servers {
        collect_location_error_logs(&server.locations, &mut targets);
        if !server.context.error_logs.is_empty() {
            targets.insert(server.context.error_logs.clone());
        }
    }
    let loggers = targets
        .into_iter()
        .map(|targets| {
            let logger = Arc::new(Logger::new(&targets, &config.prefix)?);
            Ok((targets, logger))
        })
        .collect::<io::Result<HashMap<_, _>>>()?;
    Ok(Arc::new(loggers))
}

fn collect_location_error_logs(
    locations: &[LocationConfig],
    targets: &mut HashSet<Vec<crate::config::LogTarget>>,
) {
    for location in locations {
        if !location.context.error_logs.is_empty() {
            targets.insert(location.context.error_logs.clone());
        }
        collect_location_error_logs(&location.nested, targets);
    }
}

pub(in crate::server) fn http_logger_for_context(
    state: &SharedState,
    http_error_loggers: &HttpErrorLoggers,
    context: &EffectiveConfig,
) -> Arc<Logger> {
    if context.error_logs.is_empty() {
        return state.logger.clone();
    }
    http_error_loggers
        .get(&context.error_logs)
        .cloned()
        .unwrap_or_else(|| state.logger.clone())
}

pub(in crate::server) fn reopen_http_error_loggers(state: &SharedState) {
    for logger in state.config.load().http_error_loggers.values() {
        logger.reopen();
    }
}
