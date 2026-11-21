use super::*;

pub(super) fn try_apply_uwsgi_tls_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "uwsgi_ssl_server_name" => {
            context.uwsgi_ssl_server_name = parse_flag(directive, "uwsgi_ssl_server_name")?;
        }
        "uwsgi_ssl_name" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_name = Some(one_arg(directive)?.to_string());
        }
        "uwsgi_ssl_session_reuse" => {
            context.uwsgi_ssl_session_reuse = parse_flag(directive, "uwsgi_ssl_session_reuse")?;
        }
        "uwsgi_ssl_verify" => {
            context.uwsgi_ssl_verify = parse_flag(directive, "uwsgi_ssl_verify")?;
        }
        "uwsgi_ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "uwsgi_ssl_verify_depth" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_verify_depth = parse_usize(
                one_arg(directive)?,
                &directive.location,
                "uwsgi_ssl_verify_depth",
            )? as u32;
        }
        "uwsgi_ssl_crl" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "uwsgi_ssl_certificate" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "uwsgi_ssl_certificate_key" => {
            validate_no_block(directive)?;
            context.uwsgi_ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "uwsgi_ssl_password_file" => {
            context.uwsgi_ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "uwsgi_ssl_protocols" => {
            apply_tls_protocols(
                directive,
                &mut context.uwsgi_ssl_protocols,
                &mut seen.uwsgi_ssl_protocols,
            )?;
        }
        "uwsgi_ssl_ciphers" => {
            context.uwsgi_ssl_ciphers = parse_tls_cipher_config(directive)?;
        }
        "uwsgi_ssl_conf_command" => {
            reject_ssl_conf_command(directive, "uwsgi_ssl_conf_command")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn reject_ssl_conf_command(directive: &Directive, name: &str) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!(
            "\"{name}\" is not supported by the rustls TLS backend; \
             use \"uwsgi_ssl_protocols\" or \"uwsgi_ssl_ciphers\" instead"
        ),
    ))
}
