use super::*;

pub(in crate::server) enum LimitReqDecision {
    Passed,
    Delayed { delay: u64, excess: u64 },
    Rejected { excess: u64 },
}

pub(in crate::server) async fn apply_limit_req(
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Option<Response> {
    if context.limit_reqs.is_empty() {
        return None;
    }
    let now = Instant::now();
    let logger = variables.error_logger_for(context);
    let mut applied = false;
    let mut max_delay: Option<(u64, u64, &str)> = None;
    for limit in &context.limit_reqs {
        let key = eval_variables_bytes(&limit.key, variables);
        if key.is_empty() {
            continue;
        }
        if key.len() > 65_535 {
            continue;
        }
        applied = true;
        let Ok(mut state) = variables.limits.lock() else {
            return Some(error_response(500));
        };
        let decision = consume_request_limit(&mut state, limit, &key, now);
        drop(state);
        match decision {
            LimitReqDecision::Passed => {}
            LimitReqDecision::Delayed { delay, excess } => {
                if delay > 0 && max_delay.is_none_or(|(current, _, _)| delay >= current) {
                    max_delay = Some((delay, excess, &limit.zone));
                }
            }
            LimitReqDecision::Rejected { excess } => {
                log_limit_req(
                    logger.as_ref(),
                    context.limit_req_log_level,
                    "limiting requests",
                    excess,
                    &limit.zone,
                    context.limit_req_dry_run,
                );
                if context.limit_req_dry_run {
                    variables.limit_req_status = "REJECTED_DRY_RUN".into();
                    return None;
                }
                variables.limit_req_status = "REJECTED".into();
                return Some(error_response(context.limit_req_status));
            }
        }
    }
    if applied {
        variables.limit_req_status = if let Some((delay, excess, zone)) = max_delay {
            log_limit_req(
                logger.as_ref(),
                limit_req_delay_log_level(context.limit_req_log_level),
                "delaying request",
                excess,
                zone,
                context.limit_req_dry_run,
            );
            if context.limit_req_dry_run {
                "DELAYED_DRY_RUN".into()
            } else {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                "DELAYED".into()
            }
        } else {
            "PASSED".into()
        };
    }
    None
}

pub(in crate::server) fn consume_request_limit(
    state: &mut LimitState,
    limit: &LimitReqLimit,
    key: &[u8],
    now: Instant,
) -> LimitReqDecision {
    let state_key = (limit.zone.clone(), key.to_vec());
    let Some(bucket) = state.requests.get_mut(&state_key) else {
        state.requests.insert(
            state_key,
            RequestBucket {
                last: now,
                excess: 0,
            },
        );
        return LimitReqDecision::Passed;
    };
    let elapsed = now.saturating_duration_since(bucket.last).as_millis() as u64;
    let decayed = (limit.rate_millis as i128) * (elapsed as i128) / 1000;
    let excess = ((bucket.excess as i128) - decayed + 1000)
        .max(0)
        .min(u64::MAX as i128) as u64;
    if excess > limit.burst.saturating_mul(1000) {
        return LimitReqDecision::Rejected { excess };
    }
    bucket.excess = excess;
    if elapsed > 0 {
        bucket.last = now;
    }
    let delay_threshold = limit.delay.saturating_mul(1000);
    if limit.nodelay || excess <= delay_threshold {
        return LimitReqDecision::Passed;
    }
    let delay =
        excess.saturating_sub(delay_threshold).saturating_mul(1000) / limit.rate_millis.max(1);
    if delay == 0 {
        LimitReqDecision::Passed
    } else {
        LimitReqDecision::Delayed { delay, excess }
    }
}

pub(in crate::server) fn acquire_connection_limits(
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Result<ConnectionLimitGuard, u16> {
    let socket = variables
        .client_abort
        .as_ref()
        .and_then(ClientAbortState::socket);
    let mut guard = ConnectionLimitGuard::new(variables.limits.clone(), socket);
    let logger = variables.error_logger_for(context);
    let mut applied = false;
    let mut dry_run_rejected = false;
    for limit in &context.limit_conns {
        let key = eval_variables_bytes(&limit.key, variables);
        if key.is_empty() {
            continue;
        }
        if key.len() > 255 {
            continue;
        }
        applied = true;
        match guard.acquire(&limit.zone, &key, limit.connections) {
            Ok(true) => {}
            Ok(false) => {
                if let Some(logger) = &logger {
                    logger.log(
                        context.limit_conn_log_level,
                        format!(
                            "limiting connections{} by zone \"{}\"",
                            if context.limit_conn_dry_run {
                                ", dry run,"
                            } else {
                                ""
                            },
                            limit.zone
                        ),
                    );
                }
                if context.limit_conn_dry_run {
                    dry_run_rejected = true;
                    guard.release_all();
                    break;
                }
                variables.limit_conn_status = "REJECTED".into();
                return Err(context.limit_conn_status);
            }
            Err(()) => return Err(500),
        }
    }
    if applied {
        variables.limit_conn_status = if dry_run_rejected {
            "REJECTED_DRY_RUN".into()
        } else {
            "PASSED".into()
        };
        if let Some(client_abort) = variables.client_abort.as_ref() {
            client_abort.set_connection_limit(guard.handle());
        }
    }
    Ok(guard)
}

fn limit_req_delay_log_level(level: LogLevel) -> LogLevel {
    match level {
        LogLevel::Debug | LogLevel::Info => LogLevel::Info,
        LogLevel::Notice => LogLevel::Warn,
        LogLevel::Warn => LogLevel::Error,
        LogLevel::Error => LogLevel::Crit,
        LogLevel::Crit => LogLevel::Alert,
        LogLevel::Alert | LogLevel::Emerg => LogLevel::Emerg,
    }
}

fn log_limit_req(
    logger: Option<&Arc<Logger>>,
    level: LogLevel,
    message: &str,
    excess: u64,
    zone: &str,
    dry_run: bool,
) {
    if let Some(logger) = logger {
        logger.log(
            level,
            format!(
                "{message}{}, excess: {}.{:03}{} by zone \"{zone}\"",
                if dry_run { ", dry run" } else { "" },
                excess / 1000,
                excess % 1000,
                if message == "limiting requests" {
                    ""
                } else {
                    ","
                },
            ),
        );
    }
}
