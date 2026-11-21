use super::*;

#[derive(Default)]
pub(super) struct RequestBodySeen {
    buffer_size: bool,
    temp_path: bool,
    in_file_only: bool,
    in_single_buffer: bool,
}

pub(super) fn try_apply_request_body_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    seen: &mut RequestBodySeen,
) -> ConfigResult<bool> {
    let name = directive.name.as_str();
    if !matches!(
        name,
        "client_body_buffer_size"
            | "client_body_temp_path"
            | "client_body_in_file_only"
            | "client_body_in_single_buffer"
    ) {
        return Ok(false);
    }
    validate_no_block(directive)?;

    match name {
        "client_body_buffer_size" => {
            mark_duplicate(&mut seen.buffer_size, directive)?;
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            let size = usize::try_from(size)
                .ok()
                .filter(|size| *size > 0)
                .ok_or_else(|| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "invalid value \"{}\" in \"{name}\" directive",
                            directive.args[0]
                        ),
                    )
                })?;
            Arc::make_mut(&mut context.client_body).buffer_size = size;
        }
        "client_body_temp_path" => {
            mark_duplicate(&mut seen.temp_path, directive)?;
            Arc::make_mut(&mut context.client_body).temp_path = parse_temp_path(directive, prefix)?;
        }
        "client_body_in_file_only" => {
            mark_duplicate(&mut seen.in_file_only, directive)?;
            Arc::make_mut(&mut context.client_body).in_file_only =
                parse_file_only(one_arg(directive)?, directive)?;
        }
        "client_body_in_single_buffer" => {
            mark_duplicate(&mut seen.in_single_buffer, directive)?;
            Arc::make_mut(&mut context.client_body).in_single_buffer = parse_flag(directive, name)?;
        }
        _ => unreachable!("recognized request body directive"),
    }
    Ok(true)
}

fn parse_file_only(value: &str, directive: &Directive) -> ConfigResult<ClientBodyInFileOnly> {
    match value {
        "off" => Ok(ClientBodyInFileOnly::Off),
        "on" => Ok(ClientBodyInFileOnly::On),
        "clean" => Ok(ClientBodyInFileOnly::Clean),
        _ => Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"client_body_in_file_only\" directive"),
        )),
    }
}

fn mark_duplicate(seen: &mut bool, directive: &Directive) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}
