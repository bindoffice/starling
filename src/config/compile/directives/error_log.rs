use super::*;

pub(in crate::config::compile) fn try_apply_error_log_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    seen: &mut bool,
) -> ConfigResult<bool> {
    if directive.name != "error_log" {
        return Ok(false);
    }
    if !*seen {
        context.error_logs.clear();
        *seen = true;
    }
    context
        .error_logs
        .push(parse_log_target(prefix, directive)?);
    Ok(true)
}
