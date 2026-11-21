use super::*;

use super::super::compat::reject_proxy_ssl_conf_command;

pub(super) fn try_apply_proxy_tls_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "proxy_ssl_server_name" => {
            context.proxy_ssl_server_name = parse_flag(directive, "proxy_ssl_server_name")?;
        }
        "proxy_ssl_name" => {
            validate_no_block(directive)?;
            context.proxy_ssl_name = Some(one_arg(directive)?.to_string());
        }
        "proxy_ssl_session_reuse" => {
            context.proxy_ssl_session_reuse = parse_flag(directive, "proxy_ssl_session_reuse")?;
        }
        "proxy_ssl_verify" => {
            context.proxy_ssl_verify = parse_flag(directive, "proxy_ssl_verify")?;
        }
        "proxy_ssl_trusted_certificate" => {
            validate_no_block(directive)?;
            context.proxy_ssl_trusted_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_verify_depth" => {
            validate_no_block(directive)?;
            context.proxy_ssl_verify_depth = parse_usize(
                one_arg(directive)?,
                &directive.location,
                "proxy_ssl_verify_depth",
            )? as u32;
        }
        "proxy_ssl_crl" => {
            validate_no_block(directive)?;
            context.proxy_ssl_crl = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_certificate" => {
            validate_no_block(directive)?;
            context.proxy_ssl_certificate = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_certificate_key" => {
            validate_no_block(directive)?;
            context.proxy_ssl_certificate_key = Some(resolve_path(prefix, one_arg(directive)?));
        }
        "proxy_ssl_password_file" => {
            context.proxy_ssl_passwords = parse_password_file(prefix, directive)?;
        }
        "proxy_ssl_conf_command" => {
            reject_proxy_ssl_conf_command(directive)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
