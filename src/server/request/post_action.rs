use super::*;

const MAX_POST_ACTIONS: usize = 10;

pub(in crate::server) fn start_post_action(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    env: &RequestEnv,
    variables: &Variables,
    context: &EffectiveConfig,
    depth: usize,
) {
    let Some(target) = context.post_action.clone() else {
        return;
    };
    let http = http.clone();
    let server = server.clone();
    let mut request = request.clone();
    if request.body_stream.take().is_some() {
        request.headers.retain(|header| {
            !header.name.eq_ignore_ascii_case("content-length")
                && !header.name.eq_ignore_ascii_case("transfer-encoding")
                && !header.name.eq_ignore_ascii_case("expect")
        });
    }
    let env = env.clone();
    let mut variables = variables.clone();
    tokio::spawn(async move {
        let _ = execute_post_actions(
            &http,
            &server,
            &request,
            env,
            &mut variables,
            target,
            depth + 1,
        )
        .await;
    });
}

pub(in crate::server) async fn execute_post_actions(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    env: RequestEnv,
    variables: &mut Variables,
    mut target: String,
    depth: usize,
) -> Option<Processed> {
    let mut env = env;
    let mut request = request.clone();
    let mut last = None;
    let limit = MAX_POST_ACTIONS.saturating_sub(depth.saturating_sub(1));

    for step in 0..limit {
        if target.starts_with('/') {
            let (path, args) = rewrite_destination(&target, &None);
            let uri = normalize_uri(&path)?;
            let unparsed_uri = append_args(uri.clone(), args.clone());
            env = RequestEnv {
                uri,
                args,
                unparsed_uri: Some(unparsed_uri.clone()),
                internal: true,
                bypass_try_files: false,
                limits_checked: false,
                auth_request_checked: false,
                mirror_checked: false,
            };
            request.target = unparsed_uri;
        } else {
            env.internal = true;
        }

        let processed = if target.starts_with('/') {
            Box::pin(process_request(
                http,
                server,
                &mut request,
                env.clone(),
                variables,
                depth + step,
            ))
            .await
        } else {
            process_named_location(
                http,
                server,
                &mut request,
                &target,
                env.clone(),
                variables,
                depth,
                MAX_URI_CHANGES + 1,
            )
            .await?
        };

        let Some(next) = processed.context.post_action.clone() else {
            return Some(processed);
        };
        last = Some(processed);
        target = next;
    }

    last
}
