use super::*;

#[derive(Default)]
pub(super) struct SsiSeen {
    enabled: bool,
    silent_errors: bool,
    ignore_recycled_buffers: bool,
    last_modified: bool,
    types: bool,
    min_file_chunk: bool,
    value_length: bool,
}

pub(super) fn is_ssi_directive(name: &str) -> bool {
    matches!(
        name,
        "ssi"
            | "ssi_silent_errors"
            | "ssi_ignore_recycled_buffers"
            | "ssi_last_modified"
            | "ssi_types"
            | "ssi_min_file_chunk"
            | "ssi_value_length"
    )
}

pub(super) fn apply_ssi_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SsiSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    match directive.name.as_str() {
        "ssi" => {
            reject_duplicate(directive, &mut seen.enabled)?;
            context.ssi.enabled = parse_flag(directive, "ssi")?;
        }
        "ssi_silent_errors" => {
            reject_duplicate(directive, &mut seen.silent_errors)?;
            context.ssi.silent_errors = parse_flag(directive, "ssi_silent_errors")?;
        }
        "ssi_ignore_recycled_buffers" => {
            reject_duplicate(directive, &mut seen.ignore_recycled_buffers)?;
            context.ssi.ignore_recycled_buffers =
                parse_flag(directive, "ssi_ignore_recycled_buffers")?;
        }
        "ssi_last_modified" => {
            reject_duplicate(directive, &mut seen.last_modified)?;
            context.ssi.last_modified = parse_flag(directive, "ssi_last_modified")?;
        }
        "ssi_types" => {
            reject_duplicate(directive, &mut seen.types)?;
            if directive.args.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"ssi_types\" directive",
                ));
            }
            context.ssi.types = directive
                .args
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect();
        }
        "ssi_min_file_chunk" => {
            reject_duplicate(directive, &mut seen.min_file_chunk)?;
            context.ssi.min_file_chunk = parse_size(one_arg(directive)?, &directive.location)?;
        }
        "ssi_value_length" => {
            reject_duplicate(directive, &mut seen.value_length)?;
            context.ssi.value_length = parse_size(one_arg(directive)?, &directive.location)?;
            if context.ssi.value_length == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"ssi_value_length\" directive",
                ));
            }
        }
        _ => unreachable!("checked by is_ssi_directive"),
    }
    Ok(())
}

fn reject_duplicate(directive: &Directive, configured: &mut bool) -> ConfigResult<()> {
    if *configured {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *configured = true;
    Ok(())
}
