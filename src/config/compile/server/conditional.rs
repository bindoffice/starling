use super::*;

mod condition;

use condition::{parse_condition, require_location_scope};

pub(super) fn compile_conditional(
    directive: &Directive,
    parent: &EffectiveConfig,
    prefix: &Path,
    scope: Scope,
) -> ConfigResult<ConditionalConfig> {
    let condition = parse_condition(directive)?;
    let mut add_headers = Vec::new();
    let mut add_trailers = Vec::new();
    let mut add_header_inherit = parent.add_header_inherit;
    let mut add_trailer_inherit = parent.add_trailer_inherit;
    let mut charset = parent.charset.clone();
    let mut charset_seen = CharsetSeen::default();
    let mut access_log = parent.access_log.clone();
    let mut access_log_seen = false;
    let mut error_pages = parent.error_pages.clone();
    let mut seen_error_page = false;
    let mut seen_ssi = false;
    let mut seen_expires = false;
    let mut seen_gzip = false;
    let mut seen_root = false;
    let mut seen_sendfile = false;
    let mut seen_limit_rate = false;
    let mut seen_limit_rate_after = false;
    let mut seen_post_action = false;
    let mut seen_proxy_pass = false;
    let mut seen_grpc_pass = false;
    let mut seen_fastcgi_pass = false;
    let mut seen_uwsgi_pass = false;
    let mut seen_scgi_pass = false;
    let mut seen_memcached_pass = false;
    let mut seen_tunnel_pass = false;
    let mut seen_header_inherit = false;
    let mut seen_trailer_inherit = false;
    let mut config = ConditionalConfig {
        condition,
        actions: Vec::new(),
        explicit_content_handler: None,
        ssi: parent.ssi.clone(),
        return_directive: None,
        rewrites: Vec::new(),
        proxy_pass: parent.proxy_pass.clone(),
        tunnel_pass: parent.tunnel.pass.clone(),
        grpc_pass: parent.grpc.pass.clone(),
        fastcgi_pass: parent.fastcgi_pass.clone(),
        uwsgi_pass: parent.uwsgi_pass.clone(),
        scgi_pass: parent.scgi_pass.clone(),
        memcached_pass: parent.memcached_pass.clone(),
        root: parent.root.clone(),
        alias: parent.alias.clone(),
        sendfile: parent.sendfile,
        limit_rate: parent.limit_rate.clone(),
        limit_rate_after: parent.limit_rate_after.clone(),
        error_pages: parent.error_pages.clone(),
        post_action: parent.post_action.clone(),
        gzip: parent.gzip,
        charset: parent.charset.clone(),
        access_log: parent.access_log.clone(),
        add_headers: Vec::new(),
        add_header_inherit,
        add_trailers: Vec::new(),
        add_trailer_inherit,
        expires: parent.expires.clone(),
        access_rules: parent.access_rules.clone(),
        sets: Vec::new(),
        rewrite_log: parent.rewrite_log,
        uninitialized_variable_warn: parent.uninitialized_variable_warn,
    };
    for child in &directive.children {
        match child.name.as_str() {
            "ssi" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                if seen_ssi {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"ssi\" directive is duplicate",
                    ));
                }
                seen_ssi = true;
                let mut ssi = parent.ssi.clone();
                ssi.enabled = parse_flag(child, "ssi")?;
                config.ssi = ssi;
            }
            "return" => {
                validate_no_block(child)?;
                let rule = parse_return(child)?;
                config.return_directive = Some(rule.clone());
                config.actions.push(RewriteAction::Return(rule));
            }
            "rewrite" => {
                validate_no_block(child)?;
                let rule = parse_rewrite(child)?;
                config.rewrites.push(rule.clone());
                config.actions.push(RewriteAction::Rewrite(rule));
            }
            "break" => {
                validate_no_block(child)?;
                if !child.args.is_empty() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"break\" directive",
                    ));
                }
                config.actions.push(RewriteAction::Break);
            }
            "proxy_pass" => {
                require_location_scope(child, scope)?;
                if seen_proxy_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"proxy_pass\" directive is duplicate",
                    ));
                }
                seen_proxy_pass = true;
                validate_no_block(child)?;
                config.proxy_pass = Some(parse_proxy_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::Proxy);
                config.grpc_pass = None;
                config.fastcgi_pass = None;
                config.uwsgi_pass = None;
                config.scgi_pass = None;
                config.memcached_pass = None;
                config.tunnel_pass = None;
            }
            "grpc_pass" => {
                require_location_scope(child, scope)?;
                if seen_grpc_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"grpc_pass\" directive is duplicate",
                    ));
                }
                seen_grpc_pass = true;
                validate_no_block(child)?;
                config.grpc_pass = Some(parse_grpc_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::Grpc);
                config.proxy_pass = None;
                config.fastcgi_pass = None;
                config.uwsgi_pass = None;
                config.scgi_pass = None;
                config.memcached_pass = None;
                config.tunnel_pass = None;
            }
            "fastcgi_pass" => {
                require_location_scope(child, scope)?;
                if seen_fastcgi_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"fastcgi_pass\" directive is duplicate",
                    ));
                }
                seen_fastcgi_pass = true;
                validate_no_block(child)?;
                config.fastcgi_pass = Some(parse_fastcgi_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::FastCgi);
                config.proxy_pass = None;
                config.grpc_pass = None;
                config.uwsgi_pass = None;
                config.scgi_pass = None;
                config.memcached_pass = None;
                config.tunnel_pass = None;
            }
            "uwsgi_pass" => {
                require_location_scope(child, scope)?;
                if seen_uwsgi_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"uwsgi_pass\" directive is duplicate",
                    ));
                }
                seen_uwsgi_pass = true;
                validate_no_block(child)?;
                config.uwsgi_pass = Some(parse_uwsgi_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::Uwsgi);
                config.proxy_pass = None;
                config.grpc_pass = None;
                config.fastcgi_pass = None;
                config.scgi_pass = None;
                config.memcached_pass = None;
                config.tunnel_pass = None;
            }
            "scgi_pass" => {
                require_location_scope(child, scope)?;
                if seen_scgi_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"scgi_pass\" directive is duplicate",
                    ));
                }
                seen_scgi_pass = true;
                validate_no_block(child)?;
                config.scgi_pass = Some(parse_scgi_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::Scgi);
                config.proxy_pass = None;
                config.grpc_pass = None;
                config.fastcgi_pass = None;
                config.uwsgi_pass = None;
                config.memcached_pass = None;
                config.tunnel_pass = None;
            }
            "memcached_pass" => {
                require_location_scope(child, scope)?;
                if seen_memcached_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"memcached_pass\" directive is duplicate",
                    ));
                }
                seen_memcached_pass = true;
                validate_no_block(child)?;
                config.memcached_pass = Some(MemcachedPassConfig {
                    raw: parse_gateway_pass(child, "memcached")?,
                });
                config.explicit_content_handler = Some(ContentHandlerKind::Memcached);
                config.proxy_pass = None;
                config.grpc_pass = None;
                config.fastcgi_pass = None;
                config.uwsgi_pass = None;
                config.scgi_pass = None;
                config.tunnel_pass = None;
            }
            "tunnel_pass" => {
                require_location_scope(child, scope)?;
                if seen_tunnel_pass {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"tunnel_pass\" directive is duplicate",
                    ));
                }
                seen_tunnel_pass = true;
                config.tunnel_pass = Some(parse_tunnel_pass(child)?);
                config.explicit_content_handler = Some(ContentHandlerKind::Tunnel);
                config.proxy_pass = None;
                config.grpc_pass = None;
                config.fastcgi_pass = None;
                config.uwsgi_pass = None;
                config.scgi_pass = None;
                config.memcached_pass = None;
            }
            "root" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                if seen_root {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"root\" directive is duplicate",
                    ));
                }
                seen_root = true;
                config.root = resolve_path(prefix, one_arg(child)?);
                config.alias = None;
            }
            "sendfile" => {
                require_location_scope(child, scope)?;
                if seen_sendfile {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"sendfile\" directive is duplicate",
                    ));
                }
                seen_sendfile = true;
                config.sendfile = parse_flag(child, "sendfile")?;
            }
            "limit_rate" => {
                require_location_scope(child, scope)?;
                if seen_limit_rate {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"limit_rate\" directive is duplicate",
                    ));
                }
                seen_limit_rate = true;
                config.limit_rate = Some(parse_complex_size(child, "limit_rate")?);
            }
            "limit_rate_after" => {
                require_location_scope(child, scope)?;
                if seen_limit_rate_after {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"limit_rate_after\" directive is duplicate",
                    ));
                }
                seen_limit_rate_after = true;
                config.limit_rate_after = Some(parse_complex_size(child, "limit_rate_after")?);
            }
            "error_page" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                if !seen_error_page {
                    error_pages.clear();
                    seen_error_page = true;
                }
                error_pages.push(parse_error_page(child)?);
                config.error_pages = error_pages.clone();
            }
            "post_action" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                if seen_post_action {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"post_action\" directive is duplicate",
                    ));
                }
                seen_post_action = true;
                config.post_action = Some(one_arg(child)?.to_string());
            }
            "add_header" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                add_headers.push(parse_add_header(child)?);
            }
            "add_trailer" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                add_trailers.push(parse_add_header(child)?);
            }
            "add_header_inherit" => {
                require_location_scope(child, scope)?;
                if seen_header_inherit {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"add_header_inherit\" directive is duplicate",
                    ));
                }
                seen_header_inherit = true;
                add_header_inherit = parse_header_inheritance(child)?;
            }
            "add_trailer_inherit" => {
                require_location_scope(child, scope)?;
                if seen_trailer_inherit {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"add_trailer_inherit\" directive is duplicate",
                    ));
                }
                seen_trailer_inherit = true;
                add_trailer_inherit = parse_header_inheritance(child)?;
            }
            "expires" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                if seen_expires {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"expires\" directive is duplicate",
                    ));
                }
                seen_expires = true;
                config.expires = Some(parse_expires(child)?);
            }
            "gzip" => {
                require_location_scope(child, scope)?;
                if seen_gzip {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"gzip\" directive is duplicate",
                    ));
                }
                seen_gzip = true;
                config.gzip = parse_flag(child, "gzip")?;
            }
            "charset" | "source_charset" | "override_charset" => {
                require_location_scope(child, scope)?;
                try_apply_charset_directive(child, &mut charset, &mut charset_seen)?;
                config.charset = charset.clone();
            }
            "access_log" => {
                require_location_scope(child, scope)?;
                apply_access_log_directive(
                    child,
                    prefix,
                    Some("combined"),
                    &mut access_log,
                    &mut access_log_seen,
                )?;
                config.access_log = access_log.clone();
            }
            "allow" | "deny" => {
                require_location_scope(child, scope)?;
                validate_no_block(child)?;
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("\"{}\" directive is not allowed here", child.name),
                ));
            }
            "set" => {
                let set = parse_set(child)?;
                config.sets.push(set.clone());
                config.actions.push(RewriteAction::Set(set));
            }
            "rewrite_log" => config.rewrite_log = parse_flag(child, "rewrite_log")?,
            "uninitialized_variable_warn" => {
                config.uninitialized_variable_warn =
                    parse_flag(child, "uninitialized_variable_warn")?;
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in \"if\" context"),
                ));
            }
        }
    }
    let mut inherited_headers = parent.add_headers.clone();
    let mut inherited_trailers = parent.add_trailers.clone();
    add_header_inherit.merge(&mut inherited_headers, add_headers);
    add_trailer_inherit.merge(&mut inherited_trailers, add_trailers);
    config.add_headers = inherited_headers;
    config.add_header_inherit = add_header_inherit;
    config.add_trailers = inherited_trailers;
    config.add_trailer_inherit = add_trailer_inherit;
    config.charset = charset;
    config.access_log = access_log;
    config.error_pages = error_pages;
    validate_charset_values(&config.charset, &parent.charset_maps, &directive.location)?;
    if seen_proxy_pass
        && let Some(proxy) = &config.proxy_pass
        && proxy.uri.is_some()
        && !proxy.raw.contains('$')
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"proxy_pass\" cannot have URI part inside \"if\" statement",
        ));
    }
    Ok(config)
}

pub(in crate::config::compile) fn parse_set(directive: &Directive) -> ConfigResult<SetVariable> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"set\" directive",
        ));
    }
    let Some(name) = directive.args[0].strip_prefix('$') else {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid variable name in \"set\" directive",
        ));
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid variable name in \"set\" directive",
        ));
    }
    Ok(SetVariable {
        name: name.to_string(),
        value: directive.args[1].clone(),
    })
}
