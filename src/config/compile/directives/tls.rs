use super::super::compat::reject_ssl_conf_command;
use super::*;

pub(super) fn try_apply_tls_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    scope: Scope,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "ssl_password_file" => {
            require_server_scope(directive, scope)?;
            context.ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "ssl_protocols" => {
            require_server_scope(directive, scope)?;
            apply_tls_protocols(
                directive,
                &mut context.ssl_protocols,
                &mut seen.ssl_protocols,
            )?;
        }
        "ssl_prefer_server_ciphers" => {
            require_server_scope(directive, scope)?;
            context.ssl_prefer_server_ciphers = parse_flag(directive, "ssl_prefer_server_ciphers")?;
        }
        "ssl_ciphers" => {
            require_server_scope(directive, scope)?;
            context.ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "ssl_verify_client" => {
            require_server_scope(directive, scope)?;
            context.ssl_verify_client = parse_ssl_verify_client(directive)?;
        }
        "ssl_verify_depth" => {
            require_server_scope(directive, scope)?;
            context.ssl_verify_depth = parse_ssl_verify_depth(directive)?;
        }
        "ssl_client_certificate" => {
            require_server_scope(directive, scope)?;
            context.ssl_client_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_trusted_certificate" => {
            require_server_scope(directive, scope)?;
            context.ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "ssl_crl" => {
            require_server_scope(directive, scope)?;
            context.ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_ciphers" => {
            context.proxy_ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "proxy_ssl_protocols" => {
            apply_tls_protocols(
                directive,
                &mut context.proxy_ssl_protocols,
                &mut seen.proxy_ssl_protocols,
            )?;
        }
        "ssl_conf_command" => {
            require_server_scope(directive, scope)?;
            reject_ssl_conf_command(directive, "ssl_conf_command")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn require_server_scope(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    if matches!(scope, Scope::Server) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is not allowed here", directive.name),
    ))
}
