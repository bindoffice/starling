use super::*;

pub(super) fn try_apply_server_tokens_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    if directive.name != "server_tokens" {
        return Ok(false);
    }
    validate_no_block(directive)?;
    if seen.server_tokens {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"server_tokens\" directive is duplicate",
        ));
    }
    context.server_tokens = match one_arg(directive)? {
        "off" => ServerTokens::Off,
        "on" => ServerTokens::On,
        "build" => ServerTokens::Build,
        value => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid value \"{value}\" in \"server_tokens\" directive, \
                     it must be \"off\", \"on\" or \"build\""
                ),
            ));
        }
    };
    seen.server_tokens = true;
    Ok(true)
}
