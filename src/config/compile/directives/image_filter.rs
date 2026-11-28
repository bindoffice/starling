use super::*;

#[derive(Default)]
pub(super) struct ImageFilterSeen {
    filter: bool,
    buffer_size: bool,
    jpeg_quality: bool,
    webp_quality: bool,
    sharpen: bool,
    transparency: bool,
    interlace: bool,
}

pub(super) fn try_apply_image_filter_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    scope: Option<Scope>,
    seen: &mut ImageFilterSeen,
) -> ConfigResult<bool> {
    if !directive.name.starts_with("image_filter") {
        return Ok(false);
    }
    validate_no_block(directive)?;

    match directive.name.as_str() {
        "image_filter" => {
            if !matches!(scope, Some(Scope::Location)) {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"image_filter\" directive is not allowed here",
                ));
            }
            apply_filter(directive, &mut context.image_filter, seen)?;
        }
        "image_filter_buffer" => {
            if seen.buffer_size {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"image_filter_buffer\" directive is duplicate",
                ));
            }
            seen.buffer_size = true;
            context.image_filter.buffer_size =
                parse_size(one_arg(directive)?, &directive.location)?;
        }
        "image_filter_jpeg_quality" => {
            reject_duplicate(directive, seen.jpeg_quality)?;
            seen.jpeg_quality = true;
            context.image_filter.jpeg_quality = Some(parse_percentage(directive, 1)?);
        }
        "image_filter_webp_quality" => {
            reject_duplicate(directive, seen.webp_quality)?;
            seen.webp_quality = true;
            context.image_filter.webp_quality = Some(parse_percentage(directive, 1)?);
        }
        "image_filter_sharpen" => {
            reject_duplicate(directive, seen.sharpen)?;
            seen.sharpen = true;
            context.image_filter.sharpen = Some(parse_percentage(directive, 0)?);
        }
        "image_filter_transparency" => {
            if seen.transparency {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"image_filter_transparency\" directive is duplicate",
                ));
            }
            seen.transparency = true;
            context.image_filter.transparency = parse_flag(directive, "image_filter_transparency")?;
        }
        "image_filter_interlace" => {
            if seen.interlace {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "\"image_filter_interlace\" directive is duplicate",
                ));
            }
            seen.interlace = true;
            context.image_filter.interlace = parse_flag(directive, "image_filter_interlace")?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn apply_filter(
    directive: &Directive,
    config: &mut ImageFilterConfig,
    seen: &mut ImageFilterSeen,
) -> ConfigResult<()> {
    if !seen.filter {
        config.mode = ImageFilterMode::Off;
        config.width = None;
        config.height = None;
        config.angle = None;
        seen.filter = true;
    }

    match directive.args.as_slice() {
        [mode] => {
            config.mode = match mode.as_str() {
                "off" => ImageFilterMode::Off,
                "test" => ImageFilterMode::Test,
                "size" => ImageFilterMode::Size,
                _ => return invalid_image_filter(directive),
            };
        }
        [mode, angle] if mode == "rotate" => {
            if !angle.contains('$') && !matches!(angle.as_str(), "90" | "180" | "270") {
                return invalid_image_filter(directive);
            }
            if !matches!(config.mode, ImageFilterMode::Resize | ImageFilterMode::Crop) {
                config.mode = ImageFilterMode::Rotate;
            }
            config.angle = Some(angle.clone());
        }
        [mode, width, height] if matches!(mode.as_str(), "resize" | "crop") => {
            validate_dimension(width, directive)?;
            validate_dimension(height, directive)?;
            config.mode = if mode == "resize" {
                ImageFilterMode::Resize
            } else {
                ImageFilterMode::Crop
            };
            config.width = Some(width.clone());
            config.height = Some(height.clone());
        }
        _ => return invalid_image_filter(directive),
    }
    Ok(())
}

fn validate_dimension(value: &str, directive: &Directive) -> ConfigResult<()> {
    if value.contains('$') || value == "-" {
        return Ok(());
    }
    if value.parse::<u64>().is_ok_and(|value| value > 0) {
        return Ok(());
    }
    invalid_image_filter(directive)
}

fn parse_percentage(directive: &Directive, minimum: u64) -> ConfigResult<String> {
    let value = one_arg(directive)?;
    if value.contains('$') {
        return Ok(value.to_string());
    }
    if !value
        .parse::<u64>()
        .is_ok_and(|value| (minimum..=100).contains(&value))
    {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value \"{value}\""),
        ));
    }
    Ok(value.to_string())
}

fn reject_duplicate(directive: &Directive, seen: bool) -> ConfigResult<()> {
    if seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    Ok(())
}

fn invalid_image_filter<T>(directive: &Directive) -> ConfigResult<T> {
    Err(ConfigError::at(
        directive.location.clone(),
        "invalid arguments in \"image_filter\" directive",
    ))
}
