use super::*;

pub async fn run(
    config: RuntimeConfig,
    logger: Arc<Logger>,
    worker_layout: Vec<usize>,
    blocking_thread_limit: usize,
) -> io::Result<()> {
    cleanup_stale_temp_files(&config, &logger);
    for warning in &config.warnings {
        logger.log(LogLevel::Warn, warning.to_string());
    }
    let config_path = config.config_path.clone();
    let prefix = config.prefix.clone();
    let config = Arc::new(config);
    let worker_shutdown_timeout = config
        .core
        .worker_shutdown_timeout
        .filter(|timeout| !timeout.is_zero());
    let groups = build_listener_groups(&config);
    let listener_groups: Arc<[ListenerGroup]> = groups.clone().into();
    let tls_configs = build_tls_configs(&config, &groups)?;
    let stream_tls_configs = build_stream_tls_configs(&config)?;
    let http_error_loggers = build_http_error_loggers(&config)?;
    let stream_loggers = build_stream_loggers(&config)?;
    let proxy_cache = Arc::new(ProxyCacheRegistry::new(config.http.as_ref()));
    let upstream_keepalive = Arc::new(UpstreamKeepalivePool::default());
    let (reload_tx, _) = watch::channel(0_u64);
    let state = SharedState {
        config: Arc::new(ArcSwap::from_pointee(RuntimeSnapshot {
            config,
            listener_groups,
            http_error_loggers,
            upstream_keepalive_generation: upstream_keepalive.generation(),
        })),
        reload: Arc::new(reload_tx),
        tls_configs: Arc::new(RwLock::new(tls_configs)),
        stream_tls_configs: Arc::new(RwLock::new(stream_tls_configs)),
        logger,
        stream_loggers: Arc::new(RwLock::new(stream_loggers)),
        limits: Arc::new(Mutex::new(LimitState::default())),
        stats: Arc::new(ConnectionStats::default()),
        upstream_keepalive,
        proxy_cache,
        client_tls_cache: Arc::new(ClientTlsConfigCache::default()),
        http2_budget: Arc::new(Http2Budget::new()),
    };
    let stream_routes = {
        let config = state.current_config();
        build_stream_routes(&config)
    };
    let no_http_listeners = groups.is_empty();
    let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownMode::Running);
    let (background_shutdown_tx, background_shutdown_rx) = watch::channel(false);
    let mut tasks = JoinSet::new();
    let worker_pool = WorkerPool::start(
        groups,
        stream_routes,
        state.clone(),
        shutdown_rx.clone(),
        shutdown_tx.clone(),
        worker_layout,
        blocking_thread_limit,
    )
    .await?;

    if no_http_listeners {
        state
            .logger
            .log(LogLevel::Notice, "no HTTP listening sockets configured");
    }
    {
        let cache = state.proxy_cache.clone();
        let logger = state.logger.clone();
        let shutdown = background_shutdown_rx.clone();
        tasks.spawn(async move {
            cache.manage_loop(logger, shutdown).await;
        });
    }
    {
        let cache = state.proxy_cache.clone();
        let logger = state.logger.clone();
        let shutdown = background_shutdown_rx.clone();
        tasks.spawn(async move {
            cache.loader_loop(logger, shutdown).await;
        });
    }

    tasks.spawn(async move {
        worker_pool.wait().await;
    });

    state.logger.log(LogLevel::Notice, "start worker process");

    let shutdown_mode;
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate())?;
        let mut quit = signal(SignalKind::quit())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut hup = signal(SignalKind::hangup())?;
        let mut user1 = signal(SignalKind::user_defined1())?;
        loop {
            tokio::select! {
                _ = term.recv() => {
                    state.logger.log(LogLevel::Notice, "signal 15 (SIGTERM) received, exiting");
                    shutdown_mode = ShutdownMode::Fast;
                    break;
                }
                _ = quit.recv() => {
                    state.logger.log(LogLevel::Notice, "signal 3 (SIGQUIT) received, shutting down");
                    shutdown_mode = ShutdownMode::Graceful;
                    break;
                }
                _ = interrupt.recv() => {
                    state.logger.log(LogLevel::Notice, "signal 2 (SIGINT) received, exiting");
                    shutdown_mode = ShutdownMode::Fast;
                    break;
                }
                _ = hup.recv() => {
                    reload(&state, &config_path, &prefix).await;
                }
                _ = user1.recv() => {
                    reopen_access_logs();
                    state.logger.reopen();
                    reopen_http_error_loggers(&state);
                    reopen_stream_loggers(&state).await;
                    state.logger.log(LogLevel::Notice, "reopening logs");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_mode = ShutdownMode::Fast;
    }

    let _ = shutdown_tx.send(shutdown_mode);
    let _ = background_shutdown_tx.send(true);
    let graceful_timeout = (shutdown_mode == ShutdownMode::Graceful)
        .then_some(worker_shutdown_timeout)
        .flatten();
    let log_task_error = |err| {
        state
            .logger
            .log(LogLevel::Error, format!("listener task failed: {err}"));
    };
    if drain_tasks(&mut tasks, graceful_timeout, log_task_error).await == TaskDrain::TimedOut {
        state.logger.log(
            LogLevel::Notice,
            format!(
                "worker_shutdown_timeout {}ms expired, forcing shutdown",
                worker_shutdown_timeout.unwrap_or_default().as_millis()
            ),
        );
        let _ = shutdown_tx.send(ShutdownMode::Fast);
        while let Some(result) = tasks.join_next().await {
            if let Err(err) = result {
                state
                    .logger
                    .log(LogLevel::Error, format!("listener task failed: {err}"));
            }
        }
    }
    flush_access_logs();
    state.logger.log(LogLevel::Notice, "exit");
    Ok(())
}

pub(in crate::server) async fn reload(state: &SharedState, config_path: &Path, prefix: &Path) {
    use crate::config::{ConfigLoader, compile};

    let loaded = ConfigLoader::new(prefix.to_path_buf()).load(config_path);
    let loaded = match loaded {
        Ok(document) => compile(document, prefix.to_path_buf(), config_path.to_path_buf()),
        Err(err) => Err(err),
    };
    let new_config = match loaded {
        Ok(config) => Arc::new(config),
        Err(err) => {
            state
                .logger
                .log(LogLevel::Error, format!("reload failed: {err}"));
            return;
        }
    };
    let old = state.current_config();
    if listener_signature(&old) != listener_signature(&new_config) {
        state.logger.log(
            LogLevel::Error,
            "reload failed: changing listen sockets requires a restart",
        );
        return;
    }
    let groups = build_listener_groups(&new_config);
    let tls_configs = match build_tls_configs(&new_config, &groups) {
        Ok(configs) => configs,
        Err(err) => {
            state
                .logger
                .log(LogLevel::Error, format!("reload failed: {err}"));
            return;
        }
    };
    let stream_tls_configs = match build_stream_tls_configs(&new_config) {
        Ok(configs) => configs,
        Err(err) => {
            state
                .logger
                .log(LogLevel::Error, format!("reload failed: {err}"));
            return;
        }
    };
    let stream_loggers = match build_stream_loggers(&new_config) {
        Ok(loggers) => loggers,
        Err(err) => {
            state.logger.log(
                LogLevel::Error,
                format!("reload failed: cannot open new stream error log: {err}"),
            );
            return;
        }
    };
    let http_error_loggers = match build_http_error_loggers(&new_config) {
        Ok(loggers) => loggers,
        Err(err) => {
            state.logger.log(
                LogLevel::Error,
                format!("reload failed: cannot open new HTTP error log: {err}"),
            );
            return;
        }
    };
    if let Err(err) = state
        .logger
        .reconfigure(&new_config.core.error_logs, prefix)
    {
        state.logger.log(
            LogLevel::Error,
            format!("reload failed: cannot open new error log: {err}"),
        );
        return;
    }
    reopen_access_logs();
    *state.tls_configs.write().await = tls_configs;
    *state.stream_tls_configs.write().await = stream_tls_configs;
    *state.stream_loggers.write().await = stream_loggers;
    state.upstream_keepalive.clear().await;
    state.proxy_cache.reload(new_config.http.as_ref());
    state.client_tls_cache.clear();
    let warnings = new_config.warnings.clone();
    let generation = state.upstream_keepalive.generation();
    state.config.store(Arc::new(RuntimeSnapshot {
        config: new_config,
        listener_groups: groups.into(),
        http_error_loggers,
        upstream_keepalive_generation: generation,
    }));
    state.notify_reload();
    for warning in warnings {
        state.logger.log(LogLevel::Warn, warning.to_string());
    }
    state.logger.log(
        LogLevel::Notice,
        format!("configuration {} reloaded", config_path.display()),
    );
}
