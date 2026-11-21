use super::*;

pub(super) fn try_apply_proxy_hash_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    let (target, configured) = match directive.name.as_str() {
        "proxy_headers_hash_max_size" => (
            &mut context.proxy_headers_hash_max_size,
            &mut seen.proxy_headers_hash_max_size,
        ),
        "proxy_headers_hash_bucket_size" => (
            &mut context.proxy_headers_hash_bucket_size,
            &mut seen.proxy_headers_hash_bucket_size,
        ),
        _ => return Ok(false),
    };
    validate_no_block(directive)?;
    if *configured {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    let value = one_arg(directive)?;
    *target = value.parse::<usize>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number \"{value}\" in \"{}\" directive",
                directive.name
            ),
        )
    })?;
    *configured = true;
    Ok(true)
}
