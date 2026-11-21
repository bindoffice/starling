use super::*;

pub(in crate::server) enum LimitReqDecision {
    Passed,
    Delayed(u64),
    Rejected,
}

pub(in crate::server) async fn apply_limit_req(
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Option<Response> {
    if context.limit_reqs.is_empty() {
        return None;
    }
    let now = Instant::now();
    let mut applied = false;
    let mut max_delay = 0_u64;
    let mut dry_run_rejected = false;
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
            LimitReqDecision::Delayed(delay) => max_delay = max_delay.max(delay),
            LimitReqDecision::Rejected if context.limit_req_dry_run => {
                dry_run_rejected = true;
            }
            LimitReqDecision::Rejected => {
                variables.limit_req_status = "REJECTED".into();
                return Some(error_response(context.limit_req_status));
            }
        }
    }
    if applied {
        variables.limit_req_status = if dry_run_rejected {
            "REJECTED_DRY_RUN".into()
        } else if max_delay > 0 {
            "DELAYED".into()
        } else {
            "PASSED".into()
        };
    }
    if max_delay > 0 {
        tokio::time::sleep(Duration::from_millis(max_delay)).await;
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
    let excess = bucket
        .excess
        .saturating_sub(limit.rate_millis.saturating_mul(elapsed) / 1000)
        .saturating_add(1000);
    if excess > limit.burst.saturating_mul(1000) {
        return LimitReqDecision::Rejected;
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
    LimitReqDecision::Delayed(delay)
}

pub(in crate::server) fn acquire_connection_limits(
    context: &EffectiveConfig,
    variables: &mut Variables,
) -> Result<ConnectionLimitGuard, u16> {
    let mut guard = ConnectionLimitGuard::new(variables.limits.clone());
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
            Ok(false) if context.limit_conn_dry_run => {
                dry_run_rejected = true;
            }
            Ok(false) => {
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
    }
    Ok(guard)
}
