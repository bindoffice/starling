use super::*;

pub(super) fn try_apply_access_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    scope: Scope,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "open_log_file_cache" => {
            try_apply_open_log_file_cache_directive(
                directive,
                &mut context.open_log_file_cache,
                &mut seen.open_log_file_cache,
            )?;
        }
        "access_log" => {
            apply_access_log_directive(
                directive,
                prefix,
                Some("combined"),
                &mut context.access_log,
                &mut seen.access_log,
            )?;
        }
        "allow" | "deny" => {
            validate_no_block(directive)?;
            if !seen.access {
                context.access_rules.clear();
                seen.access = true;
            }
            context.access_rules.push(parse_access_rule(directive)?);
        }
        "limit_req" => {
            validate_no_block(directive)?;
            if !seen.limit_req {
                context.limit_reqs.clear();
                seen.limit_req = true;
            }
            let zones = context.limit_req_zones.clone();
            let limit = compile_limit_req(directive, &zones)?;
            if context
                .limit_reqs
                .iter()
                .any(|existing| existing.zone == limit.zone)
            {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"limit_req\" directive is duplicate",
                ));
            }
            context.limit_reqs.push(limit);
        }
        "limit_conn" => {
            validate_no_block(directive)?;
            if !seen.limit_conn {
                context.limit_conns.clear();
                seen.limit_conn = true;
            }
            let zones = context.limit_conn_zones.clone();
            let limit = compile_limit_conn(directive, &zones)?;
            if context
                .limit_conns
                .iter()
                .any(|existing| existing.zone == limit.zone)
            {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"limit_conn\" directive is duplicate",
                ));
            }
            context.limit_conns.push(limit);
        }
        "limit_req_status" => {
            context.limit_req_status = parse_limit_status(directive, "limit_req_status")?;
        }
        "limit_conn_status" => {
            context.limit_conn_status = parse_limit_status(directive, "limit_conn_status")?;
        }
        "limit_req_dry_run" => {
            context.limit_req_dry_run = parse_flag(directive, "limit_req_dry_run")?;
        }
        "limit_conn_dry_run" => {
            context.limit_conn_dry_run = parse_flag(directive, "limit_conn_dry_run")?;
        }
        "limit_req_log_level" => {
            context.limit_req_log_level = parse_limit_log_level(directive, &directive.name)?;
        }
        "limit_conn_log_level" => {
            context.limit_conn_log_level = parse_limit_log_level(directive, &directive.name)?;
        }
        "stub_status" => {
            validate_no_block(directive)?;
            if !directive.args.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"stub_status\" directive",
                ));
            }
            require_location_scope(directive, scope)?;
            context.stub_status = true;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn require_location_scope(directive: &Directive, scope: Scope) -> ConfigResult<()> {
    if matches!(scope, Scope::Location) {
        return Ok(());
    }
    Err(ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is not allowed here", directive.name),
    ))
}
