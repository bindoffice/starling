use super::*;

pub(super) fn compile_rewrite_script(
    directives: &[&Directive],
    context: &mut EffectiveConfig,
    prefix: &Path,
    scope: Scope,
) -> ConfigResult<()> {
    for directive in directives {
        let action = match directive.name.as_str() {
            "set" => {
                validate_no_block(directive)?;
                let set = parse_set(directive)?;
                context.sets.push(set.clone());
                RewriteAction::Set(set)
            }
            "rewrite" => {
                validate_no_block(directive)?;
                let rule = parse_rewrite(directive)?;
                context.rewrites.push(rule.clone());
                RewriteAction::Rewrite(rule)
            }
            "return" => {
                validate_no_block(directive)?;
                let rule = parse_return(directive)?;
                context.return_directive = Some(rule.clone());
                RewriteAction::Return(rule)
            }
            "break" => {
                validate_no_block(directive)?;
                if !directive.args.is_empty() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "invalid number of arguments in \"break\" directive",
                    ));
                }
                RewriteAction::Break
            }
            "if" => {
                require_block(directive)?;
                let conditional = compile_conditional(directive, context, prefix, scope)?;
                context.conditionals.push(conditional.clone());
                RewriteAction::If(Box::new(conditional))
            }
            other => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("unexpected rewrite directive \"{other}\""),
                ));
            }
        };
        context.rewrite_actions.push(action);
    }
    Ok(())
}
