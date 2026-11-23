use super::*;

pub(in crate::server) fn apply_rewrite_script(
    actions: &[RewriteAction],
    env: &mut RequestEnv,
    context: &mut EffectiveConfig,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
    logger: Option<Arc<Logger>>,
) -> RewriteOutcome {
    let mut uri_changed = false;
    let outcome = execute_actions(
        actions,
        env,
        context,
        variables,
        request,
        location,
        logger,
        &mut uri_changed,
    );
    if !matches!(outcome, RewriteOutcome::None) {
        return outcome;
    }
    if uri_changed {
        RewriteOutcome::Last
    } else {
        RewriteOutcome::None
    }
}

#[cfg(test)]
pub(in crate::server) fn apply_conditional(
    conditional: &ConditionalConfig,
    env: &mut RequestEnv,
    context: &mut EffectiveConfig,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
) -> RewriteOutcome {
    if !condition_matches(&conditional.condition, context, variables) {
        return RewriteOutcome::None;
    }
    apply_conditional_config(conditional, context);
    let mut uri_changed = false;
    let outcome = execute_actions(
        &conditional.actions,
        env,
        context,
        variables,
        request,
        location,
        None,
        &mut uri_changed,
    );
    if !matches!(outcome, RewriteOutcome::None) {
        return outcome;
    }
    if uri_changed {
        RewriteOutcome::Last
    } else {
        RewriteOutcome::None
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_actions(
    actions: &[RewriteAction],
    env: &mut RequestEnv,
    context: &mut EffectiveConfig,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
    logger: Option<Arc<Logger>>,
    uri_changed: &mut bool,
) -> RewriteOutcome {
    let previous_logger = std::mem::replace(&mut variables.rewrite_logger, logger.clone());
    let previous_rewrite_log = variables.rewrite_log;
    let previous_uninitialized_warn = variables.uninitialized_variable_warn;
    variables.rewrite_log = context.rewrite_log;
    variables.uninitialized_variable_warn = context.uninitialized_variable_warn;
    let outcome = execute_actions_inner(
        actions,
        env,
        context,
        variables,
        request,
        location,
        logger,
        uri_changed,
    );
    variables.rewrite_logger = previous_logger;
    variables.rewrite_log = previous_rewrite_log;
    variables.uninitialized_variable_warn = previous_uninitialized_warn;
    outcome
}

#[allow(clippy::too_many_arguments)]
fn execute_actions_inner(
    actions: &[RewriteAction],
    env: &mut RequestEnv,
    context: &mut EffectiveConfig,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
    logger: Option<Arc<Logger>>,
    uri_changed: &mut bool,
) -> RewriteOutcome {
    for action in actions {
        match action {
            RewriteAction::Set(set) => {
                let value = eval_variables(&set.value, variables);
                variables.custom.insert(set.name.clone(), value);
            }
            RewriteAction::Rewrite(rule) => {
                let old_uri = env.uri.clone();
                let old_args = env.args.clone();
                if let Some(outcome) = rule::apply_rule(rule, env, variables, uri_changed) {
                    return outcome;
                }
                if env.uri != old_uri || env.args != old_args {
                    refresh_variables(env, location, variables, context);
                }
            }
            RewriteAction::Return(rule) => {
                return RewriteOutcome::Response(Box::new(apply_return(
                    rule.status,
                    rule.value.as_deref(),
                    variables,
                    request,
                    context,
                )));
            }
            RewriteAction::Break => {
                if *uri_changed {
                    env.valid_location = false;
                }
                *uri_changed = false;
                return RewriteOutcome::Break;
            }
            RewriteAction::If(conditional) => {
                if !condition_matches(&conditional.condition, context, variables) {
                    continue;
                }
                apply_conditional_config(conditional, context);
                match execute_actions(
                    &conditional.actions,
                    env,
                    context,
                    variables,
                    request,
                    location,
                    logger.clone(),
                    uri_changed,
                ) {
                    RewriteOutcome::None => {}
                    outcome => return outcome,
                }
            }
        }
    }
    RewriteOutcome::None
}

fn apply_conditional_config(conditional: &ConditionalConfig, context: &mut EffectiveConfig) {
    // A matched "if" installs its complete compile-time loc_conf snapshot.
    // Inherited values therefore overwrite configuration from earlier branches.
    context.add_headers.clone_from(&conditional.add_headers);
    context.add_header_inherit = conditional.add_header_inherit;
    context.add_trailers.clone_from(&conditional.add_trailers);
    context.add_trailer_inherit = conditional.add_trailer_inherit;
    context.access_rules.clone_from(&conditional.access_rules);
    context.expires.clone_from(&conditional.expires);
    context.ssi.clone_from(&conditional.ssi);
    context.gzip = conditional.gzip;
    context.charset.clone_from(&conditional.charset);
    context.access_log.clone_from(&conditional.access_log);
    context.proxy_pass.clone_from(&conditional.proxy_pass);
    context.tunnel.pass.clone_from(&conditional.tunnel_pass);
    context.grpc.pass.clone_from(&conditional.grpc_pass);
    context.fastcgi_pass.clone_from(&conditional.fastcgi_pass);
    context.uwsgi_pass.clone_from(&conditional.uwsgi_pass);
    context.scgi_pass.clone_from(&conditional.scgi_pass);
    context
        .memcached_pass
        .clone_from(&conditional.memcached_pass);
    context.root.clone_from(&conditional.root);
    context.alias.clone_from(&conditional.alias);
    context.sendfile = conditional.sendfile;
    context.limit_rate.clone_from(&conditional.limit_rate);
    context
        .limit_rate_after
        .clone_from(&conditional.limit_rate_after);
    context.error_pages.clone_from(&conditional.error_pages);
    context.post_action.clone_from(&conditional.post_action);
    context.rewrite_log = conditional.rewrite_log;
    context.uninitialized_variable_warn = conditional.uninitialized_variable_warn;
}
