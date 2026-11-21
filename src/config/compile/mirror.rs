use super::*;

#[derive(Default)]
pub(super) struct MirrorSeen {
    configured: bool,
    disabled: bool,
    request_body: bool,
}

pub(super) fn apply_mirror_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut MirrorSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    let uri = one_arg(directive)?;
    if seen.disabled || (uri == "off" && seen.configured) {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"mirror\" directive is duplicate",
        ));
    }
    if !seen.configured {
        context.mirrors.clear();
        seen.configured = true;
    }
    if uri == "off" {
        seen.disabled = true;
    } else {
        context.mirrors.push(uri.to_string());
    }
    Ok(())
}

pub(super) fn apply_mirror_request_body(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut MirrorSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if seen.request_body {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"mirror_request_body\" directive is duplicate",
        ));
    }
    seen.request_body = true;
    context.mirror_request_body = parse_flag(directive, "mirror_request_body")?;
    Ok(())
}
