use super::*;

pub(in crate::server) async fn auto_https_manager_loop(
    state: SharedState,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut failure_delay = Duration::ZERO;
    let mut reloads = state.reload_receiver();
    let mut refresh_needed = false;
    loop {
        let mut all_succeeded = renew_pending_certificates(&state, &mut refresh_needed).await;
        if refresh_needed {
            if refresh_tls_configs(&state).await {
                refresh_needed = false;
            } else {
                all_succeeded = false;
            }
        }
        let retry_delay = if all_succeeded {
            failure_delay = Duration::ZERO;
            state.auto_https.config().check_interval
        } else {
            failure_delay = if failure_delay.is_zero() {
                Duration::from_secs(5 * 60)
            } else {
                (failure_delay * 2).min(Duration::from_secs(60 * 60))
            };
            failure_delay
        };
        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    break;
                }
            }
            changed = reloads.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            _ = tokio::time::sleep(retry_delay) => {}
        }
    }
}

async fn renew_pending_certificates(state: &SharedState, refresh_needed: &mut bool) -> bool {
    let config = state.auto_https.config();
    if !config.enabled {
        return true;
    }
    let mut all_succeeded = true;
    for order in state.auto_https.orders() {
        if !order_needs_certificate(&order, &config) {
            continue;
        }
        let key = order.paths.cert.to_string_lossy().to_string();
        if !state.auto_https.begin_order(&key) {
            continue;
        }
        let lock_path = order.paths.cert.with_extension("acme.lock");
        let _lock = match CertificateLock::acquire(&lock_path, Duration::from_millis(100)) {
            Ok(lock) => lock,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                state.auto_https.finish_order(&key);
                all_succeeded = false;
                continue;
            }
            Err(error) => {
                state.auto_https.finish_order(&key);
                state.logger.log(
                    LogLevel::Error,
                    format!("automatic HTTPS certificate lock failed: {error}"),
                );
                all_succeeded = false;
                continue;
            }
        };
        state.logger.log(
            LogLevel::Notice,
            format!(
                "requesting automatic HTTPS certificate for {} ({})",
                order.domains.join(", "),
                if order.staging {
                    "staging"
                } else {
                    "production"
                }
            ),
        );
        let result = obtain_certificate(&order, &state.auto_https, Some(&state.logger)).await;
        state.auto_https.finish_order(&key);
        match result {
            Ok(()) => {
                state.logger.log(
                    LogLevel::Notice,
                    format!(
                        "automatic HTTPS certificate installed for {}",
                        order.domains.join(", ")
                    ),
                );
                *refresh_needed = true;
            }
            Err(error) => {
                all_succeeded = false;
                state.logger.log(
                    LogLevel::Error,
                    format!(
                        "automatic HTTPS certificate request for {} failed: {error}",
                        order.domains.join(", ")
                    ),
                );
            }
        }
    }
    all_succeeded
}

fn order_needs_certificate(order: &AcmeOrder, config: &AutoHttpsConfig) -> bool {
    order.paths.source() != Some(CertificateSource::Acme)
        || !order.paths.is_usable(&order.domains, config.renew_before)
}

async fn refresh_tls_configs(state: &SharedState) -> bool {
    let _reload_guard = state.tls_reload_lock.lock().await;
    let snapshot = state.config_snapshot();
    let groups = build_listener_groups(&snapshot.config);
    match build_tls_configs(&snapshot.config, &groups) {
        Ok(configs) => {
            if let Err(error) =
                install_http3_server_configs(state, &snapshot.config, &groups, &configs)
            {
                state.logger.log(
                    LogLevel::Error,
                    format!("failed to refresh HTTP/3 automatic HTTPS configuration: {error}"),
                );
                return false;
            }
            *state.tls_configs.write().await = configs;
            true
        }
        Err(error) => {
            state.logger.log(
                LogLevel::Error,
                format!("failed to reload automatic HTTPS certificates: {error}"),
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_certificates_do_not_suppress_acme_orders() {
        let directory = std::env::temp_dir().join(format!(
            "starling-auto-https-manager-{}-{}",
            std::process::id(),
            AUTO_HTTPS_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let domains = vec!["public.starling.dev".to_string()];
        let paths = ensure_local_certificate(&directory, &domains, Duration::from_secs(1)).unwrap();
        let order = AcmeOrder {
            domains: domains.clone(),
            paths: paths.clone(),
            staging: false,
            directory_url: AutoHttpsConfig::new(&directory)
                .acme_directory_url()
                .to_string(),
            ca_file: None,
            email: None,
            challenge: AutoHttpsChallenge::Http01,
            dns_hook: None,
            dns_propagation_delay: Duration::from_secs(30),
        };
        let mut config = AutoHttpsConfig::new(&directory);
        assert!(order_needs_certificate(&order, &config));

        write_acme_certificate(
            &paths,
            &std::fs::read(&paths.cert).unwrap(),
            &std::fs::read(&paths.key).unwrap(),
        )
        .unwrap();
        assert!(!order_needs_certificate(&order, &config));

        config.renew_before = Duration::from_secs(100 * 24 * 60 * 60);
        assert!(order_needs_certificate(&order, &config));

        config.renew_before = Duration::from_secs(1);
        std::fs::remove_file(&paths.key).unwrap();
        assert!(order_needs_certificate(&order, &config));
        let _ = std::fs::remove_dir_all(directory);
    }
}
