use super::*;

pub(in crate::server) async fn auth_request_response(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    context: &EffectiveConfig,
    variables: &mut Variables,
    depth: usize,
) -> Option<Response> {
    let auth_uri = eval_variables(context.auth_request.as_ref()?, variables);
    let (path, args) = match auth_uri.split_once('?') {
        Some((path, args)) => (path, Some(args.to_string())),
        None => (auth_uri.as_str(), None),
    };
    if !path.starts_with('/') {
        return Some(error_response(500));
    }
    let uri = normalize_uri(path)?;
    let mut headers = request.headers.clone();
    headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
    });
    let mut auth_request = Request {
        method: "GET".into(),
        target: auth_uri,
        version: request.version.clone(),
        headers,
        body_stream: None,
        body: Vec::new(),
        remote_addr: request.remote_addr.clone(),
        local_addr: request.local_addr.clone(),
        keep_alive: false,
    };
    let mut auth_variables = variables.clone();
    auth_variables.method = "GET".into();
    auth_variables.request_target = auth_request.target.clone();
    auth_variables.uri = uri.clone();
    auth_variables.fastcgi_script_name = uri.clone();
    auth_variables.fastcgi_path_info.clear();
    auth_variables.uwsgi_script_name = uri.clone();
    auth_variables.scgi_script_name = uri.clone();
    auth_variables.args = args.clone().unwrap_or_default();
    let env = RequestEnv {
        uri,
        args,
        unparsed_uri: Some(auth_request.target.clone()),
        valid_location: true,
        internal: true,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: true,
        auth_request_checked: true,
        mirror_checked: true,
    };
    let processed = Box::pin(process_request(
        http,
        server,
        &mut auth_request,
        env,
        &mut auth_variables,
        depth + 1,
    ))
    .await;
    auth_variables.status = processed.response.status;
    for set in &context.auth_request_sets {
        let value = eval_variables(&set.value, &auth_variables);
        variables.custom.insert(set.name.clone(), value);
    }
    if (200..300).contains(&processed.response.status) {
        return None;
    }
    if processed.response.status == 401 || processed.response.status == 403 {
        let mut response = error_response(processed.response.status);
        if let Some(challenge) = processed.response.header("WWW-Authenticate") {
            response.set_header("WWW-Authenticate", challenge);
        }
        return Some(response);
    }
    Some(error_response(500))
}
