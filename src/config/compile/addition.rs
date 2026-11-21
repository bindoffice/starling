use super::*;

#[derive(Default)]
pub(super) struct AdditionSeen {
    before: bool,
    after: bool,
    types: bool,
}

#[derive(Clone, Copy)]
pub(super) enum AdditionPart {
    Before,
    After,
}

pub(super) fn apply_addition_part(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut AdditionSeen,
    part: AdditionPart,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    let value = one_arg(directive)?.to_string();
    let (configured, target) = match part {
        AdditionPart::Before => (&mut seen.before, &mut context.addition_before),
        AdditionPart::After => (&mut seen.after, &mut context.addition_after),
    };
    if *configured {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *configured = true;
    *target = Some(value);
    Ok(())
}

pub(super) fn apply_addition_types(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut AdditionSeen,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if directive.args.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"addition_types\" directive",
        ));
    }
    if seen.types {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"addition_types\" directive is duplicate",
        ));
    }
    seen.types = true;
    context.addition_types = directive
        .args
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect();
    Ok(())
}
