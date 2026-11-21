use super::*;

pub(super) fn apply_rule(
    rule: &crate::config::RewriteRule,
    env: &mut RequestEnv,
    variables: &mut Variables,
    uri_changed: &mut bool,
) -> Option<RewriteOutcome> {
    let captures = rule.regex.captures(&env.uri);
    if variables.rewrite_log_enabled()
        && let Some(logger) = &variables.rewrite_logger
    {
        let result = if captures.is_some() {
            "matches"
        } else {
            "does not match"
        };
        logger.log(
            LogLevel::Notice,
            format!("\"{}\" {result} \"{}\"", rule.regex.as_str(), env.uri),
        );
    }
    let captures = captures?;
    set_regex_captures(variables, &rule.regex, &captures);
    let replacement = expand_captures(&rule.replacement, &captures, variables);
    let redirect_status = match rule.flag {
        RewriteFlag::Permanent => Some(301),
        RewriteFlag::Redirect => Some(302),
        _ if is_external_redirect(&replacement) => Some(302),
        _ => None,
    };
    if let Some(status) = redirect_status {
        log_rewritten_redirect(&replacement, &env.args, variables);
        return Some(RewriteOutcome::Response(Box::new(rewrite_redirect(
            status,
            &replacement,
            &env.args,
        ))));
    }
    match rule.flag {
        RewriteFlag::Last => {
            update_uri(env, &replacement);
            log_rewritten_data(env, variables);
            Some(RewriteOutcome::Last)
        }
        RewriteFlag::Break => {
            update_uri(env, &replacement);
            log_rewritten_data(env, variables);
            *uri_changed = false;
            Some(RewriteOutcome::Break)
        }
        RewriteFlag::None => {
            update_uri(env, &replacement);
            log_rewritten_data(env, variables);
            *uri_changed = true;
            None
        }
        RewriteFlag::Redirect | RewriteFlag::Permanent => unreachable!(),
    }
}

fn log_rewritten_data(env: &RequestEnv, variables: &Variables) {
    if !variables.rewrite_log_enabled() {
        return;
    }
    if let Some(logger) = &variables.rewrite_logger {
        logger.log(
            LogLevel::Notice,
            format!(
                "rewritten data: \"{}\", args: \"{}\"",
                env.uri,
                env.args.as_deref().unwrap_or_default()
            ),
        );
    }
}

fn log_rewritten_redirect(
    replacement: &str,
    original_args: &Option<String>,
    variables: &Variables,
) {
    if !variables.rewrite_log_enabled() {
        return;
    }
    if let Some(logger) = &variables.rewrite_logger {
        let (location, args) = rewrite_destination(replacement, original_args);
        logger.log(
            LogLevel::Notice,
            format!("rewritten redirect: \"{}\"", append_args(location, args)),
        );
    }
}

fn is_external_redirect(replacement: &str) -> bool {
    replacement.starts_with("http://")
        || replacement.starts_with("https://")
        || replacement.starts_with("$scheme")
}

fn update_uri(env: &mut RequestEnv, replacement: &str) {
    let (uri, args) = rewrite_destination(replacement, &env.args);
    env.uri = normalize_uri(&uri).unwrap_or(uri);
    env.args = args;
    env.unparsed_uri = None;
    env.internal = true;
}

fn rewrite_redirect(status: u16, replacement: &str, original_args: &Option<String>) -> Response {
    let (location, args) = rewrite_destination(replacement, original_args);
    let location = append_args(location, args);
    let body = format!(
        "<html><body>Redirecting to <a href=\"{replacement}\">{replacement}</a></body></html>"
    );
    let mut response = Response::html(status, body);
    response.set_header("Location", location);
    response
}

pub(in crate::server) fn rewrite_destination(
    replacement: &str,
    original_args: &Option<String>,
) -> (String, Option<String>) {
    if let Some(replacement) = replacement.strip_suffix('?') {
        return match replacement.split_once('?') {
            Some((path, args)) => (
                path.to_string(),
                (!args.is_empty()).then(|| args.to_string()),
            ),
            None => (replacement.to_string(), None),
        };
    }
    match replacement.split_once('?') {
        Some((path, args)) => (
            path.to_string(),
            (!args.is_empty()).then(|| args.to_string()),
        ),
        None => (replacement.to_string(), original_args.clone()),
    }
}

pub(in crate::server) fn append_args(mut target: String, args: Option<String>) -> String {
    if let Some(args) = args
        && !args.is_empty()
    {
        target.push('?');
        target.push_str(&args);
    }
    target
}

pub(in crate::server) fn apply_return(
    status: u16,
    value: Option<&str>,
    variables: &Variables,
    request: &Request,
) -> Response {
    let value = value.map(|value| eval_variables(value, variables));
    if matches!(status, 301 | 302 | 303 | 307 | 308) {
        let location = value.unwrap_or_default();
        let mut response = Response::html(
            status,
            format!(
                "<html>\r\n<head><title>{status} {}</title></head>\r\n<body>\r\n<center><h1>{status} {}</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n",
                reason_phrase(status),
                reason_phrase(status)
            ),
        );
        response.set_header("Location", location);
        return response;
    }
    if status == 204 || status == 304 {
        return Response::empty(status);
    }
    let body = value.unwrap_or_default();
    if request.method.eq_ignore_ascii_case("HEAD") {
        let mut response = Response::new(status);
        response.set_header("Content-Type", "text/plain");
        response.set_header("Content-Length", body.len().to_string());
        return response;
    }
    Response::bytes(status, "text/plain", body.into_bytes())
}
