use super::*;

pub(super) fn try_apply_rewrite_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "return" => {
            validate_no_block(directive)?;
            let rule = parse_return(directive)?;
            context.return_directive = Some(rule.clone());
            context.rewrite_actions.push(RewriteAction::Return(rule));
        }
        "set" => {
            validate_no_block(directive)?;
            let set = parse_set(directive)?;
            context.sets.push(set.clone());
            context.rewrite_actions.push(RewriteAction::Set(set));
        }
        "rewrite" => {
            validate_no_block(directive)?;
            let rule = parse_rewrite(directive)?;
            context.rewrites.push(rule.clone());
            context.rewrite_actions.push(RewriteAction::Rewrite(rule));
        }
        "break" => {
            validate_no_block(directive)?;
            if !directive.args.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"break\" directive",
                ));
            }
            context.rewrite_actions.push(RewriteAction::Break);
        }
        "try_files" => {
            validate_no_block(directive)?;
            if directive.args.len() < 2 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"try_files\" directive",
                ));
            }
            let mut args = directive.args.clone();
            let fallback = args.pop().expect("checked length");
            context.try_files = Some(TryFiles {
                candidates: args,
                fallback,
            });
        }
        "proxy_pass" => {
            validate_no_block(directive)?;
            context.proxy_pass = Some(parse_proxy_pass(directive)?);
            context.grpc.pass = None;
            context.fastcgi_pass = None;
            context.uwsgi_pass = None;
            context.scgi_pass = None;
            context.memcached_pass = None;
            context.tunnel.pass = None;
        }
        "tunnel_pass" => {
            if seen.tunnel_pass {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"tunnel_pass\" directive is duplicate",
                ));
            }
            seen.tunnel_pass = true;
            context.tunnel.pass = Some(super::tunnel::parse_tunnel_pass(directive)?);
            context.proxy_pass = None;
            context.grpc.pass = None;
            context.fastcgi_pass = None;
            context.uwsgi_pass = None;
            context.scgi_pass = None;
            context.memcached_pass = None;
        }
        "internal" => {
            validate_no_block(directive)?;
            context.internal = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
