use super::*;

pub(in crate::server) async fn basic_access_response(
    context: &EffectiveConfig,
    request: &Request,
    variables: &mut Variables,
) -> Option<Response> {
    let addresses_allowed = if let Some(limit_except) = &context.limit_except
        && limit_except_applies(limit_except, &request.method)
    {
        access_allowed(&limit_except.access_rules, &variables.remote_addr)
    } else {
        access_allowed(&context.access_rules, &variables.remote_addr)
    };
    let auth_configured = context.auth_basic.is_some() && context.auth_basic_user_file.is_some();
    let auth_response = basic_auth_response(context, request, variables).await;
    if !context.satisfy_any {
        if !addresses_allowed {
            return Some(error_response(403));
        }
        return auth_response;
    }
    if addresses_allowed {
        return None;
    }
    if auth_configured {
        return auth_response;
    }
    Some(error_response(403))
}

pub(in crate::server) async fn basic_auth_response(
    context: &EffectiveConfig,
    request: &Request,
    variables: &mut Variables,
) -> Option<Response> {
    let realm = context
        .auth_basic
        .as_ref()
        .map(|realm| eval_variables(realm, variables))?;
    if realm == "off" {
        return None;
    }
    // Nginx declines authentication when only one of the two directives is set.
    let user_file = context.auth_basic_user_file.as_ref()?;
    let Some(credentials) = request
        .header("authorization")
        .and_then(parse_authorization_header)
    else {
        return Some(basic_auth_challenge(&realm));
    };
    variables.remote_user = credentials.username.clone();
    let user_file = eval_variables(user_file, variables);
    let user_file = match tokio::fs::read_to_string(user_file).await {
        Ok(user_file) => user_file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Some(error_response(403)),
        Err(_) => return Some(error_response(500)),
    };
    let Some(stored_password) = find_user_password(&user_file, &credentials.username) else {
        variables.remote_user.clear();
        return Some(basic_auth_challenge(&realm));
    };
    if !verify_password(&credentials.password, stored_password) {
        variables.remote_user.clear();
        return Some(basic_auth_challenge(&realm));
    }
    None
}

pub(in crate::server) fn basic_auth_challenge(realm: &str) -> Response {
    let mut response = error_response(401);
    response.set_header("WWW-Authenticate", format!("Basic realm=\"{realm}\""));
    response
}
