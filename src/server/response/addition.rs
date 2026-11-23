use super::*;

pub(in crate::server) async fn apply_addition(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &mut Variables,
) {
    if request.method.eq_ignore_ascii_case("HEAD")
        || response.status != 200
        || response.header("Content-Encoding").is_some()
        || context.addition_before.is_none() && context.addition_after.is_none()
    {
        return;
    }
    let content_type = response
        .header("Content-Type")
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if !context
        .addition_types
        .iter()
        .any(|allowed| allowed == "*" || allowed.eq_ignore_ascii_case(content_type))
    {
        return;
    }

    let Some(before) = addition_fragment(
        http,
        server,
        request,
        context.addition_before.as_deref(),
        variables,
    )
    .await
    else {
        return;
    };
    let Some(after) = addition_fragment(
        http,
        server,
        request,
        context.addition_after.as_deref(),
        variables,
    )
    .await
    else {
        return;
    };
    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let main = match body.into_memory(MAX_BODY_BYTES as u64).await {
        Ok(body) => body,
        Err(body) => {
            response.body = *body;
            return;
        }
    };

    let mut combined = Vec::with_capacity(before.len() + main.len() + after.len());
    combined.extend_from_slice(&before);
    combined.extend_from_slice(&main);
    combined.extend_from_slice(&after);
    response.body = ResponseBody::Bytes(combined);
    response.remove_header("Accept-Ranges");
    response.set_content_length();
    weaken_etag(response);
}

async fn addition_fragment(
    http: &HttpConfig,
    server: &ServerConfig,
    request: &Request,
    target: Option<&str>,
    variables: &Variables,
) -> Option<Vec<u8>> {
    let Some(target) = target else {
        return Some(Vec::new());
    };
    let target = eval_variables(target, variables);
    if target.is_empty() {
        return Some(Vec::new());
    }
    let (path, args) = rewrite_destination(&target, &None);
    let uri = normalize_uri(&path)?;
    let env = RequestEnv {
        uri: uri.clone(),
        args: args.clone(),
        unparsed_uri: Some(append_args(uri, args)),
        valid_location: true,
        internal: true,
        bypass_try_files: false,
        add_uri_to_alias: false,
        limits_checked: true,
        auth_request_checked: true,
        mirror_checked: true,
    };
    let mut subrequest = request.clone();
    subrequest.method = "GET".into();
    subrequest.body.clear();
    subrequest.body_stream = None;
    subrequest.headers.retain(|header| {
        !header.name.eq_ignore_ascii_case("content-length")
            && !header.name.eq_ignore_ascii_case("transfer-encoding")
            && !header.name.eq_ignore_ascii_case("expect")
    });
    let mut variables = variables.clone();
    variables.method = "GET".into();
    clear_request_body_variables(&mut variables);
    let processed = process_request(http, server, &mut subrequest, env, &mut variables, 1).await;
    processed
        .response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .ok()
}
