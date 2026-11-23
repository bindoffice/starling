use super::*;

pub(in crate::server) struct UpstreamRedirectContext<'a> {
    pub http: &'a HttpConfig,
    pub server: &'a ServerConfig,
    pub request: &'a mut Request,
    pub env: &'a RequestEnv,
    pub current: &'a EffectiveConfig,
    pub depth: usize,
}

pub(in crate::server) async fn follow_upstream_redirect(
    redirect: &str,
    variables: &mut Variables,
    context: UpstreamRedirectContext<'_>,
) -> Processed {
    if redirect.starts_with('@') {
        let mut env = context.env.clone();
        env.internal = true;
        if let Some(processed) = Box::pin(process_named_location(
            context.http,
            context.server,
            context.request,
            redirect,
            env,
            variables,
            context.depth + 1,
            MAX_URI_CHANGES + 1,
        ))
        .await
        {
            return processed;
        }
        return redirect_failure(
            context.http,
            context.server,
            context.request,
            500,
            context.current,
            variables,
            context.depth,
        )
        .await;
    }

    let mut internal_request = internal_redirect_request(context.request);
    if !internal_request.method.eq_ignore_ascii_case("HEAD") {
        variables.method = "GET".into();
    }
    let (path, args) = match redirect.split_once('?') {
        Some((path, args)) => (path, Some(args.to_string())),
        None => (redirect, None),
    };
    if !path.starts_with('/') {
        return redirect_failure(
            context.http,
            context.server,
            context.request,
            404,
            context.current,
            variables,
            context.depth,
        )
        .await;
    }
    let Some(uri) = normalize_uri(path) else {
        return redirect_failure(
            context.http,
            context.server,
            context.request,
            404,
            context.current,
            variables,
            context.depth,
        )
        .await;
    };
    let env = RequestEnv {
        uri,
        args,
        unparsed_uri: None,
        valid_location: true,
        internal: true,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: false,
        auth_request_checked: false,
        mirror_checked: false,
    };
    Box::pin(process_request(
        context.http,
        context.server,
        &mut internal_request,
        env,
        variables,
        context.depth + 1,
    ))
    .await
}

async fn redirect_failure(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    status: u16,
    current_context: &EffectiveConfig,
    variables: &mut Variables,
    depth: usize,
) -> Processed {
    let outcome = configured_error_page(
        http,
        server,
        request,
        error_response(status),
        current_context,
        variables,
        depth,
    )
    .await;
    Processed {
        response: outcome.response,
        context: outcome.context,
        tunnel: false,
    }
}
