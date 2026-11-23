use super::*;

mod gateway;
mod proxy_tls;
mod proxy_version;
mod upstream_socket;
mod uwsgi_tls;

use super::compat::try_apply_compat_directive;
use gateway::try_apply_gateway_directive;
use proxy_tls::try_apply_proxy_tls_directive;
use proxy_version::try_apply_proxy_http_version;
pub(in crate::config::compile) use upstream_socket::parse_upstream_bind;
use upstream_socket::try_apply_upstream_socket_directive;
use uwsgi_tls::try_apply_uwsgi_tls_directive;

pub(in crate::config::compile) fn parse_complex_size(
    directive: &Directive,
    name: &str,
) -> ConfigResult<String> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?;
    if !value.contains('$') {
        parse_size(value, &directive.location).map_err(|_| {
            ConfigError::at(
                directive.location.clone(),
                format!("invalid value \"{value}\" in \"{name}\" directive"),
            )
        })?;
    }
    Ok(value.to_string())
}

pub(super) fn apply_common_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    allow_http_only: bool,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<()> {
    if try_apply_gateway_directive(directive, context)? {
        return Ok(());
    }
    if try_apply_proxy_tls_directive(directive, context, prefix)? {
        return Ok(());
    }
    if try_apply_uwsgi_tls_directive(directive, context, prefix, seen)? {
        return Ok(());
    }
    if try_apply_upstream_socket_directive(directive, context)? {
        return Ok(());
    }
    if try_apply_proxy_http_version(directive, context)? {
        return Ok(());
    }
    if try_apply_memcached_directive(directive, context, !allow_http_only)? {
        return Ok(());
    }
    if try_apply_compat_directive(directive)? {
        return Ok(());
    }
    match directive.name.as_str() {
        "root" => {
            validate_no_block(directive)?;
            context.root = resolve_path(prefix, one_arg(directive)?);
            context.alias = None;
            context.alias_uri_prefix = None;
        }
        "alias" => {
            validate_no_block(directive)?;
            context.alias = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "index" => {
            validate_no_block(directive)?;
            context.index = directive.args.clone();
        }
        "default_type" => {
            validate_no_block(directive)?;
            context.default_type = one_arg(directive)?.to_string();
        }
        "client_max_body_size" => {
            validate_no_block(directive)?;
            context.client_max_body_size = parse_size(one_arg(directive)?, &directive.location)?;
        }
        "keepalive_timeout" => {
            validate_no_block(directive)?;
            if !(1..=2).contains(&directive.args.len()) {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"keepalive_timeout\" directive",
                ));
            }
            let seconds = parse_time_seconds(&directive.args[0], &directive.location)?;
            context.keepalive_timeout = Duration::from_secs_f64(seconds);
            if let Some(header_timeout) = directive.args.get(1) {
                let seconds = parse_time_seconds(header_timeout, &directive.location)?;
                context.keepalive_header = Some(Duration::from_secs_f64(seconds));
            }
        }
        "keepalive_time" => {
            validate_no_block(directive)?;
            context.keepalive_time = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "keepalive_requests" => {
            validate_no_block(directive)?;
            context.keepalive_requests = one_arg(directive)?.parse::<u64>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"keepalive_requests\" directive",
                )
            })?;
        }
        "client_header_timeout" => {
            if !allow_http_only {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"client_header_timeout\" directive is not allowed here",
                ));
            }
            validate_no_block(directive)?;
            context.client_header_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "client_body_timeout" => {
            validate_no_block(directive)?;
            context.client_body_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "send_timeout" => {
            validate_no_block(directive)?;
            context.send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "tcp_nodelay" => {
            if seen.tcp_nodelay {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"tcp_nodelay\" directive is duplicate",
                ));
            }
            seen.tcp_nodelay = true;
            context.tcp_nodelay = parse_flag(directive, "tcp_nodelay")?;
        }
        "absolute_redirect" => {
            context.absolute_redirect = parse_flag(directive, "absolute_redirect")?
        }
        "server_name_in_redirect" => {
            context.server_name_in_redirect = parse_flag(directive, "server_name_in_redirect")?
        }
        "port_in_redirect" => context.port_in_redirect = parse_flag(directive, "port_in_redirect")?,
        "limit_rate" => {
            context.limit_rate = Some(parse_complex_size(directive, "limit_rate")?);
        }
        "limit_rate_after" => {
            context.limit_rate_after = Some(parse_complex_size(directive, "limit_rate_after")?);
        }
        "merge_slashes" => context.merge_slashes = parse_flag(directive, "merge_slashes")?,
        "rewrite_log" => context.rewrite_log = parse_flag(directive, "rewrite_log")?,
        "uninitialized_variable_warn" => {
            context.uninitialized_variable_warn =
                parse_flag(directive, "uninitialized_variable_warn")?;
        }
        "etag" => context.etag = parse_flag(directive, "etag")?,
        "chunked_transfer_encoding" => {
            context.chunked_transfer_encoding = parse_flag(directive, "chunked_transfer_encoding")?
        }
        "gzip" => context.gzip = parse_flag(directive, "gzip")?,
        "gzip_types" => {
            validate_no_block(directive)?;
            context.gzip_types = directive
                .args
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .filter(|value| value != "text/html")
                .collect();
        }
        "gzip_min_length" => {
            validate_no_block(directive)?;
            context.gzip_min_length = parse_size(one_arg(directive)?, &directive.location)?;
        }
        "gzip_comp_level" => {
            validate_no_block(directive)?;
            context.gzip_comp_level = one_arg(directive)?.parse::<u32>().map_err(|_| {
                ConfigError::at(directive.location.clone(), "invalid gzip compression level")
            })?;
            if !(1..=9).contains(&context.gzip_comp_level) {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid gzip compression level",
                ));
            }
        }
        "gzip_vary" => context.gzip_vary = parse_flag(directive, "gzip_vary")?,
        "gzip_proxied" => context.gzip_proxied = parse_gzip_proxied(directive)?,
        "gzip_static" => context.gzip_static = parse_gzip_static(directive)?,
        "gunzip" => context.gunzip = parse_flag(directive, "gunzip")?,
        "sendfile" => context.sendfile = parse_flag(directive, "sendfile")?,
        "log_not_found" => context.log_not_found = parse_flag(directive, "log_not_found")?,
        "proxy_connect_timeout" => {
            validate_no_block(directive)?;
            context.proxy_connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_read_timeout" => {
            validate_no_block(directive)?;
            context.proxy_read_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_send_timeout" => {
            validate_no_block(directive)?;
            context.proxy_send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "proxy_next_upstream" => {
            apply_next_upstream(directive, &mut context.proxy_next_upstream)?;
            return Ok(());
        }
        "proxy_next_upstream_tries" => {
            context.proxy_next_upstream.tries =
                parse_next_upstream_tries(directive, "proxy_next_upstream_tries")?;
            return Ok(());
        }
        "proxy_next_upstream_timeout" => {
            context.proxy_next_upstream.timeout =
                parse_next_upstream_timeout(directive, "proxy_next_upstream_timeout")?;
            return Ok(());
        }
        "ssl_certificate" => {
            validate_no_block(directive)?;
            context.ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_certificate_key" => {
            validate_no_block(directive)?;
            context.ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "auth_basic" => {
            validate_no_block(directive)?;
            let realm = one_arg(directive)?;
            context.auth_basic = (realm != "off").then(|| realm.to_string());
        }
        "auth_basic_user_file" => {
            validate_no_block(directive)?;
            context.auth_basic_user_file = Some(
                resolve_path(prefix, one_arg(directive)?)
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        "auth_request" => {
            validate_no_block(directive)?;
            let uri = one_arg(directive)?;
            context.auth_request = (uri != "off").then(|| uri.to_string());
        }
        "auth_request_set" => {
            validate_no_block(directive)?;
            if directive.args.len() != 2 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"auth_request_set\" directive",
                ));
            }
            let name =
                parse_variable_name(&directive.args[0], &directive.location, "auth_request_set")?;
            context.auth_request_sets.push(AuthRequestSet {
                name,
                value: directive.args[1].clone(),
            });
        }
        "satisfy" => {
            validate_no_block(directive)?;
            context.satisfy_any = match one_arg(directive)? {
                "any" => true,
                "all" => false,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid value \"{value}\" in \"satisfy\" directive"),
                    ));
                }
            };
        }
        "add_header" | "proxy_set_header" | "allow" | "deny" | "return" | "rewrite"
        | "try_files" | "proxy_pass" | "error_page" | "internal" => {
            if allow_http_only {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("\"{}\" directive is not allowed here", directive.name),
                ));
            }
        }
        "server" | "upstream" | "types" | "log_format" => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("\"{}\" directive is not allowed here", directive.name),
            ));
        }
        other => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!("unknown directive \"{other}\""),
            ));
        }
    }
    Ok(())
}
