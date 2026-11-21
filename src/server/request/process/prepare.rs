use super::*;

pub(super) struct PreparedRequest {
    pub(super) env: RequestEnv,
    pub(super) context: EffectiveConfig,
    pub(super) connection_guard: Option<ConnectionLimitGuard>,
}

pub(super) enum PrepareOutcome {
    Ready(PreparedRequest),
    Response(Processed),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_request(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &mut Request,
    mut env: RequestEnv,
    variables: &mut Variables,
    depth: usize,
    mut run_server_rewrite: bool,
    mut uri_changes: usize,
) -> PrepareOutcome {
    let mut connection_guard = None;
    loop {
        let mut server_context = Box::new(server.context.clone());
        let server_logger = variables.error_logger_for(&server_context);
        variables.set_variable_warning_context(&server_context, server_logger.clone());
        prepare_userid_variables(&server_context, variables);
        refresh_variables(&env, None, variables, &server_context);
        if run_server_rewrite && !server_may_stream_request_body(server) {
            if let Err(error) = buffer_request_body(request, &server_context).await {
                request.keep_alive = false;
                return response_outcome(*server_context, request_body_error_response(&error));
            }
            if let Err(error) =
                validate_declared_request_body_size(request, server_context.client_max_body_size)
                    .and_then(|()| {
                        validate_buffered_request_body_size(
                            request,
                            server_context.client_max_body_size,
                        )
                    })
            {
                request.keep_alive = false;
                return response_outcome(*server_context, request_body_error_response(&error));
            }
            set_buffered_request_body_variable(request, variables);
        }
        if run_server_rewrite {
            match run_script(
                &mut server_context,
                &mut env,
                variables,
                request,
                None,
                server_logger,
            ) {
                RewriteOutcome::Response(response) => {
                    return response_outcome(*server_context, *response);
                }
                RewriteOutcome::Last => {
                    if !consume_uri_change(&mut uri_changes) {
                        return error_outcome(*server_context);
                    }
                }
                RewriteOutcome::Break | RewriteOutcome::None => {}
            }
            refresh_variables(&env, None, variables, &server_context);
        }

        let location = resolve_location(&server.locations, &env.uri);
        set_location_captures(location, &env, variables);
        let mut context = Box::new(
            location
                .as_ref()
                .map(|location| location.context.clone())
                .unwrap_or_else(|| (*server_context).clone()),
        );
        apply_limit_except(&mut context, request);
        let context_logger = variables.error_logger_for(&context);
        variables.set_variable_warning_context(&context, context_logger.clone());
        prepare_userid_variables(&context, variables);
        refresh_variables(&env, location, variables, &context);
        if !request_body_stream_enabled(request, &context) {
            if let Err(error) = buffer_request_body(request, &context).await {
                request.keep_alive = false;
                return response_outcome(*context, request_body_error_response(&error));
            }
            if let Err(error) =
                validate_declared_request_body_size(request, context.client_max_body_size).and_then(
                    |()| validate_buffered_request_body_size(request, context.client_max_body_size),
                )
            {
                request.keep_alive = false;
                return response_outcome(*context, request_body_error_response(&error));
            }
            set_buffered_request_body_variable(request, variables);
        }
        if let Some(response) = pre_rewrite_response(
            http,
            server,
            request,
            depth,
            &mut env,
            &context,
            location,
            variables,
            &mut connection_guard,
        )
        .await
        {
            return response_outcome(*context, response);
        }

        let has_rewrite_script = !context.rewrite_actions.is_empty();
        let access_rules = has_rewrite_script.then(|| context.access_rules.clone());
        let outcome = if has_rewrite_script {
            run_script(
                &mut context,
                &mut env,
                variables,
                request,
                location,
                context_logger,
            )
        } else {
            RewriteOutcome::None
        };
        match outcome {
            RewriteOutcome::Response(response) => {
                return response_outcome(*context, *response);
            }
            RewriteOutcome::Last => {
                if !consume_uri_change(&mut uri_changes) {
                    return error_outcome(*context);
                }
                run_server_rewrite = false;
                variables.uri = env.uri.clone();
                continue;
            }
            RewriteOutcome::Break | RewriteOutcome::None => {}
        }
        if access_rules.is_some_and(|access_rules| context.access_rules != access_rules)
            && !access_allowed(&context.access_rules, &variables.remote_addr)
        {
            return response_outcome(*context, error_response(403));
        }
        refresh_variables(&env, location, variables, &context);

        if !env.bypass_try_files
            && let Some(try_files) = &context.try_files
        {
            match resolve_try_files(try_files, &context, location, &env, variables).await {
                TryFilesResult::Found(uri) => {
                    env.uri = uri;
                    env.unparsed_uri = None;
                    env.bypass_try_files = true;
                }
                TryFilesResult::FallbackStatus(status) => {
                    let response = configured_error_page(
                        http,
                        server,
                        request,
                        error_response(status),
                        &context,
                        variables,
                        depth,
                    )
                    .await;
                    return response_outcome(*context, response);
                }
                TryFilesResult::FallbackUri(uri) => {
                    env.uri = normalize_uri(&uri).unwrap_or_else(|| uri.clone());
                    env.unparsed_uri = None;
                    env.internal = true;
                    env.bypass_try_files = false;
                    if !consume_uri_change(&mut uri_changes) {
                        return error_outcome(*context);
                    }
                    run_server_rewrite = true;
                    continue;
                }
                TryFilesResult::Named(name) => {
                    if let Some(processed) = process_named_location(
                        http,
                        server,
                        request,
                        &name,
                        env,
                        variables,
                        depth,
                        uri_changes,
                    )
                    .await
                    {
                        return PrepareOutcome::Response(processed);
                    }
                    return error_outcome(*context);
                }
            }
        }

        return PrepareOutcome::Ready(PreparedRequest {
            env,
            context: *context,
            connection_guard,
        });
    }
}

#[allow(clippy::too_many_arguments)]
async fn pre_rewrite_response(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    depth: usize,
    env: &mut RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    variables: &mut Variables,
    connection_guard: &mut Option<ConnectionLimitGuard>,
) -> Option<Response> {
    if let Err(error) = validate_declared_request_body_size(request, context.client_max_body_size) {
        return Some(request_body_error_response(&error));
    }
    if context.client_max_body_size != 0
        && request.buffered_body_len() > context.client_max_body_size
    {
        return Some(error_response(413));
    }
    if location.is_some_and(|location| location.context.internal) && !env.internal {
        return Some(error_response(404));
    }
    if let Some(response) = basic_access_response(context, request, variables).await {
        return Some(apply_auth_delay(context, response).await);
    }
    if !env.limits_checked {
        env.limits_checked = true;
        if let Some(response) = apply_limit_req(context, variables).await {
            return Some(
                configured_error_page(http, server, request, response, context, variables, depth)
                    .await,
            );
        }
        match acquire_connection_limits(context, variables) {
            Ok(guard) => *connection_guard = Some(guard),
            Err(status) => {
                return Some(
                    configured_error_page(
                        http,
                        server,
                        request,
                        error_response(status),
                        context,
                        variables,
                        depth,
                    )
                    .await,
                );
            }
        }
    }
    if !env.auth_request_checked
        && let Some(response) =
            auth_request_response(http, server, request, context, variables, depth).await
    {
        let response = apply_auth_delay(context, response).await;
        return Some(
            configured_error_page(http, server, request, response, context, variables, depth).await,
        );
    }
    None
}

fn run_script(
    context: &mut EffectiveConfig,
    env: &mut RequestEnv,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
    logger: Option<Arc<Logger>>,
) -> RewriteOutcome {
    let actions = std::mem::take(&mut context.rewrite_actions);
    let outcome =
        apply_rewrite_script(&actions, env, context, variables, request, location, logger);
    context.rewrite_actions = actions;
    outcome
}

fn set_location_captures(
    location: Option<&LocationConfig>,
    env: &RequestEnv,
    variables: &mut Variables,
) {
    if let Some(location) = location
        && let LocationMatcher::Regex(regex) = &location.matcher
        && let Some(captures) = regex.captures(&env.uri)
    {
        set_regex_captures(variables, regex, &captures);
    }
}

fn response_outcome(context: EffectiveConfig, response: Response) -> PrepareOutcome {
    PrepareOutcome::Response(Processed {
        response,
        context,
        tunnel: false,
    })
}

fn error_outcome(context: EffectiveConfig) -> PrepareOutcome {
    response_outcome(context, error_response(500))
}
