use super::*;

pub(in crate::server) const DEFAULT_PROXY_HIDE_HEADERS: &[&str] = &[
    "date",
    "server",
    "x-pad",
    "x-accel-expires",
    "x-accel-redirect",
    "x-accel-limit-rate",
    "x-accel-buffering",
    "x-accel-charset",
];

pub(in crate::server) fn filter_proxy_response_headers(
    headers: Vec<Header>,
    context: &EffectiveConfig,
) -> Vec<Header> {
    filter_proxy_headers(
        headers,
        &context.proxy_pass_headers,
        &context.proxy_hide_headers,
    )
}

pub(in crate::server) fn filter_proxy_trailer_headers(
    headers: Vec<Header>,
    pass_headers: &[String],
    hide_headers: &[String],
) -> Vec<Header> {
    filter_proxy_headers(headers, pass_headers, hide_headers)
}

fn filter_proxy_headers(
    headers: Vec<Header>,
    pass_headers: &[String],
    hide_headers: &[String],
) -> Vec<Header> {
    headers
        .into_iter()
        .filter(|header| {
            let name = header.name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "connection" | "keep-alive" | "proxy-connection" | "transfer-encoding"
            ) {
                return false;
            }
            if pass_headers
                .iter()
                .any(|header| header.eq_ignore_ascii_case(&name))
            {
                return true;
            }
            !DEFAULT_PROXY_HIDE_HEADERS.contains(&name.as_str())
                && !hide_headers
                    .iter()
                    .any(|header| header.eq_ignore_ascii_case(&name))
        })
        .collect()
}

pub(in crate::server) fn apply_proxy_redirect(
    response: &mut Response,
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    for header in &mut response.headers {
        if header.name.eq_ignore_ascii_case("location") {
            if let Some(value) =
                proxy_redirect_value(&header.value, proxy, location, context, variables)
            {
                header.value = value;
            }
        } else if header.name.eq_ignore_ascii_case("refresh")
            && let Some(position) = header.value.to_ascii_lowercase().find("url=")
        {
            let value_start = position + "url=".len();
            if let Some(value) = proxy_redirect_value(
                &header.value[value_start..],
                proxy,
                location,
                context,
                variables,
            ) {
                header.value.replace_range(value_start.., &value);
            }
        }
    }
}

pub(in crate::server) fn proxy_redirect_value(
    value: &str,
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
    context: &EffectiveConfig,
    variables: &Variables,
) -> Option<String> {
    match &context.proxy_redirect {
        ProxyRedirectConfig::Off => None,
        ProxyRedirectConfig::Rules(rules) => rules.iter().find_map(|rule| match rule {
            ProxyRedirectRule::Default => {
                let (pattern, replacement) = default_proxy_redirect_rule(proxy, location)?;
                apply_proxy_redirect_rule(value, &pattern, &replacement, variables)
            }
            ProxyRedirectRule::Pattern {
                pattern,
                replacement,
            } => apply_proxy_redirect_rule(value, pattern, replacement, variables),
        }),
    }
}

pub(in crate::server) fn default_proxy_redirect_rule(
    proxy: &ProxyPassConfig,
    location: Option<&LocationConfig>,
) -> Option<(String, String)> {
    if proxy.raw.contains('$') {
        return None;
    }
    if proxy.uri.is_none() {
        return Some((format!("{}/", proxy.raw), "/".into()));
    }
    let location = location?;
    let replacement = match &location.matcher {
        LocationMatcher::Exact(path)
        | LocationMatcher::Prefix(path)
        | LocationMatcher::PrefixNoRegex(path) => path.clone(),
        LocationMatcher::Regex(_) | LocationMatcher::Named(_) => return None,
    };
    Some((proxy.raw.clone(), replacement))
}

pub(in crate::server) fn apply_proxy_redirect_rule(
    value: &str,
    pattern: &str,
    replacement: &str,
    variables: &Variables,
) -> Option<String> {
    if let Some(pattern) = pattern.strip_prefix('~') {
        let (pattern, case_insensitive) = match pattern.strip_prefix('*') {
            Some(pattern) => (pattern, true),
            None => (pattern, false),
        };
        let regex = crate::config::RegexBuilder::new(pattern)
            .case_insensitive(case_insensitive)
            .build()
            .ok()?;
        let captures = regex.captures(value)?;
        return Some(expand_captures(replacement, &captures, variables));
    }
    let pattern = eval_variables(pattern, variables);
    let replacement = eval_variables(replacement, variables);
    if value.starts_with(&pattern) {
        return Some(format!("{replacement}{}", &value[pattern.len()..]));
    }
    None
}

pub(in crate::server) fn apply_proxy_cookie_rewrites(
    response: &mut Response,
    context: &EffectiveConfig,
    variables: &Variables,
) {
    for header in &mut response.headers {
        if !header.name.eq_ignore_ascii_case("set-cookie") {
            continue;
        }
        let mut attributes = parse_proxy_cookie(&header.value);
        let mut changed = false;
        for attribute in attributes.iter_mut().skip(1) {
            let Some(cookie_value) = attribute.value.as_deref() else {
                continue;
            };
            let rewritten = if attribute.key.eq_ignore_ascii_case("domain") {
                apply_proxy_cookie_domain_rule(
                    cookie_value,
                    &context.proxy_cookie_domains,
                    variables,
                )
            } else if attribute.key.eq_ignore_ascii_case("path") {
                context.proxy_cookie_paths.iter().find_map(|rule| {
                    apply_proxy_redirect_rule(
                        cookie_value,
                        &rule.pattern,
                        &rule.replacement,
                        variables,
                    )
                })
            } else {
                None
            };
            if let Some(rewritten) = rewritten {
                attribute.value = Some(rewritten);
                changed = true;
            }
        }
        changed |=
            apply_proxy_cookie_flags(&mut attributes, &context.proxy_cookie_flags, variables);
        if changed {
            header.value = render_proxy_cookie(&attributes);
        }
    }
}

pub(in crate::server) fn apply_proxy_cookie_domain_rule(
    value: &str,
    rules: &[crate::config::ProxyCookieRule],
    variables: &Variables,
) -> Option<String> {
    for rule in rules {
        if let Some(pattern) = rule.pattern.strip_prefix('~') {
            // Cookie domains are DNS names, so nginx always matches this
            // regular expression case-insensitively.
            let pattern = pattern.strip_prefix('*').unwrap_or(pattern);
            let regex = crate::config::RegexBuilder::new(pattern)
                .case_insensitive(true)
                .build()
                .ok()?;
            let Some(captures) = regex.captures(value) else {
                continue;
            };
            return Some(expand_captures(&rule.replacement, &captures, variables));
        }
        let pattern = eval_variables(&rule.pattern, variables);
        let pattern = pattern.strip_prefix('.').unwrap_or(&pattern);
        let leading_dot = value.starts_with('.');
        let candidate = value.strip_prefix('.').unwrap_or(value);
        if candidate.eq_ignore_ascii_case(pattern) {
            let replacement = eval_variables(&rule.replacement, variables);
            let replacement = replacement.strip_prefix('.').unwrap_or(&replacement);
            return Some(format!(
                "{}{replacement}",
                if leading_dot { "." } else { "" }
            ));
        }
    }
    None
}
