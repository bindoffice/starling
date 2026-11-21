use super::*;

pub(super) fn try_apply_proxy_http_version(
    directive: &Directive,
    context: &mut EffectiveConfig,
) -> ConfigResult<bool> {
    if directive.name != "proxy_http_version" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    let version = one_arg(directive)?;
    if !matches!(version, "1.0" | "1.1" | "2") {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{version}\" in \"proxy_http_version\" directive"),
        ));
    }
    context.proxy_http_version = version.to_string();
    Ok(true)
}
