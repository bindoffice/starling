use super::*;

#[derive(Debug, Clone, Copy)]
enum UpstreamKind {
    Proxy,
    FastCgi,
    Uwsgi,
    Scgi,
}

impl UpstreamKind {
    const ALL: [Self; 4] = [Self::Proxy, Self::FastCgi, Self::Uwsgi, Self::Scgi];

    fn prefix(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::FastCgi => "fastcgi",
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }
}

#[derive(Default)]
pub(super) struct UpstreamBufferingSeen {
    buffering: bool,
    request_buffering: bool,
    buffers: bool,
    buffer_size: bool,
    busy_buffers_size: bool,
    temp_path: bool,
    max_temp_file_size: bool,
    temp_file_write_size: bool,
}

#[derive(Default)]
pub(super) struct BufferingSeen {
    proxy: UpstreamBufferingSeen,
    fastcgi: UpstreamBufferingSeen,
    uwsgi: UpstreamBufferingSeen,
    scgi: UpstreamBufferingSeen,
}

pub(super) fn try_apply_upstream_buffering_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    seen: &mut BufferingSeen,
) -> ConfigResult<bool> {
    let Some((kind, suffix)) = classify(directive.name.as_str()) else {
        return Ok(false);
    };
    validate_no_block(directive)?;
    if suffix == "_request_buffering" {
        if matches!(kind, UpstreamKind::Proxy) {
            return Ok(false);
        }
        let seen = match kind {
            UpstreamKind::FastCgi => &mut seen.fastcgi,
            UpstreamKind::Uwsgi => &mut seen.uwsgi,
            UpstreamKind::Scgi => &mut seen.scgi,
            UpstreamKind::Proxy => unreachable!("proxy request buffering has its own parser"),
        };
        mark_duplicate(&mut seen.request_buffering, directive)?;
        let enabled = parse_flag(directive, &directive.name)?;
        match kind {
            UpstreamKind::FastCgi => context.fastcgi_request_buffering = enabled,
            UpstreamKind::Uwsgi => context.uwsgi_request_buffering = enabled,
            UpstreamKind::Scgi => context.scgi_request_buffering = enabled,
            UpstreamKind::Proxy => unreachable!("proxy request buffering has its own parser"),
        }
        return Ok(true);
    }
    let (config, seen) = match kind {
        UpstreamKind::Proxy => (&mut context.proxy_buffering, &mut seen.proxy),
        UpstreamKind::FastCgi => (&mut context.fastcgi_buffering, &mut seen.fastcgi),
        UpstreamKind::Uwsgi => (&mut context.uwsgi_buffering, &mut seen.uwsgi),
        UpstreamKind::Scgi => (&mut context.scgi_buffering, &mut seen.scgi),
    };

    match suffix {
        "_buffering" => {
            mark_duplicate(&mut seen.buffering, directive)?;
            config.enabled = parse_flag(directive, &directive.name)?;
        }
        "_buffer_size" => {
            mark_duplicate(&mut seen.buffer_size, directive)?;
            config.buffer_size = parse_nonzero_size(directive)?;
        }
        "_buffers" => {
            mark_duplicate(&mut seen.buffers, directive)?;
            if directive.args.len() != 2 {
                return Err(invalid_arguments(directive));
            }
            let count = directive.args[0]
                .parse::<usize>()
                .ok()
                .filter(|count| *count > 0)
                .ok_or_else(|| invalid_value(directive, &directive.args[0]))?;
            let size = super::parse_size(&directive.args[1], &directive.location)
                .ok()
                .and_then(|size| usize::try_from(size).ok())
                .filter(|size| *size > 0)
                .ok_or_else(|| invalid_value(directive, &directive.args[1]))?;
            config.buffers = UpstreamBuffers { count, size };
        }
        "_busy_buffers_size" => {
            mark_duplicate(&mut seen.busy_buffers_size, directive)?;
            config.busy_buffers_size = Some(parse_size_value(directive)?);
        }
        "_temp_path" => {
            mark_duplicate(&mut seen.temp_path, directive)?;
            config.temp_path = parse_temp_path(directive, prefix)?;
        }
        "_max_temp_file_size" => {
            mark_duplicate(&mut seen.max_temp_file_size, directive)?;
            config.max_temp_file_size = Some(parse_directive_size(directive)?);
        }
        "_temp_file_write_size" => {
            mark_duplicate(&mut seen.temp_file_write_size, directive)?;
            config.temp_file_write_size = Some(parse_size_value(directive)?);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(in crate::config::compile) fn validate_upstream_buffering(
    context: &EffectiveConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    for kind in UpstreamKind::ALL {
        let config = match kind {
            UpstreamKind::Proxy => &context.proxy_buffering,
            UpstreamKind::FastCgi => &context.fastcgi_buffering,
            UpstreamKind::Uwsgi => &context.uwsgi_buffering,
            UpstreamKind::Scgi => &context.scgi_buffering,
        };
        validate_buffering(kind, config, location)?;
    }
    Ok(())
}

fn classify(name: &str) -> Option<(UpstreamKind, &str)> {
    UpstreamKind::ALL.into_iter().find_map(|kind| {
        name.strip_prefix(kind.prefix())
            .filter(|suffix| suffix.starts_with('_'))
            .map(|suffix| (kind, suffix))
    })
}

fn validate_buffering(
    kind: UpstreamKind,
    config: &UpstreamBufferingConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    let prefix = kind.prefix();
    if config.buffers.count < 2 {
        return Err(ConfigError::at(
            location.clone(),
            format!("there must be at least 2 \"{prefix}_buffers\""),
        ));
    }
    let size = config.max_buffer_size();
    let busy = config.resolved_busy_buffers_size();
    if busy < size {
        return Err(ConfigError::at(
            location.clone(),
            format!(
                "\"{prefix}_busy_buffers_size\" must be equal to or greater than the maximum of the value of \"{prefix}_buffer_size\" and one of the \"{prefix}_buffers\""
            ),
        ));
    }
    let available = config
        .buffers
        .count
        .saturating_sub(1)
        .saturating_mul(config.buffers.size);
    if busy > available {
        return Err(ConfigError::at(
            location.clone(),
            format!(
                "\"{prefix}_busy_buffers_size\" must be less than the size of all \"{prefix}_buffers\" minus one buffer"
            ),
        ));
    }
    if config.resolved_temp_file_write_size() < size {
        return Err(ConfigError::at(
            location.clone(),
            format!(
                "\"{prefix}_temp_file_write_size\" must be equal to or greater than the maximum of the value of \"{prefix}_buffer_size\" and one of the \"{prefix}_buffers\""
            ),
        ));
    }
    let max_temp = config.resolved_max_temp_file_size();
    if max_temp != 0 && max_temp < size as u64 {
        return Err(ConfigError::at(
            location.clone(),
            format!(
                "\"{prefix}_max_temp_file_size\" must be equal to zero to disable temporary files usage or must be equal to or greater than the maximum of the value of \"{prefix}_buffer_size\" and one of the \"{prefix}_buffers\""
            ),
        ));
    }
    Ok(())
}

fn parse_size_value(directive: &Directive) -> ConfigResult<usize> {
    let value = one_arg(directive)?;
    let size = parse_size(value, &directive.location)?;
    usize::try_from(size).map_err(|_| invalid_value(directive, value))
}

fn parse_directive_size(directive: &Directive) -> ConfigResult<u64> {
    let value = one_arg(directive)?;
    super::parse_size(value, &directive.location)
}

fn parse_nonzero_size(directive: &Directive) -> ConfigResult<usize> {
    let value = one_arg(directive)?;
    let size = parse_size_value(directive)?;
    if size == 0 {
        return Err(invalid_value(directive, value));
    }
    Ok(size)
}

fn mark_duplicate(seen: &mut bool, directive: &Directive) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

fn invalid_value(directive: &Directive, value: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid value \"{value}\" in \"{}\" directive",
            directive.name
        ),
    )
}

fn invalid_arguments(directive: &Directive) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid number of arguments in \"{}\" directive",
            directive.name
        ),
    )
}
