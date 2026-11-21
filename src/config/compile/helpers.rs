use super::*;

pub(super) fn parse_size(value: &str, location: &SourceLocation) -> ConfigResult<u64> {
    parse_size_inner(value)
        .ok_or_else(|| ConfigError::at(location.clone(), format!("invalid size \"{value}\"")))
}

pub(crate) fn parse_size_value(value: &str) -> Option<u64> {
    parse_size_inner(value)
}

pub(super) fn parse_size_inner(value: &str) -> Option<u64> {
    if value == "0" {
        return Some(0);
    }
    let split = value
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let number = number.parse::<u64>().ok()?;
    let multiplier = match suffix.to_ascii_lowercase().as_str() {
        "" => 1,
        "k" => 1024,
        "m" => 1024 * 1024,
        "g" => 1024 * 1024 * 1024,
        _ => return None,
    };
    number.checked_mul(multiplier)
}

pub(super) fn parse_usize(
    value: &str,
    location: &SourceLocation,
    name: &str,
) -> ConfigResult<usize> {
    value.parse::<usize>().map_err(|_| {
        ConfigError::at(
            location.clone(),
            format!("invalid value \"{value}\" in \"{name}\" directive"),
        )
    })
}

pub(super) fn one_arg(directive: &Directive) -> ConfigResult<&str> {
    if directive.args.len() != 1 {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    Ok(&directive.args[0])
}

pub(super) fn require_block(directive: &Directive) -> ConfigResult<()> {
    if !directive.has_block {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"directive {}\" has no opening \"{{\"", directive.name),
        ));
    }
    Ok(())
}

pub(super) fn validate_no_block(directive: &Directive) -> ConfigResult<()> {
    if directive.has_block {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "directive \"{}\" is not terminated by \";\"",
                directive.name
            ),
        ));
    }
    Ok(())
}

pub(super) fn resolve_path(prefix: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        prefix.join(path)
    }
}
