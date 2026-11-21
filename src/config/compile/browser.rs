use super::*;

#[derive(Default)]
pub(super) struct BrowserSeen {
    modern: bool,
    ancient: bool,
}

pub(super) fn try_apply_browser_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut BrowserSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "modern_browser" => {
            validate_no_block(directive)?;
            if !seen.modern {
                context.browser.modern.clear();
                context.browser.modern_unlisted = false;
                seen.modern = true;
            }
            match directive.args.as_slice() {
                [value] if value == "unlisted" => {
                    context.browser.modern_unlisted = true;
                }
                [browser, version] => {
                    let browser = parse_browser_kind(browser, &directive.location)?;
                    let version = parse_browser_version(version, &directive.location)?;
                    context
                        .browser
                        .modern
                        .push(ModernBrowserRule { browser, version });
                }
                _ => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "invalid number of arguments in \"modern_browser\" directive",
                    ));
                }
            }
        }
        "ancient_browser" => {
            validate_no_block(directive)?;
            if directive.args.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid number of arguments in \"ancient_browser\" directive",
                ));
            }
            if !seen.ancient {
                context.browser.ancient.clear();
                context.browser.netscape4 = false;
                seen.ancient = true;
            }
            for value in &directive.args {
                if value == "netscape4" {
                    context.browser.netscape4 = true;
                } else {
                    context.browser.ancient.push(value.clone());
                }
            }
        }
        "modern_browser_value" => {
            validate_no_block(directive)?;
            context.browser.modern_value = one_arg(directive)?.to_string();
        }
        "ancient_browser_value" => {
            validate_no_block(directive)?;
            context.browser.ancient_value = one_arg(directive)?.to_string();
        }
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_browser_kind(value: &str, location: &SourceLocation) -> ConfigResult<BrowserKind> {
    match value.to_ascii_lowercase().as_str() {
        "opera" => Ok(BrowserKind::Opera),
        "msie" => Ok(BrowserKind::Msie),
        "gecko" => Ok(BrowserKind::Gecko),
        "safari" => Ok(BrowserKind::Safari),
        "konqueror" => Ok(BrowserKind::Konqueror),
        _ => Err(ConfigError::at(
            location.clone(),
            format!("unknown browser name \"{value}\""),
        )),
    }
}

fn parse_browser_version(value: &str, location: &SourceLocation) -> ConfigResult<u32> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.is_empty()
        || parts.len() > 4
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid_version(value, location));
    }
    let numbers = parts
        .iter()
        .map(|part| part.parse::<u32>().unwrap_or(u32::MAX))
        .collect::<Vec<_>>();
    if numbers[0] > 4000 || numbers[1..].iter().any(|number| *number > 99) {
        return Err(invalid_version(value, location));
    }
    Ok(numbers
        .iter()
        .enumerate()
        .map(|(index, number)| {
            let scale = 1_000_000_u32 / 100_u32.pow(index as u32);
            number.saturating_mul(scale)
        })
        .sum())
}

fn invalid_version(value: &str, location: &SourceLocation) -> ConfigError {
    ConfigError::at(
        location.clone(),
        format!("invalid browser version \"{value}\""),
    )
}
