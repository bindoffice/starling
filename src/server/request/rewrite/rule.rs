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
    let replacement = expand_replacement(&rule.replacement, &captures, variables);
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
            let old_uri = env.uri.clone();
            let old_args = env.args.clone();
            update_uri(env, &replacement);
            log_rewritten_data(env, variables);
            *uri_changed = false;
            if env.uri != old_uri || env.args != old_args {
                env.valid_location = false;
            }
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

fn expand_replacement(
    replacement: &str,
    captures: &crate::config::Captures<'_>,
    variables: &Variables,
) -> String {
    match replacement.split_once('?') {
        Some((path, args)) => format!(
            "{}?{}",
            expand_captures(path, captures, variables),
            expand_captures_in_args(args, captures, variables)
        ),
        None => expand_captures(replacement, captures, variables),
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
    let location = unescape_redirect_path(&location);
    let location = append_args(location, args);
    let body = format!(
        "<html><body>Redirecting to <a href=\"{replacement}\">{replacement}</a></body></html>"
    );
    let mut response = Response::html(status, body);
    response.set_header("Location", location);
    response
}

fn unescape_redirect_path(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            output.push(bytes[index]);
            index += 1;
            continue;
        }
        if index + 2 >= bytes.len() {
            index += 1;
            continue;
        }
        let Some(high) = hex_value(bytes[index + 1]) else {
            index += 1;
            continue;
        };
        let Some(low) = hex_value(bytes[index + 2]) else {
            index += 1;
            continue;
        };
        let decoded = (high << 4) | low;
        if decoded == b'?' {
            output.push(decoded);
            output.extend_from_slice(&bytes[index + 3..]);
            break;
        }
        if decoded > b'%' && decoded < 0x7f {
            output.push(decoded);
        } else {
            output.extend_from_slice(&bytes[index..index + 3]);
        }
        index += 3;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
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
        Some((path, args)) => {
            let args = match original_args.as_deref() {
                Some(original) if !original.is_empty() && !args.is_empty() => {
                    Some(format!("{args}&{original}"))
                }
                Some(original) if !original.is_empty() => Some(original.to_string()),
                _ => (!args.is_empty()).then(|| args.to_string()),
            };
            (path.to_string(), args)
        }
        None => (replacement.to_string(), original_args.clone()),
    }
}

pub(in crate::server) fn append_args(mut target: String, args: Option<String>) -> String {
    if let Some(args) = args
        && !args.is_empty()
    {
        target.push(if target.contains('?') { '&' } else { '?' });
        target.push_str(&args);
    }
    target
}

pub(in crate::server) fn apply_return(
    status: u16,
    value: Option<&str>,
    variables: &Variables,
    request: &Request,
    context: &EffectiveConfig,
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
    if status >= 400 && value.is_none() {
        return error_response(status);
    }
    let body = value.unwrap_or_default();
    if request.method.eq_ignore_ascii_case("HEAD") {
        let mut response = Response::new(status);
        response.set_header(
            "Content-Type",
            content_type(Path::new(request.path_and_query().0), context),
        );
        response.set_header("Content-Length", body.len().to_string());
        return response;
    }
    Response::bytes(
        status,
        &content_type(Path::new(request.path_and_query().0), context),
        body.into_bytes(),
    )
}
