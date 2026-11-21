use super::*;

pub(in crate::server) fn should_send_early_hints(
    config: &crate::config::EarlyHintsConfig,
    variables: &Variables,
) -> bool {
    config.is_configured()
        && config.predicates.iter().any(|predicate| {
            let value = eval_variables(predicate, variables);
            !value.is_empty() && value != "0"
        })
}
