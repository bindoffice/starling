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
    let _connections = if !env.limits_checked {
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
            Ok(guard) => Some(guard),
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
    } else {
        None
    };
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
            return Some(Processed {
                response: *response,
                context: *context,
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
                ))
                .await,
            );
        }
        RewriteOutcome::Break | RewriteOutcome::None => {}
    }
    refresh_variables(&env, Some(location), variables, &context);
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
            },
        )
        .await;
        response.inherit_upstream_control_to(&mut processed.response);
        sync_upstream_response_variables(variables, &processed.response);
        return Some(processed);
    }
    if let Some(ranged) = apply_upstream_ranges(
        &mut response,
        request,
        content.force_ranges(&context),
        content.upstream(),
        context.max_ranges,
    ) {
        response = ranged;
    }
    sync_upstream_response_variables(variables, &response);
    Some(Processed {
        response,
        context: *context,
        tunnel: false,
    })
}
