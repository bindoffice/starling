use std::collections::HashSet;

use super::*;

const HTTP2_MAX_WINDOW: u64 = (1 << 31) - 1;
const HTTP2_MAX_FRAME_SIZE: u64 = (1 << 24) - 1;

#[derive(Default)]
pub(super) struct Http2Seen {
    names: HashSet<String>,
}

impl Http2Seen {
    fn mark(&mut self, name: &str, location: &SourceLocation) -> ConfigResult<()> {
        if self.names.insert(name.to_string()) {
            return Ok(());
        }
        Err(ConfigError::at(
            location.clone(),
            format!("\"{name}\" directive is duplicate"),
        ))
    }
}

pub(super) fn try_apply_http2_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    scope: Option<Scope>,
    seen: &mut SeenScopeDirectives,
) -> ConfigResult<bool> {
    let name = directive.name.as_str();
    if !name.starts_with("http2_") {
        return Ok(false);
    }

    let (main, server, location) = match name {
        "http2_recv_buffer_size" => (true, false, false),
        "http2_pool_size"
        | "http2_max_concurrent_streams"
        | "http2_body_preread_size"
        | "http2_streams_index_size"
        | "http2_max_concurrent_pushes"
        | "http2_max_requests"
        | "http2_max_field_size"
        | "http2_max_header_size"
        | "http2_recv_timeout"
        | "http2_idle_timeout" => (true, true, false),
        "http2_chunk_size" | "http2_push_preload" | "http2_push" => (true, true, true),
        _ => return Ok(false),
    };
    let allowed = match scope {
        None => main,
        Some(Scope::Server) => server,
        Some(Scope::Location) => location,
    };
    if !allowed {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{name}\" directive is not allowed here"),
        ));
    }
    validate_no_block(directive)?;
    if is_obsolete(name) {
        // Nginx parses only the argument count for obsolete directives. Values
        // are intentionally ignored, including malformed sizes, flags, and
        // numbers, and duplicates do not participate in the normal seen check.
        one_arg(directive)?;
        return Ok(true);
    }
    seen.http2.mark(name, &directive.location)?;

    match name {
        "http2_max_concurrent_streams" => {
            context.http2_limits.max_concurrent_streams = parse_u32(directive)?;
        }
        "http2_body_preread_size" => {
            context.http2_limits.body_preread_size =
                parse_bounded_size(directive, HTTP2_MAX_WINDOW)? as u32;
        }
        "http2_chunk_size" => {
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            if size == 0 {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "the http2 chunk size cannot be zero",
                ));
            }
            context.http2_limits.chunk_size = size.min(HTTP2_MAX_FRAME_SIZE) as usize;
        }
        "http2_pool_size" => {
            parse_size(one_arg(directive)?, &directive.location)?;
        }
        "http2_recv_buffer_size" => {
            parse_size(one_arg(directive)?, &directive.location)?;
        }
        "http2_streams_index_size" => {
            let value = parse_usize(one_arg(directive)?, &directive.location, name)?;
            if value == 0 || !value.is_power_of_two() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "must be a power of two",
                ));
            }
        }
        _ => unreachable!("recognized http2 directive"),
    }
    Ok(true)
}

fn is_obsolete(name: &str) -> bool {
    matches!(
        name,
        "http2_max_concurrent_pushes"
            | "http2_max_requests"
            | "http2_max_field_size"
            | "http2_max_header_size"
            | "http2_recv_timeout"
            | "http2_idle_timeout"
            | "http2_push_preload"
            | "http2_push"
    )
}

fn parse_u32(directive: &Directive) -> ConfigResult<u32> {
    let value = one_arg(directive)?;
    value.parse::<u32>().map_err(|_| {
        ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid value \"{value}\" in \"{}\" directive",
                directive.name
            ),
        )
    })
}

fn parse_bounded_size(directive: &Directive, maximum: u64) -> ConfigResult<u64> {
    let value = parse_size(one_arg(directive)?, &directive.location)?;
    if value > maximum {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "value \"{value}\" is too large in \"{}\" directive",
                directive.name
            ),
        ));
    }
    Ok(value)
}
