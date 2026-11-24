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

    let mut jobs = Vec::with_capacity(context.mirrors.len());
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
            valid_location: true,
            internal: true,
            bypass_try_files: false,
            add_uri_to_alias: false,
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
        let mut variables = variables.clone();
        if !context.mirror_request_body {
            clear_request_body_variables(&mut variables);
        }
        jobs.push((mirror_request, mirror_env, variables));
    }
    if jobs.is_empty() {
        return;
    }

    let http = http.clone();
    let server = server.clone();
    let depth = depth + 1;
    tokio::spawn(async move {
        // Nginx keeps mirror subrequests in declaration order for logging.
        for (mut mirror_request, mirror_env, mut variables) in jobs {
            let processed = process_request(
                &http,
                &server,
                &mut mirror_request,
                mirror_env,
                &mut variables,
                depth,
            )
            .await;
            variables.status = processed.response.status;
            variables.bytes_sent = processed.response.body_len();
            variables.body_bytes_sent = variables.bytes_sent;
            if processed.context.log_subrequest
                && (processed.context.log_not_found || processed.response.status != 404)
            {
                log_access(&http, &processed.context, &variables);
            }
        }
    });
}
