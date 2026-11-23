use super::*;

mod content;
mod index;
mod named;
mod prepare;

use index::{IndexDecision, resolve_index};
pub(in crate::server) use named::process_named_location;
use prepare::{PrepareOutcome, prepare_request};

pub(in crate::server) const MAX_URI_CHANGES: usize = 10;

pub(in crate::server) async fn process_request(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &mut Request,
    env: RequestEnv,
    variables: &mut Variables,
    depth: usize,
) -> Processed {
    Box::pin(process_request_inner(
        http,
        server,
        request,
        env,
        variables,
        depth,
        true,
        MAX_URI_CHANGES + 1,
    ))
    .await
}

#[allow(clippy::too_many_arguments)]
async fn process_request_inner(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &mut Request,
    env: RequestEnv,
    variables: &mut Variables,
    depth: usize,
    run_server_rewrite: bool,
    mut uri_changes: usize,
) -> Processed {
    if depth > MAX_URI_CHANGES + 1 {
        return Processed {
            response: error_response(500),
            context: server.context.clone(),
            tunnel: false,
        };
    }
    if request.method == "TRACE" {
        return response_with_context(server.context.clone(), error_response(405));
    }
    if request.method == "CONNECT" {
        if request.version != "HTTP/1.1" || !server_allows_connect(server) {
            return response_with_context(server.context.clone(), error_response(405));
        }
        if request.has_nonzero_request_body() {
            return response_with_context(server.context.clone(), error_response(400));
        }
    }

    let prepared = match prepare_request(
        http,
        server,
        request,
        env,
        variables,
        depth,
        run_server_rewrite,
        uri_changes,
    )
    .await
    {
        PrepareOutcome::Ready(prepared) => prepared,
        PrepareOutcome::Response(processed) => return processed,
    };
    let mut env = prepared.env;
    let mut context = Box::new(prepared.context);
    let _connections = prepared.connection_guard;
    let location = resolve_location(&server.locations, &env.uri);

    if let Some((response, tunnel)) = tunnel_request_outcome(request, &context) {
        return Processed {
            response,
            context: *context,
            tunnel,
        };
    }

    if let Err(error) = prepare_request_body(request, &context, variables).await {
        request.keep_alive = false;
        return Processed {
            response: request_body_error_response(&error),
            context: *context,
            tunnel: false,
        };
    }

    start_mirror_requests(http, server, request, &mut env, &context, variables, depth);

    match resolve_index(&env, &context, location, variables).await {
        IndexDecision::None => {}
        IndexDecision::Response(response) => {
            return Processed {
                response,
                context: *context,
                tunnel: false,
            };
        }
        IndexDecision::Redirect(uri) => {
            if !consume_uri_change(&mut uri_changes) {
                return Processed {
                    response: error_response(500),
                    context: *context,
                    tunnel: false,
                };
            }
            env.uri = uri;
            env.unparsed_uri = None;
            env.valid_location = true;
            env.internal = true;
            env.bypass_try_files = true;
            variables.uri = env.uri.clone();
            return Box::pin(process_request_inner(
                http,
                server,
                request,
                env,
                variables,
                depth + 1,
                false,
                uri_changes,
            ))
            .await;
        }
    }

    if context.stub_status {
        return Processed {
            response: stub_status_response(request, variables),
            context: *context,
            tunnel: false,
        };
    }

    let (mut response, content) =
        content::serve_content(http, request, &env, location, &context, variables).await;
    sync_upstream_response_variables(variables, &response);

    // Nginx intercepts upstream statuses starting at 300 before X-Accel-Redirect.
    if intercepts_upstream_status(
        response.status,
        content.intercept_errors(&context),
        &context.error_pages,
    ) {
        let outcome =
            configured_error_page(http, server, request, response, &context, variables, depth)
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
                depth,
            },
        )
        .await;
        processed.response.inherit_early_hints_from(&response);
        response.inherit_upstream_control_to(&mut processed.response);
        sync_upstream_response_variables(variables, &processed.response);
        return processed;
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
    Processed {
        response,
        context: *context,
        tunnel: false,
    }
}

fn server_allows_connect(server: &ServerConfig) -> bool {
    config_allows_connect(&server.context) || server.locations.iter().any(location_allows_connect)
}

fn location_allows_connect(location: &LocationConfig) -> bool {
    config_allows_connect(&location.context) || location.nested.iter().any(location_allows_connect)
}

fn config_allows_connect(context: &EffectiveConfig) -> bool {
    context.tunnel.pass.is_some()
        || context.rewrite_actions.iter().any(|action| {
            matches!(
                action,
                RewriteAction::If(conditional) if conditional.tunnel_pass.is_some()
            )
        })
}

fn response_with_context(context: EffectiveConfig, response: Response) -> Processed {
    Processed {
        response,
        context,
        tunnel: false,
    }
}

pub(super) fn tunnel_request_outcome(
    request: &Request,
    context: &EffectiveConfig,
) -> Option<(Response, bool)> {
    if request.method == "CONNECT" {
        if request.version != "HTTP/1.1" || context.tunnel.pass.is_none() {
            return Some((error_response(405), false));
        }
        if request.has_request_body() {
            return Some((error_response(400), false));
        }
        return Some((Response::new(200), true));
    }
    context
        .tunnel
        .pass
        .is_some()
        .then(|| (error_response(405), false))
}

fn sync_upstream_response_variables(variables: &mut Variables, response: &Response) {
    variables
        .upstream_headers
        .clone_from(&response.upstream_headers().to_vec());
    variables
        .upstream_trailers
        .clone_from(&response.upstream_trailers().to_vec());
}

fn consume_uri_change(uri_changes: &mut usize) -> bool {
    *uri_changes = uri_changes.saturating_sub(1);
    *uri_changes > 0
}
