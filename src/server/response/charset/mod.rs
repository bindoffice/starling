use super::*;

mod recode;

pub(in crate::server) async fn apply_charset(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    let Some(content_type) = response.header("Content-Type").map(str::to_string) else {
        return;
    };
    let (mime_type, existing_charset) = parse_content_type(&content_type);
    if mime_type.is_empty() {
        return;
    }
    let override_charset = response
        .upstream_control()
        .and_then(|control| control.charset.clone());

    let target = if let Some(charset) = override_charset {
        if !charset_known(&charset, context) {
            return;
        }
        charset
    } else {
        let Some(charset) = &context.charset.charset else {
            return;
        };
        if existing_charset.is_some() && !context.charset.override_charset {
            return;
        }
        if existing_charset.is_none() && !type_is_allowed(mime_type, &context.charset.types) {
            return;
        }
        let Some(charset) = resolve_charset(charset, variables) else {
            return;
        };
        charset
    };

    let configured_source = context
        .charset
        .source_charset
        .as_deref()
        .and_then(|value| resolve_charset(value, variables));
    let source = existing_charset.map(str::to_string).or(configured_source);
    let Some(source) = source else {
        set_content_type(response, mime_type, &target);
        return;
    };
    if source.eq_ignore_ascii_case(&target) {
        set_content_type(response, mime_type, &target);
        return;
    }
    if content_is_encoded(response) {
        return;
    }
    let Some(conversion) = context
        .charset_maps
        .iter()
        .find_map(|map| map.conversion(&source, &target))
    else {
        return;
    };
    if response.body_len() > MAX_BODY_BYTES as u64 {
        return;
    }
    let original = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let body = match original.into_memory(MAX_BODY_BYTES as u64).await {
        Ok(body) => body,
        Err(original) => {
            response.body = *original;
            return;
        }
    };
    response.body = ResponseBody::Bytes(recode::convert(body, conversion));
    set_content_type(response, mime_type, &target);
    response.set_content_length();
}

fn parse_content_type(value: &str) -> (&str, Option<&str>) {
    let mut parts = value.split(';');
    let mime_type = parts.next().unwrap_or_default().trim();
    let charset = parts.find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"'))
    });
    (mime_type, charset)
}

fn resolve_charset(value: &str, variables: &Variables) -> Option<String> {
    let resolved = if value.starts_with('$') {
        eval_variables(value, variables)
    } else {
        value.to_string()
    };
    (!resolved.is_empty()).then_some(resolved)
}

fn charset_known(name: &str, context: &EffectiveConfig) -> bool {
    name.eq_ignore_ascii_case("utf-8")
        || context
            .charset
            .charset
            .as_deref()
            .is_some_and(|charset| charset.eq_ignore_ascii_case(name))
        || context
            .charset
            .source_charset
            .as_deref()
            .is_some_and(|charset| charset.eq_ignore_ascii_case(name))
        || context.charset_maps.iter().any(|map| map.includes(name))
}

fn type_is_allowed(mime_type: &str, allowed_types: &[String]) -> bool {
    allowed_types.iter().any(|allowed| {
        if allowed == "*" {
            return true;
        }
        if let Some(major) = allowed.strip_suffix("/*") {
            return mime_type
                .split_once('/')
                .is_some_and(|(candidate, _)| candidate.eq_ignore_ascii_case(major));
        }
        allowed.eq_ignore_ascii_case(mime_type)
    })
}

fn content_is_encoded(response: &Response) -> bool {
    response
        .header("Content-Encoding")
        .is_some_and(|value| !value.is_empty())
}

fn set_content_type(response: &mut Response, mime_type: &str, charset: &str) {
    if matches!(response.status, 301 | 302) {
        response.set_header("Content-Type", mime_type);
    } else {
        response.set_header("Content-Type", format!("{mime_type}; charset={charset}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_type_parameters() {
        assert_eq!(
            parse_content_type("text/plain; boundary=x; charset=\"utf-8\""),
            ("text/plain", Some("utf-8"))
        );
        assert_eq!(parse_content_type("text/html"), ("text/html", None));
    }

    #[test]
    fn matches_exact_and_wildcard_types() {
        let types = vec!["text/*".into(), "application/json".into()];
        assert!(type_is_allowed("text/plain", &types));
        assert!(type_is_allowed("application/json", &types));
        assert!(!type_is_allowed("image/png", &types));
    }
}
