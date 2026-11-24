use super::*;

pub(super) struct PreparedRequest {
    pub(super) env: RequestEnv,
    pub(super) context: Arc<EffectiveConfig>,
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
    connection_guard: &mut Option<ConnectionLimitGuard>,
) -> PrepareOutcome {
    loop {
        env.add_uri_to_alias = false;
        let mut server_context = server.context.clone();
        let server_logger = variables.error_logger_for(&server_context);
        variables.set_variable_warning_context(&server_context, server_logger.clone());
        prepare_userid_variables(&server_context, variables);
        refresh_variables(&env, None, variables, &server_context);
        if run_server_rewrite
            && !server_context.rewrite_actions.is_empty()
            && !server_may_stream_request_body(server)
        {
            if let Err(error) = buffer_request_body(request, &server_context).await {
                request.keep_alive = false;
                return response_outcome(
                    server_context.clone(),
                    request_body_error_response(&error),
                );
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
                return response_outcome(
                    server_context.clone(),
                    request_body_error_response(&error),
                );
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
                    return rewrite_response_outcome(
                        http,
                        server,
                        request,
                        *response,
                        server_context.clone(),
                        variables,
                        depth,
                    )
                    .await;
                }
                RewriteOutcome::Last => {
                    if !consume_uri_change(&mut uri_changes) {
                        return error_outcome(server_context.clone());
                    }
                }
                RewriteOutcome::Break | RewriteOutcome::None => {}
            }
            refresh_variables(&env, None, variables, &server_context);
        }

        let location = resolve_location(&server.locations, &env.uri);
        if let Some((context, response)) =
            location_auto_redirect(&server.locations, &env.uri, env.args.as_deref())
        {
            return response_outcome(context, response);
        }
        set_location_captures(location, &env, variables);
        let mut context = location
            .as_ref()
            .map(|location| location.context.clone())
            .unwrap_or_else(|| server_context.clone());
        apply_limit_except(&mut context, request);
        variables.content_handler = content_handler_for_context(&context);
        let context_logger = variables.error_logger_for(&context);
        variables.set_variable_warning_context(&context, context_logger.clone());
        prepare_userid_variables(&context, variables);
        refresh_variables(&env, location, variables, &context);
        if !request_body_stream_enabled(request, &context)
            && should_buffer_request_body(request, &context)
        {
            if let Err(error) = buffer_request_body(request, &context).await {
                request.keep_alive = false;
                return response_outcome(context.clone(), request_body_error_response(&error));
            }
            if let Err(error) =
                validate_declared_request_body_size(request, context.client_max_body_size).and_then(
                    |()| validate_buffered_request_body_size(request, context.client_max_body_size),
                )
            {
                request.keep_alive = false;
                return response_outcome(context.clone(), request_body_error_response(&error));
            }
            set_buffered_request_body_variable(request, variables);
        }
        if let Some(processed) = Box::pin(pre_rewrite_response(
            http,
            server,
            request,
            depth,
            &mut env,
            &context,
            location,
            variables,
            connection_guard,
        ))
        .await
        {
            return PrepareOutcome::Response(processed);
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
                return rewrite_response_outcome(
                    http,
                    server,
                    request,
                    *response,
                    context.clone(),
                    variables,
                    depth,
                )
                .await;
            }
            RewriteOutcome::Last => {
                if !consume_uri_change(&mut uri_changes) {
                    return error_outcome(context.clone());
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
            return response_outcome(context.clone(), error_response(403));
        }
        refresh_variables(&env, location, variables, &context);

        if !env.bypass_try_files
            && let Some(try_files) = &context.try_files
        {
            match resolve_try_files(try_files, &context, location, &env, variables).await {
                TryFilesResult::Found { uri, is_dir } => {
                    env.uri = uri;
                    env.unparsed_uri = None;
                    env.bypass_try_files = true;
                    env.add_uri_to_alias =
                        !is_dir && matches!(context.alias_uri_prefix.as_deref(), Some(""));
                }
                TryFilesResult::FallbackStatus(status) => {
                    let outcome = configured_error_page(
                        http,
                        server,
                        request,
                        error_response(status),
                        &context,
                        variables,
                        depth,
                    )
                    .await;
                    return PrepareOutcome::Response(Processed {
                        response: outcome.response,
                        context: outcome.context,
                        tunnel: false,
                        upgrade: None,
                    });
                }
                TryFilesResult::FallbackUri(uri) => {
                    apply_try_files_fallback_uri(&mut env, &uri);
                    if !consume_uri_change(&mut uri_changes) {
                        return error_outcome(context.clone());
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
                        connection_guard,
                    )
                    .await
                    {
                        return PrepareOutcome::Response(processed);
                    }
                    return error_outcome(context.clone());
                }
            }
        }

        return PrepareOutcome::Ready(PreparedRequest { env, context });
    }
}

#[allow(clippy::too_many_arguments)]
async fn pre_rewrite_response(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    depth: usize,
    env: &mut RequestEnv,
    context: &Arc<EffectiveConfig>,
    location: Option<&LocationConfig>,
    variables: &mut Variables,
    connection_guard: &mut Option<ConnectionLimitGuard>,
) -> Option<Processed> {
    if let Err(error) = validate_declared_request_body_size(request, context.client_max_body_size) {
        return Some(Processed {
            response: request_body_error_response(&error),
            context: context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    if context.client_max_body_size != 0
        && request.buffered_body_len() > context.client_max_body_size
    {
        return Some(Processed {
            response: error_response(413),
            context: context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    if location.is_some_and(|location| location.context.internal) && !env.internal {
        return Some(Processed {
            response: error_response(404),
            context: context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    if let Some(response) = basic_access_response(context, request, variables).await {
        return Some(Processed {
            response: apply_auth_delay(context, response).await,
            context: context.clone(),
            tunnel: false,
            upgrade: None,
        });
    }
    if !env.limits_checked {
        env.limits_checked = true;
        if let Some(response) = apply_limit_req(context, variables).await {
            let outcome =
                configured_error_page(http, server, request, response, context, variables, depth)
                    .await;
            return Some(Processed {
                response: outcome.response,
                context: outcome.context,
                tunnel: false,
                upgrade: None,
            });
        }
        match acquire_connection_limits(context, variables) {
            Ok(guard) => *connection_guard = Some(guard),
            Err(status) => {
                let outcome = configured_error_page(
                    http,
                    server,
                    request,
                    error_response(status),
                    context,
                    variables,
                    depth,
                )
                .await;
                return Some(Processed {
                    response: outcome.response,
                    context: outcome.context,
                    tunnel: false,
                    upgrade: None,
                });
            }
        }
    }
    if !env.auth_request_checked
        && let Some(response) =
            auth_request_response(http, server, request, context, variables, depth).await
    {
        let response = apply_auth_delay(context, response).await;
        let outcome =
            configured_error_page(http, server, request, response, context, variables, depth).await;
        return Some(Processed {
            response: outcome.response,
            context: outcome.context,
            tunnel: false,
            upgrade: None,
        });
    }
    None
}

fn run_script(
    context: &mut Arc<EffectiveConfig>,
    env: &mut RequestEnv,
    variables: &mut Variables,
    request: &Request,
    location: Option<&LocationConfig>,
    logger: Option<Arc<Logger>>,
) -> RewriteOutcome {
    let context = Arc::make_mut(context);
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

fn response_outcome(context: Arc<EffectiveConfig>, response: Response) -> PrepareOutcome {
    PrepareOutcome::Response(Processed {
        response,
        context,
        tunnel: false,
        upgrade: None,
    })
}

fn location_auto_redirect(
    locations: &[LocationConfig],
    uri: &str,
    args: Option<&str>,
) -> Option<(Arc<EffectiveConfig>, Response)> {
    for location in locations {
        if let LocationMatcher::Prefix(prefix) | LocationMatcher::PrefixNoRegex(prefix) =
            &location.matcher
            && prefix.strip_suffix('/') == Some(uri)
            && location_context_redirects_to_slash(&location.context)
        {
            let mut response = Response::empty(301);
            response.set_header(
                "Location",
                append_args(
                    format!("{}/", escape_nginx_uri(uri)),
                    args.map(str::to_string),
                ),
            );
            return Some((location.context.clone(), response));
        }
        if let Some(redirect) = location_auto_redirect(&location.nested, uri, args) {
            return Some(redirect);
        }
    }
    None
}

fn location_context_redirects_to_slash(context: &EffectiveConfig) -> bool {
    context.proxy_pass.is_some()
        || context.fastcgi_pass.is_some()
        || context.uwsgi_pass.is_some()
        || context.scgi_pass.is_some()
        || context.grpc.pass.is_some()
        || context.memcached_pass.is_some()
}

async fn rewrite_response_outcome(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    response: Response,
    context: Arc<EffectiveConfig>,
    variables: &mut Variables,
    depth: usize,
) -> PrepareOutcome {
    let outcome =
        configured_error_page(http, server, request, response, &context, variables, depth).await;
    PrepareOutcome::Response(Processed {
        response: outcome.response,
        context: outcome.context,
        tunnel: false,
        upgrade: None,
    })
}

fn error_outcome(context: Arc<EffectiveConfig>) -> PrepareOutcome {
    response_outcome(context, error_response(500))
}
