use super::*;

pub(in crate::config::compile) fn parse_stream_proxy_protocol(
    directive: &Directive,
) -> ConfigResult<StreamProxyProtocol> {
    validate_no_block(directive)?;
    match one_arg(directive)? {
        "off" => Ok(StreamProxyProtocol::Off),
        "on" => Ok(StreamProxyProtocol::V1),
        "v2" => Ok(StreamProxyProtocol::V2),
        value => Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\" in \"proxy_protocol\" directive"),
        )),
    }
}

pub(in crate::config::compile) fn parse_split_percent(value: &str) -> Option<u32> {
    let value = value.strip_suffix('%')?;
    let mut result = 0_u32;
    let mut point = 2_u32;
    let mut saw_dot = false;
    for byte in value.bytes() {
        if byte == b'.' {
            if saw_dot {
                return None;
            }
            saw_dot = true;
            continue;
        }
        if !byte.is_ascii_digit() || point == 0 {
            return None;
        }
        result = result
            .checked_mul(10)?
            .checked_add(u32::from(byte - b'0'))?;
        if saw_dot {
            point -= 1;
        }
    }
    while point > 0 {
        result = result.checked_mul(10)?;
        point -= 1;
    }
    (result > 0).then_some(result)
}

pub(in crate::config::compile) fn parse_variable_name(
    value: &str,
    location: &SourceLocation,
    directive: &str,
) -> ConfigResult<String> {
    let Some(name) = value.strip_prefix('$') else {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid variable name in \"{directive}\" directive"),
        ));
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid variable name in \"{directive}\" directive"),
        ));
    }
    Ok(name.to_string())
}
