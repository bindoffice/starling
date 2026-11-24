use super::*;

#[allow(clippy::too_many_arguments)]
pub(in crate::server) async fn process_named_location(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &mut Request,
    name: &str,
    mut env: RequestEnv,
    variables: &mut Variables,
    depth: usize,
    mut uri_changes: usize,
    connection_guard: &mut Option<ConnectionLimitGuard>,
) -> Option<Processed> {
    if !consume_uri_change(&mut uri_changes) {
        return Some(Processed {
            response: error_response(500),
            context: server.context.clone(),
            tunnel: false,
        });
    }
    let location = find_named_location(&server.locations, name)?;
    let mut context = Box::new(location.context.clone());
    let context_logger = variables.error_logger_for(&context);
    variables.set_variable_warning_context(&context, context_logger.clone());
    prepare_userid_variables(&context, variables);
    refresh_variables(&env, Some(location), variables, &context);

    if let Some(response) = basic_access_response(&context, request, variables).await {
        return Some(Processed {
            response,
            context: *context,
            tunnel: false,
        });
    }
    if !env.limits_checked {
        env.limits_checked = true;
        if let Some(response) = apply_limit_req(&context, variables).await {
            let outcome =
                configured_error_page(http, server, request, response, &context, variables, depth)
                    .await;
            return Some(Processed {
                response: outcome.response,
                context: outcome.context,
                tunnel: false,
            });
        }
        match acquire_connection_limits(&context, variables) {
            Ok(guard) => *connection_guard = Some(guard),
            Err(status) => {
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
                return Some(Processed {
                    response: outcome.response,
                    context: outcome.context,
                    tunnel: false,
                });
            }
        }
    }
    if !env.auth_request_checked
        && let Some(response) =
            auth_request_response(http, server, request, &context, variables, depth).await
    {
        let outcome =
            configured_error_page(http, server, request, response, &context, variables, depth)
                .await;
        return Some(Processed {
            response: outcome.response,
            context: outcome.context,
            tunnel: false,
        });
    }
    let actions = std::mem::take(&mut context.rewrite_actions);
    let outcome = apply_rewrite_script(
        &actions,
        &mut env,
        &mut context,
        variables,
        request,
        Some(location),
        context_logger,
    );
    context.rewrite_actions = actions;
    match outcome {
        RewriteOutcome::Response(response) => {
            let outcome =
                configured_error_page(http, server, request, *response, &context, variables, depth)
                    .await;
            return Some(Processed {
                response: outcome.response,
                context: outcome.context,
                tunnel: false,
            });
        }
        RewriteOutcome::Last => {
            variables.uri = env.uri.clone();
            return Some(
                Box::pin(process_request_inner(
                    http,
                    server,
                    request,
                    env,
                    variables,
                    depth + 1,
                    false,
                    uri_changes,
                    connection_guard,
                ))
                .await,
            );
        }
        RewriteOutcome::Break | RewriteOutcome::None => {}
    }
    refresh_variables(&env, Some(location), variables, &context);
    if !env.bypass_try_files
        && let Some(try_files) = &context.try_files
    {
        match resolve_try_files(try_files, &context, Some(location), &env, variables).await {
            TryFilesResult::Found { uri, is_dir } => {
                env.uri = uri;
                env.unparsed_uri = None;
                env.bypass_try_files = true;
                env.add_uri_to_alias =
                    !is_dir && matches!(context.alias_uri_prefix.as_deref(), Some(""));
                refresh_variables(&env, Some(location), variables, &context);
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
                return Some(Processed {
                    response: outcome.response,
                    context: outcome.context,
                    tunnel: false,
                });
            }
            TryFilesResult::FallbackUri(uri) => {
                env.uri = normalize_uri(&uri).unwrap_or_else(|| uri.clone());
                env.unparsed_uri = None;
                env.internal = true;
                env.bypass_try_files = false;
                if !consume_uri_change(&mut uri_changes) {
                    return Some(Processed {
                        response: error_response(500),
                        context: *context,
                        tunnel: false,
                    });
                }
                variables.uri = env.uri.clone();
                return Some(
                    Box::pin(process_request_inner(
                        http,
                        server,
                        request,
                        env,
                        variables,
                        depth + 1,
                        false,
                        uri_changes,
                        connection_guard,
                    ))
                    .await,
                );
            }
            TryFilesResult::Named(name) => {
                if let Some(processed) = Box::pin(process_named_location(
                    http,
                    server,
                    request,
                    &name,
                    env,
                    variables,
                    depth,
                    uri_changes,
                    connection_guard,
                ))
                .await
                {
                    return Some(processed);
                }
                return Some(Processed {
                    response: error_response(500),
                    context: *context,
                    tunnel: false,
                });
            }
        }
    }
    if let Some((response, tunnel)) = tunnel_request_outcome(request, &context) {
        return Some(Processed {
            response,
            context: *context,
            tunnel,
        });
    }
    if let Err(error) = prepare_request_body(request, &context, variables).await {
        request.keep_alive = false;
        return Some(Processed {
            response: request_body_error_response(&error),
            context: *context,
            tunnel: false,
        });
    }
    start_mirror_requests(http, server, request, &mut env, &context, variables, depth);
    match resolve_index(&env, &context, Some(location), variables).await {
        IndexDecision::None => {}
        IndexDecision::Response(response) => {
            return Some(Processed {
                response,
                context: *context,
                tunnel: false,
            });
        }
        IndexDecision::Redirect(uri) => {
            if !consume_uri_change(&mut uri_changes) {
                return Some(Processed {
                    response: error_response(500),
                    context: *context,
                    tunnel: false,
                });
            }
            env.uri = uri;
            env.unparsed_uri = None;
            env.valid_location = true;
            env.internal = true;
            env.bypass_try_files = true;
            variables.uri = env.uri.clone();
            return Some(
                Box::pin(process_request_inner(
                    http,
                    server,
                    request,
                    env,
                    variables,
                    depth + 1,
                    false,
                    uri_changes,
                    connection_guard,
                ))
                .await,
            );
        }
    }
    if context.stub_status {
        return Some(Processed {
            response: stub_status_response(request, variables),
            context: *context,
            tunnel: false,
        });
    }
    let (mut response, content) =
        content::serve_content(http, request, &env, Some(location), &context, variables).await;
    sync_upstream_response_variables(variables, &response);
    if intercepts_upstream_status(
        response.status,
        content.intercept_errors(&context),
        &context.error_pages,
    ) {
        let outcome = configured_error_page(
            http,
            server,
            request,
            response,
            &context,
            variables,
            depth + 1,
        )
        .await;
        response = outcome.response;
        *context = outcome.context;
    }
    if let Some(redirect) = response.take_upstream_redirect() {
        let mut processed = follow_upstream_redirect(
            &redirect,
            variables,
            UpstreamRedirectContext {
                http,
                server,
                request,
                env: &env,
                current: &context,
                depth: depth + 1,
                connection_guard,
            },
        )
        .await;
        response.inherit_upstream_control_to(&mut processed.response);
        sync_upstream_response_variables(variables, &processed.response);
        return Some(processed);
    }
    enable_upstream_ranges(
        &mut response,
        content.force_ranges(&context),
        content.upstream(),
        cache_status_is_hit(&variables.upstream_cache_status),
    );
    sync_upstream_response_variables(variables, &response);
    Some(Processed {
        response,
        context: *context,
        tunnel: false,
    })
}
