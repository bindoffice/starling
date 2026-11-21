use super::*;

pub(super) fn compile_stream_pass(
    directive: &Directive,
    prefix: &Path,
) -> ConfigResult<StreamPass> {
    validate_no_block(directive)?;
    let target = one_arg(directive)?.to_string();
    let address = if target.contains('$') {
        None
    } else {
        Some(
            parse_stream_pass_target(&target, prefix)
                .map_err(|reason| pass_error(&directive.location, &target, &reason))?,
        )
    };
    Ok(StreamPass { target, address })
}

fn pass_error(location: &SourceLocation, target: &str, reason: &str) -> ConfigError {
    ConfigError::at(
        location.clone(),
        format!("{reason} in \"{target}\" of the \"pass\" directive"),
    )
}
