use super::*;

pub(in crate::server) fn start_mirror_requests(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    env: &mut RequestEnv,
    context: &EffectiveConfig,
    variables: &Variables,
    depth: usize,
) {
    if env.mirror_checked || context.mirrors.is_empty() {
        return;
    }
    env.mirror_checked = true;

    for target in &context.mirrors {
        let target = eval_variables(target, variables);
        let (path, args) = rewrite_destination(&target, &env.args);
        let Some(uri) = normalize_uri(&path) else {
            continue;
        };
        let unparsed_uri = Some(append_args(uri.clone(), args.clone()));
        let mirror_env = RequestEnv {
            uri,
            args,
            unparsed_uri,
            internal: true,
            bypass_try_files: false,
            limits_checked: true,
            auth_request_checked: true,
            mirror_checked: true,
        };
        let mut mirror_request = request.clone();
        if !context.mirror_request_body {
            mirror_request.body.clear();
            mirror_request.body_stream = None;
            mirror_request.headers.retain(|header| {
                !header.name.eq_ignore_ascii_case("content-length")
                    && !header.name.eq_ignore_ascii_case("transfer-encoding")
                    && !header.name.eq_ignore_ascii_case("expect")
            });
        }
        let http = http.clone();
        let server = server.clone();
        let mut variables = variables.clone();
        if !context.mirror_request_body {
            clear_request_body_variables(&mut variables);
        }
        let depth = depth + 1;
        tokio::spawn(async move {
            let mut mirror_request = mirror_request;
            let _ = process_request(
                &http,
                &server,
                &mut mirror_request,
                mirror_env,
                &mut variables,
                depth,
            )
            .await;
        });
    }
}
