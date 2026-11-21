use super::*;

mod condition;
mod rule;
mod script;

pub(in crate::server) use condition::condition_matches;
pub(in crate::server) use rule::{append_args, apply_return, rewrite_destination};
#[cfg(test)]
pub(in crate::server) use script::apply_conditional;
pub(in crate::server) use script::apply_rewrite_script;

pub(in crate::server) enum RewriteOutcome {
    None,
    Break,
    Last,
    Response(Box<Response>),
}

#[cfg(test)]
#[allow(dead_code)]
pub(in crate::server) enum ConditionalOutcome {
    None,
    Continue,
    Restart,
    Response(Box<Response>),
}

#[cfg(test)]
pub(in crate::server) fn apply_conditionals(
    conditionals: &[ConditionalConfig],
    env: &mut RequestEnv,
    context: &mut EffectiveConfig,
    variables: &mut Variables,
    request: &Request,
) -> ConditionalOutcome {
    for conditional in conditionals {
        match apply_conditional(conditional, env, context, variables, request, None) {
            RewriteOutcome::None => {}
            RewriteOutcome::Break => return ConditionalOutcome::Continue,
            RewriteOutcome::Last => return ConditionalOutcome::Restart,
            RewriteOutcome::Response(response) => {
                return ConditionalOutcome::Response(response);
            }
        }
    }
    ConditionalOutcome::None
}
