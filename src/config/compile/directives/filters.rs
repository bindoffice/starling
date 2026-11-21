use super::*;

pub(super) fn try_apply_filter_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "add_before_body" => {
            apply_addition_part(directive, context, &mut seen.addition, AdditionPart::Before)?;
        }
        "add_after_body" => {
            apply_addition_part(directive, context, &mut seen.addition, AdditionPart::After)?;
        }
        "addition_types" => {
            apply_addition_types(directive, context, &mut seen.addition)?;
        }
        "valid_referers" => {
            if !seen.valid_referers {
                context.valid_referers = ValidReferers::default();
                seen.valid_referers = true;
            }
            apply_valid_referers(directive, &mut context.valid_referers)?;
        }
        "mirror" => {
            apply_mirror_directive(directive, context, &mut seen.mirror)?;
        }
        "mirror_request_body" => {
            apply_mirror_request_body(directive, context, &mut seen.mirror)?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
