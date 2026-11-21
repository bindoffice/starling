use super::*;

#[derive(Default)]
pub(in crate::config::compile) struct CharsetSeen {
    charset: bool,
    source_charset: bool,
    override_charset: bool,
    types: bool,
}

pub(in crate::config::compile) fn try_apply_charset_directive(
    directive: &Directive,
    context: &mut CharsetConfig,
    seen: &mut CharsetSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "charset" => {
            ensure_single(directive, &mut seen.charset)?;
            let charset = one_arg(directive)?;
            context.charset = (charset != "off").then(|| charset.to_string());
        }
        "source_charset" => {
            ensure_single(directive, &mut seen.source_charset)?;
            context.source_charset = Some(one_arg(directive)?.to_string());
        }
        "override_charset" => {
            ensure_single(directive, &mut seen.override_charset)?;
            context.override_charset = parse_flag(directive, "override_charset")?;
        }
        "charset_types" => {
            ensure_single(directive, &mut seen.types)?;
            validate_no_block(directive)?;
            if directive.args.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"charset_types\" directive",
                ));
            }
            context.types = directive
                .args
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect();
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn ensure_single(directive: &Directive, seen: &mut bool) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

pub(in crate::config::compile) fn compile_charset_map(
    directive: &Directive,
) -> ConfigResult<CharsetMap> {
    require_block(directive)?;
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"charset_map\" directive",
        ));
    }
    let source = directive.args[0].clone();
    let destination = directive.args[1].clone();
    if source.eq_ignore_ascii_case(&destination) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"charset_map\" between the same charsets \"{source}\" and \"{destination}\""),
        ));
    }
    if source.eq_ignore_ascii_case("utf-8") {
        return Err(ConfigError::at(
            directive.location.clone(),
            "\"charset_map\" with \"utf-8\" charset should be given in the second column",
        ));
    }

    let utf8_destination = destination.eq_ignore_ascii_case("utf-8");
    let mut map = if utf8_destination {
        CharsetMap::utf8_destination(source, destination)
    } else {
        CharsetMap::single_byte(source, destination)
    };
    for child in &directive.children {
        if child.has_block || child.args.len() != 1 {
            return Err(ConfigError::at(
                child.location.clone(),
                "invalid parameters number",
            ));
        }
        let source = parse_hex_byte(&child.name, &child.location)?;
        if utf8_destination {
            let (encoded, codepoint) = parse_utf8_value(&child.args[0], &child.location)?;
            map.set_utf8(source, &encoded, codepoint)
                .map_err(|message| ConfigError::at(child.location.clone(), message))?;
        } else {
            let destination = parse_hex_byte(&child.args[0], &child.location)?;
            map.set_single_byte(source, destination)
                .map_err(|message| ConfigError::at(child.location.clone(), message))?;
        }
    }
    Ok(map)
}

fn parse_hex_byte(value: &str, location: &SourceLocation) -> ConfigResult<u8> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\""),
        ));
    }
    let parsed = u32::from_str_radix(value, 16)
        .map_err(|_| ConfigError::at(location.clone(), format!("invalid value \"{value}\"")))?;
    u8::try_from(parsed)
        .map_err(|_| ConfigError::at(location.clone(), format!("invalid value \"{value}\"")))
}

fn parse_utf8_value(value: &str, location: &SourceLocation) -> ConfigResult<(Vec<u8>, u32)> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || value.len() > 6
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\""),
        ));
    }
    let encoded = (0..value.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&value[offset..offset + 2], 16).unwrap())
        .collect::<Vec<_>>();
    let decoded = std::str::from_utf8(&encoded)
        .map_err(|_| ConfigError::at(location.clone(), format!("invalid value \"{value}\"")))?;
    let mut characters = decoded.chars();
    let character = characters
        .next()
        .ok_or_else(|| ConfigError::at(location.clone(), format!("invalid value \"{value}\"")))?;
    let codepoint = character as u32;
    if characters.next().is_some() || codepoint > 0xffff {
        return Err(ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\""),
        ));
    }
    Ok((encoded, codepoint))
}

pub(in crate::config::compile) fn validate_charset_map(
    context: &EffectiveConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    validate_charset_values(&context.charset, &context.charset_maps, location)
}

pub(in crate::config::compile) fn validate_charset_values(
    charset: &CharsetConfig,
    maps: &[CharsetMap],
    location: &SourceLocation,
) -> ConfigResult<()> {
    let Some(source) = charset.source_charset.as_deref() else {
        return Ok(());
    };
    let Some(destination) = charset.charset.as_deref() else {
        return Ok(());
    };
    if source.starts_with('$')
        || destination.starts_with('$')
        || source.eq_ignore_ascii_case(destination)
    {
        return Ok(());
    }
    if maps.iter().any(|map| map.connects(source, destination)) {
        return Ok(());
    }
    Err(ConfigError::at(
        location.clone(),
        format!("no \"charset_map\" between the charsets \"{source}\" and \"{destination}\""),
    ))
}
