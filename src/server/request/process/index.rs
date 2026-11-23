use super::*;

pub(super) enum IndexDecision {
    None,
    Response(Response),
    Redirect(String),
}

pub(super) async fn resolve_index(
    env: &RequestEnv,
    context: &EffectiveConfig,
    location: Option<&LocationConfig>,
    variables: &Variables,
) -> IndexDecision {
    if !is_static_request(context) || !env.uri.ends_with('/') {
        return IndexDecision::None;
    }
    for index in &context.index {
        let index = eval_variables(index, variables);
        if index.is_empty() {
            continue;
        }
        if index.starts_with('/') {
            return IndexDecision::Redirect(normalize_uri(&index).unwrap_or(index));
        }
        let uri = format!("{}{}", env.uri, index);
        let uri = normalize_uri(&uri).unwrap_or(uri);
        let path = map_request_path(context, location, &uri, env.add_uri_to_alias, variables);
        if let Err(status) = check_path_access(&path, context, variables).await {
            return IndexDecision::Response(error_response(status));
        }
        match tokio::fs::metadata(&path).await {
            Ok(_) => return IndexDecision::Redirect(uri),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                log_not_found(&path, context, variables);
            }
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
                return IndexDecision::Response(error_response(403));
            }
            Err(_) => return IndexDecision::Response(error_response(500)),
        }
    }
    IndexDecision::None
}

fn is_static_request(context: &EffectiveConfig) -> bool {
    context.fastcgi_pass.is_none()
        && context.uwsgi_pass.is_none()
        && context.scgi_pass.is_none()
        && context.grpc.pass.is_none()
        && context.memcached_pass.is_none()
        && context.proxy_pass.is_none()
}

fn log_not_found(path: &Path, context: &EffectiveConfig, variables: &Variables) {
    if !context.log_not_found {
        return;
    }
    let Some(logger) = variables.error_logger_for(context) else {
        return;
    };
    logger.log(
        LogLevel::Error,
        format!(
            "open() \"{}\" failed (2: No such file or directory)",
            path.display()
        ),
    );
}
