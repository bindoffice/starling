use super::*;

pub(in crate::config::compile) fn apply_proxy_redirect(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if *seen && matches!(context.proxy_redirect, ProxyRedirectConfig::Off) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"proxy_redirect\" directive is duplicate",
        ));
    }
    if !*seen {
        context.proxy_redirect = ProxyRedirectConfig::Rules(Vec::new());
        *seen = true;
    }
    if directive.args.len() == 1 {
        match directive.args[0].as_str() {
            "off" => {
                if matches!(
                    context.proxy_redirect,
                    ProxyRedirectConfig::Rules(ref rules) if !rules.is_empty()
                ) {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"proxy_redirect\" directive is duplicate",
                    ));
                }
                context.proxy_redirect = ProxyRedirectConfig::Off;
                return Ok(());
            }
            "default" => {
                let Some(proxy) = &context.proxy_pass else {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"proxy_redirect default\" should be placed after the \"proxy_pass\" directive",
                    ));
                };
                if proxy.raw.contains('$') {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"proxy_redirect default\" cannot be used with \"proxy_pass\" directive with variables",
                    ));
                }
                let ProxyRedirectConfig::Rules(rules) = &mut context.proxy_redirect else {
                    unreachable!("proxy_redirect was initialized above");
                };
                rules.push(ProxyRedirectRule::Default);
                return Ok(());
            }
            _ => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid parameter in \"proxy_redirect\" directive",
                ));
            }
        }
    }
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"proxy_redirect\" directive",
        ));
    }
    let rule = ProxyRedirectRule::Pattern {
        pattern: directive.args[0].clone(),
        replacement: directive.args[1].clone(),
    };
    let ProxyRedirectConfig::Rules(rules) = &mut context.proxy_redirect else {
        unreachable!("proxy_redirect was initialized above");
    };
    rules.push(rule);
    Ok(())
}

#[derive(Clone, Copy)]
pub(in crate::config::compile) enum ProxyCookieKind {
    Domain,
    Path,
}

pub(in crate::config::compile) fn apply_proxy_cookie(
    directive: &Directive,
    context: &mut EffectiveConfig,
    kind: ProxyCookieKind,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    let first = !*seen;
    *seen = true;
    let rules = match kind {
        ProxyCookieKind::Domain => &mut context.proxy_cookie_domains,
        ProxyCookieKind::Path => &mut context.proxy_cookie_paths,
    };
    if first {
        rules.clear();
    }
    if directive.args.len() == 1 {
        if directive.args[0] != "off" {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("invalid parameter \"{}\"", directive.args[0]),
            ));
        }
        if !first {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("{} directive is duplicate", directive.name),
            ));
        }
        return Ok(());
    }
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    let mut pattern = directive.args[0].clone();
    let mut replacement = directive.args[1].clone();
    if matches!(kind, ProxyCookieKind::Domain) && !pattern.starts_with('~') {
        pattern = pattern.strip_prefix('.').unwrap_or(&pattern).to_string();
        replacement = replacement
            .strip_prefix('.')
            .unwrap_or(&replacement)
            .to_string();
    }
    rules.push(ProxyCookieRule {
        pattern,
        replacement,
    });
    Ok(())
}

pub(super) fn try_apply_proxy_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    _prefix: &Path,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if try_apply_proxy_hash_directive(directive, context, seen)? {
        return Ok(true);
    }
    match directive.name.as_str() {
        "proxy_set_header" => {
            validate_no_block(directive)?;
            if !seen.proxy_header {
                context.proxy_headers.clear();
                seen.proxy_header = true;
            }
            if directive.args.len() != 2 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"proxy_set_header\" directive",
                ));
            }
            context.proxy_headers.push(HeaderValue {
                name: directive.args[0].clone(),
                value: directive.args[1].clone(),
            });
        }
        "proxy_hide_header" => {
            validate_no_block(directive)?;
            if !seen.proxy_hide_header {
                context.proxy_hide_headers.clear();
                seen.proxy_hide_header = true;
            }
            context
                .proxy_hide_headers
                .push(one_arg(directive)?.to_string());
        }
        "proxy_pass_header" => {
            validate_no_block(directive)?;
            if !seen.proxy_pass_header {
                context.proxy_pass_headers.clear();
                seen.proxy_pass_header = true;
            }
            context
                .proxy_pass_headers
                .push(one_arg(directive)?.to_string());
        }
        "proxy_ignore_headers" => apply_ignore_headers(
            directive,
            &mut context.proxy_ignore_headers,
            &mut seen.proxy_ignore_headers,
        )?,
        "proxy_pass_request_headers" => {
            context.proxy_pass_request_headers =
                parse_flag(directive, "proxy_pass_request_headers")?;
        }
        "proxy_pass_request_body" => {
            context.proxy_pass_request_body = parse_flag(directive, "proxy_pass_request_body")?;
        }
        "proxy_request_buffering" => {
            if seen.proxy_request_buffering {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"proxy_request_buffering\" directive is duplicate",
                ));
            }
            context.proxy_request_buffering = parse_flag(directive, "proxy_request_buffering")?;
            seen.proxy_request_buffering = true;
        }
        "proxy_ignore_client_abort" => {
            if seen.proxy_ignore_client_abort {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"proxy_ignore_client_abort\" directive is duplicate",
                ));
            }
            context.proxy_ignore_client_abort = parse_flag(directive, "proxy_ignore_client_abort")?;
            seen.proxy_ignore_client_abort = true;
        }
        "proxy_pass_trailers" => {
            context.proxy_pass_trailers = parse_flag(directive, "proxy_pass_trailers")?;
        }
        "proxy_method" => {
            validate_no_block(directive)?;
            context.proxy_method = Some(one_arg(directive)?.to_string());
        }
        "proxy_set_body" => {
            validate_no_block(directive)?;
            context.proxy_set_body = Some(one_arg(directive)?.to_string());
        }
        "proxy_intercept_errors" => {
            context.proxy_intercept_errors = parse_flag(directive, "proxy_intercept_errors")?;
        }
        "proxy_force_ranges" => {
            context.proxy_force_ranges = parse_flag(directive, "proxy_force_ranges")?;
        }
        "proxy_limit_rate" => {
            context.proxy_limit_rate = Some(parse_complex_size(directive, "proxy_limit_rate")?);
        }
        "proxy_cookie_domain" => {
            apply_proxy_cookie(
                directive,
                context,
                ProxyCookieKind::Domain,
                &mut seen.proxy_cookie_domain,
            )?;
        }
        "proxy_cookie_path" => {
            apply_proxy_cookie(
                directive,
                context,
                ProxyCookieKind::Path,
                &mut seen.proxy_cookie_path,
            )?;
        }
        "proxy_cookie_flags" => {
            apply_proxy_cookie_flags(directive, context, &mut seen.proxy_cookie_flags)?;
        }
        "proxy_store" => {
            apply_store_value(directive, &mut context.proxy_store, &mut seen.proxy_store)?;
        }
        "proxy_store_access" => {
            apply_store_access(
                directive,
                &mut context.proxy_store_access,
                &mut seen.proxy_store_access,
            )?;
        }
        "proxy_redirect" => {
            apply_proxy_redirect(directive, context, &mut seen.proxy_redirect)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
