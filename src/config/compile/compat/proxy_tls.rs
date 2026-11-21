use super::super::*;

pub(in crate::config::compile) fn reject_proxy_ssl_conf_command(
    directive: &Directive,
) -> ConfigResult<()> {
    reject_ssl_conf_command(directive, "proxy_ssl_conf_command")
}

pub(in crate::config::compile) fn reject_ssl_conf_command(
    directive: &Directive,
    name: &str,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid number of arguments in \"{name}\" directive"),
        ));
    }
    let prefix = name.strip_suffix("ssl_conf_command").unwrap_or(name);
    Err(ConfigError::at(
        directive.location.clone(),
        format!(
            "\"{name}\" is not supported by the rustls TLS backend; \
             use \"{prefix}ssl_protocols\" or \"{prefix}ssl_ciphers\" instead"
        ),
    ))
}
