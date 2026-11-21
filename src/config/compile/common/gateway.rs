use super::*;

pub(super) fn try_apply_gateway_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "fastcgi_pass" => {
            validate_no_block(directive)?;
            context.fastcgi_pass = Some(parse_fastcgi_pass(directive)?);
            context.tunnel.pass = None;
        }
        "fastcgi_param" => {
            validate_no_block(directive)?;
            context.fastcgi_params.push(parse_cgi_param(directive)?);
        }
        "fastcgi_index" => {
            validate_no_block(directive)?;
            context.fastcgi_index = one_arg(directive)?.to_string();
        }
        "fastcgi_split_path_info" => {
            validate_no_block(directive)?;
            context.fastcgi_split_path_info = Some(parse_fastcgi_split_path_info(directive)?);
        }
        "fastcgi_pass_request_headers" => {
            context.fastcgi_pass_request_headers =
                parse_flag(directive, "fastcgi_pass_request_headers")?;
        }
        "fastcgi_pass_request_body" => {
            context.fastcgi_pass_request_body = parse_flag(directive, "fastcgi_pass_request_body")?;
        }
        "fastcgi_intercept_errors" => {
            context.fastcgi_intercept_errors = parse_flag(directive, "fastcgi_intercept_errors")?;
        }
        "fastcgi_keep_conn" => {
            context.fastcgi_keep_conn = parse_flag(directive, "fastcgi_keep_conn")?;
        }
        "fastcgi_catch_stderr" => {
            validate_no_block(directive)?;
            context
                .fastcgi_catch_stderr
                .push(one_arg(directive)?.to_string());
        }
        "fastcgi_connect_timeout" => {
            validate_no_block(directive)?;
            context.fastcgi_connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "fastcgi_send_timeout" => {
            validate_no_block(directive)?;
            context.fastcgi_send_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "fastcgi_read_timeout" => {
            validate_no_block(directive)?;
            context.fastcgi_read_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "fastcgi_next_upstream" => {
            apply_next_upstream(directive, &mut context.fastcgi_next_upstream)?;
        }
        "fastcgi_next_upstream_tries" => {
            context.fastcgi_next_upstream.tries =
                parse_next_upstream_tries(directive, "fastcgi_next_upstream_tries")?;
        }
        "fastcgi_next_upstream_timeout" => {
            context.fastcgi_next_upstream.timeout =
                parse_next_upstream_timeout(directive, "fastcgi_next_upstream_timeout")?;
        }
        "uwsgi_pass" => {
            validate_no_block(directive)?;
            context.uwsgi_pass = Some(parse_uwsgi_pass(directive)?);
            context.tunnel.pass = None;
        }
        "uwsgi_param" => {
            validate_no_block(directive)?;
            context.uwsgi_params.push(parse_cgi_param(directive)?);
        }
        "uwsgi_modifier1" => {
            validate_no_block(directive)?;
            context.uwsgi_modifier1 =
                parse_modifier(one_arg(directive)?, "uwsgi_modifier1", &directive.location)?;
        }
        "uwsgi_modifier2" => {
            validate_no_block(directive)?;
            context.uwsgi_modifier2 =
                parse_modifier(one_arg(directive)?, "uwsgi_modifier2", &directive.location)?;
        }
        "uwsgi_pass_request_headers" => {
            context.uwsgi_pass_request_headers =
                parse_flag(directive, "uwsgi_pass_request_headers")?;
        }
        "uwsgi_pass_request_body" => {
            context.uwsgi_pass_request_body = parse_flag(directive, "uwsgi_pass_request_body")?;
        }
        "uwsgi_intercept_errors" => {
            context.uwsgi_intercept_errors = parse_flag(directive, "uwsgi_intercept_errors")?;
        }
        "uwsgi_connect_timeout" => {
            validate_no_block(directive)?;
            context.uwsgi_connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "uwsgi_send_timeout" => {
            validate_no_block(directive)?;
            context.uwsgi_send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "uwsgi_read_timeout" => {
            validate_no_block(directive)?;
            context.uwsgi_read_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "uwsgi_next_upstream" => {
            apply_next_upstream(directive, &mut context.uwsgi_next_upstream)?;
        }
        "uwsgi_next_upstream_tries" => {
            context.uwsgi_next_upstream.tries =
                parse_next_upstream_tries(directive, "uwsgi_next_upstream_tries")?;
        }
        "uwsgi_next_upstream_timeout" => {
            context.uwsgi_next_upstream.timeout =
                parse_next_upstream_timeout(directive, "uwsgi_next_upstream_timeout")?;
        }
        "scgi_pass" => {
            validate_no_block(directive)?;
            context.scgi_pass = Some(parse_scgi_pass(directive)?);
            context.tunnel.pass = None;
        }
        "scgi_param" => {
            validate_no_block(directive)?;
            context.scgi_params.push(parse_cgi_param(directive)?);
        }
        "scgi_pass_request_headers" => {
            context.scgi_pass_request_headers = parse_flag(directive, "scgi_pass_request_headers")?;
        }
        "scgi_pass_request_body" => {
            context.scgi_pass_request_body = parse_flag(directive, "scgi_pass_request_body")?;
        }
        "scgi_intercept_errors" => {
            context.scgi_intercept_errors = parse_flag(directive, "scgi_intercept_errors")?;
        }
        "scgi_connect_timeout" => {
            validate_no_block(directive)?;
            context.scgi_connect_timeout =
                parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "scgi_send_timeout" => {
            validate_no_block(directive)?;
            context.scgi_send_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "scgi_read_timeout" => {
            validate_no_block(directive)?;
            context.scgi_read_timeout = parse_duration(one_arg(directive)?, &directive.location)?;
        }
        "scgi_next_upstream" => {
            apply_next_upstream(directive, &mut context.scgi_next_upstream)?;
        }
        "scgi_next_upstream_tries" => {
            context.scgi_next_upstream.tries =
                parse_next_upstream_tries(directive, "scgi_next_upstream_tries")?;
        }
        "scgi_next_upstream_timeout" => {
            context.scgi_next_upstream.timeout =
                parse_next_upstream_timeout(directive, "scgi_next_upstream_timeout")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
