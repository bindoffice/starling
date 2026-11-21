use super::*;

pub(super) fn try_apply_cgi_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "fastcgi_param" => {
            validate_no_block(directive)?;
            if !seen.fastcgi_param {
                context.fastcgi_params.clear();
                seen.fastcgi_param = true;
            }
            context.fastcgi_params.push(parse_cgi_param(directive)?);
        }
        "fastcgi_catch_stderr" => {
            validate_no_block(directive)?;
            if !seen.fastcgi_catch_stderr {
                context.fastcgi_catch_stderr.clear();
                seen.fastcgi_catch_stderr = true;
            }
            context
                .fastcgi_catch_stderr
                .push(one_arg(directive)?.to_string());
        }
        "fastcgi_hide_header" => apply_header_list(
            directive,
            &mut context.fastcgi_hide_headers,
            &mut seen.fastcgi_hide_header,
        )?,
        "fastcgi_pass_header" => apply_header_list(
            directive,
            &mut context.fastcgi_pass_headers,
            &mut seen.fastcgi_pass_header,
        )?,
        "fastcgi_ignore_headers" => apply_ignore_headers(
            directive,
            &mut context.fastcgi_ignore_headers,
            &mut seen.fastcgi_ignore_headers,
        )?,
        "fastcgi_ignore_client_abort" => {
            if seen.fastcgi_ignore_client_abort {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"fastcgi_ignore_client_abort\" directive is duplicate",
                ));
            }
            context.fastcgi_ignore_client_abort =
                parse_flag(directive, "fastcgi_ignore_client_abort")?;
            seen.fastcgi_ignore_client_abort = true;
        }
        "fastcgi_force_ranges" => {
            context.fastcgi_force_ranges = parse_flag(directive, "fastcgi_force_ranges")?;
        }
        "fastcgi_limit_rate" => {
            context.fastcgi_limit_rate = Some(parse_complex_size(directive, "fastcgi_limit_rate")?);
        }
        "fastcgi_store" => {
            apply_store_value(
                directive,
                &mut context.fastcgi_store,
                &mut seen.fastcgi_store,
            )?;
        }
        "fastcgi_store_access" => {
            apply_store_access(
                directive,
                &mut context.fastcgi_store_access,
                &mut seen.fastcgi_store_access,
            )?;
        }
        "uwsgi_param" => {
            validate_no_block(directive)?;
            if !seen.uwsgi_param {
                context.uwsgi_params.clear();
                seen.uwsgi_param = true;
            }
            context.uwsgi_params.push(parse_cgi_param(directive)?);
        }
        "uwsgi_string" => {
            validate_no_block(directive)?;
            if seen.uwsgi_string {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"uwsgi_string\" directive is duplicate",
                ));
            }
            context.uwsgi_string = one_arg(directive)?.to_string();
            seen.uwsgi_string = true;
        }
        "uwsgi_hide_header" => apply_header_list(
            directive,
            &mut context.uwsgi_hide_headers,
            &mut seen.uwsgi_hide_header,
        )?,
        "uwsgi_pass_header" => apply_header_list(
            directive,
            &mut context.uwsgi_pass_headers,
            &mut seen.uwsgi_pass_header,
        )?,
        "uwsgi_ignore_headers" => apply_ignore_headers(
            directive,
            &mut context.uwsgi_ignore_headers,
            &mut seen.uwsgi_ignore_headers,
        )?,
        "uwsgi_ignore_client_abort" => {
            if seen.uwsgi_ignore_client_abort {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"uwsgi_ignore_client_abort\" directive is duplicate",
                ));
            }
            context.uwsgi_ignore_client_abort = parse_flag(directive, "uwsgi_ignore_client_abort")?;
            seen.uwsgi_ignore_client_abort = true;
        }
        "uwsgi_force_ranges" => {
            context.uwsgi_force_ranges = parse_flag(directive, "uwsgi_force_ranges")?;
        }
        "uwsgi_limit_rate" => {
            context.uwsgi_limit_rate = Some(parse_complex_size(directive, "uwsgi_limit_rate")?);
        }
        "uwsgi_store" => {
            apply_store_value(directive, &mut context.uwsgi_store, &mut seen.uwsgi_store)?;
        }
        "uwsgi_store_access" => {
            apply_store_access(
                directive,
                &mut context.uwsgi_store_access,
                &mut seen.uwsgi_store_access,
            )?;
        }
        "scgi_param" => {
            validate_no_block(directive)?;
            if !seen.scgi_param {
                context.scgi_params.clear();
                seen.scgi_param = true;
            }
            context.scgi_params.push(parse_cgi_param(directive)?);
        }
        "scgi_hide_header" => apply_header_list(
            directive,
            &mut context.scgi_hide_headers,
            &mut seen.scgi_hide_header,
        )?,
        "scgi_pass_header" => apply_header_list(
            directive,
            &mut context.scgi_pass_headers,
            &mut seen.scgi_pass_header,
        )?,
        "scgi_ignore_headers" => apply_ignore_headers(
            directive,
            &mut context.scgi_ignore_headers,
            &mut seen.scgi_ignore_headers,
        )?,
        "scgi_ignore_client_abort" => {
            if seen.scgi_ignore_client_abort {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"scgi_ignore_client_abort\" directive is duplicate",
                ));
            }
            context.scgi_ignore_client_abort = parse_flag(directive, "scgi_ignore_client_abort")?;
            seen.scgi_ignore_client_abort = true;
        }
        "scgi_force_ranges" => {
            context.scgi_force_ranges = parse_flag(directive, "scgi_force_ranges")?;
        }
        "scgi_limit_rate" => {
            context.scgi_limit_rate = Some(parse_complex_size(directive, "scgi_limit_rate")?);
        }
        "scgi_store" => {
            apply_store_value(directive, &mut context.scgi_store, &mut seen.scgi_store)?;
        }
        "scgi_store_access" => {
            apply_store_access(
                directive,
                &mut context.scgi_store_access,
                &mut seen.scgi_store_access,
            )?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
