use super::*;

mod conditional;
mod limit_except;
mod listen;
mod location;
mod rewrite;
mod server_name;

use conditional::compile_conditional;
pub(super) use conditional::parse_set;
use limit_except::compile_limit_except;
pub(super) use listen::{
    parse_listen, parse_port, validate_all_listen_conflicts, validate_listen_conflicts,
    validate_stream_listen_options,
};
use location::parse_location_matcher;
use rewrite::compile_rewrite_script;
pub(super) use server_name::parse_server_name;

pub(super) fn compile_server(
    directive: &Directive,
    parent: &EffectiveConfig,
    prefix: &Path,
    id: usize,
) -> ConfigResult<ServerConfig> {
    let mut context = parent.clone();
    let mut listens = Vec::new();
    let mut http2 = context.http2;
    let mut http2_seen = false;
    let mut names = Vec::new();
    let mut rewrite_directives = Vec::new();
    let mut location_directives = Vec::new();
    let mut seen = SeenScopeDirectives::default();

    for child in &directive.children {
        match child.name.as_str() {
            "listen" => {
                let parsed = parse_listen(child, prefix)?;
                for listen in parsed {
                    if listen.udp {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            "UDP listen sockets are not supported in http context",
                        ));
                    }
                    validate_listen_conflicts(&listen, &listens, &child.location)?;
                    listens.push(listen);
                }
            }
            "server_name" => {
                validate_no_block(child)?;
                for name in &child.args {
                    names.push(parse_server_name(name, &child.location)?);
                }
            }
            "http2" => {
                if http2_seen {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"http2\" directive is duplicate",
                    ));
                }
                http2_seen = true;
                http2 = parse_flag(child, "http2")?;
            }
            "location" => {
                require_block(child)?;
                location_directives.push(child);
            }
            "types" => {
                require_block(child)?;
                compile_types(child, &mut context.mime_types)?;
            }
            "if" => {
                require_block(child)?;
                rewrite_directives.push(child);
            }
            "set" | "rewrite" | "return" | "break" => {
                rewrite_directives.push(child);
            }
            _ => apply_scope_directive(child, &mut context, prefix, Scope::Server, &mut seen)?,
        }
    }
    finalize_scope_directives(&mut context, &mut seen);
    validate_upstream_buffering(&context, &directive.location)?;
    validate_charset_map(&context, &directive.location)?;
    compile_rewrite_script(&rewrite_directives, &mut context, prefix, Scope::Server)?;

    if listens.is_empty() {
        listens.push(ListenConfig {
            address: ListenAddress::Inet(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80)),
            ssl: false,
            http2: false,
            udp: false,
            proxy_protocol: false,
            default_server: false,
            socket_options: ListenSocketOptions::default(),
            socket_options_set: false,
        });
    }
    if names.is_empty() {
        names.push(ServerName::Exact(String::new()));
    }
    validate_ssl_client_verification(
        &directive.location,
        context.ssl_verify_client,
        context.ssl_client_certificate.as_ref(),
        context.ssl_trusted_certificate.as_ref(),
    )?;
    validate_ssl_stapling(&directive.location, &context)?;
    finalize_secure_link(
        &mut context.secure_link,
        &seen.secure_link,
        &directive.location,
    )?;
    validate_valid_referers(&context.valid_referers, &directive.location)?;

    // Locations inherit the final server context regardless of declaration order.
    let locations = location_directives
        .into_iter()
        .map(|location| compile_location(location, &context, prefix))
        .collect::<ConfigResult<Vec<_>>>()?;

    Ok(ServerConfig {
        id,
        listens,
        http2,
        names,
        context: Arc::new(context),
        locations,
    })
}

pub(super) fn compile_location(
    directive: &Directive,
    parent: &EffectiveConfig,
    prefix: &Path,
) -> ConfigResult<LocationConfig> {
    let matcher = parse_location_matcher(directive)?;
    let mut context = parent.clone();
    context.internal = false;
    context.flv = false;
    context.return_directive = None;
    context.try_files = None;
    context.proxy_pass = None;
    context.grpc.pass = None;
    context.fastcgi_pass = None;
    context.uwsgi_pass = None;
    context.scgi_pass = None;
    context.memcached_pass = None;
    context.rewrites.clear();
    context.rewrite_actions.clear();
    context.conditionals.clear();
    context.sets.clear();
    context.limit_except = None;
    let mut nested_directives = Vec::new();
    let mut rewrite_directives = Vec::new();
    let mut limit_except_directive = None;
    let mut seen = SeenScopeDirectives::default();

    for child in &directive.children {
        if child.name == "location" {
            require_block(child)?;
            nested_directives.push(child);
        } else if child.name == "types" {
            require_block(child)?;
            compile_types(child, &mut context.mime_types)?;
        } else if child.name == "if" {
            require_block(child)?;
            rewrite_directives.push(child);
        } else if matches!(child.name.as_str(), "set" | "rewrite" | "return" | "break") {
            rewrite_directives.push(child);
        } else if child.name == "limit_except" {
            require_block(child)?;
            if limit_except_directive.is_some() {
                return Err(ConfigError::at(
                    child.location.clone(),
                    "\"limit_except\" directive is duplicate",
                ));
            }
            limit_except_directive = Some(child);
        } else {
            apply_scope_directive(child, &mut context, prefix, Scope::Location, &mut seen)?;
        }
    }
    finalize_scope_directives(&mut context, &mut seen);
    if directive.children.iter().any(|child| child.name == "alias") {
        context.alias_uri_prefix = match &matcher {
            LocationMatcher::Exact(path)
            | LocationMatcher::Prefix(path)
            | LocationMatcher::PrefixNoRegex(path) => Some(path.clone()),
            LocationMatcher::Regex(_) => Some(String::new()),
            LocationMatcher::Named(_) => None,
        };
    }
    if context.alias.is_none() {
        context.alias_uri_prefix = None;
    }
    validate_upstream_buffering(&context, &directive.location)?;
    validate_charset_map(&context, &directive.location)?;
    compile_rewrite_script(&rewrite_directives, &mut context, prefix, Scope::Location)?;
    if let Some(directive) = limit_except_directive {
        context.limit_except = Some(compile_limit_except(directive, &context, prefix)?);
    }

    if matches!(
        matcher,
        LocationMatcher::Regex(_) | LocationMatcher::Named(_)
    ) && let Some(proxy) = &context.proxy_pass
        && proxy.uri.is_some()
        && !proxy.raw.contains('$')
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"proxy_pass\" cannot have URI part in location given by regular expression or named location",
        ));
    }
    finalize_secure_link(
        &mut context.secure_link,
        &seen.secure_link,
        &directive.location,
    )?;
    validate_valid_referers(&context.valid_referers, &directive.location)?;

    let nested = nested_directives
        .into_iter()
        .map(|location| compile_location(location, &context, prefix))
        .collect::<ConfigResult<Vec<_>>>()?;

    Ok(LocationConfig {
        matcher,
        context: Arc::new(context),
        nested,
        location: directive.location.clone(),
    })
}
