use super::*;

pub(in crate::config::compile) fn parse_duration(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<Duration> {
    let seconds = parse_time_seconds(value, location)?;
    Ok(Duration::from_secs_f64(seconds))
}

pub(in crate::config::compile) fn parse_time_seconds(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<f64> {
    if value.is_empty() {
        return Err(ConfigError::at(location.clone(), "empty time value"));
    }
    let split = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(value.len());
    let (number, suffix) = value.split_at(split);
    let number = number.parse::<f64>().map_err(|_| {
        ConfigError::at(location.clone(), format!("invalid time value \"{value}\""))
    })?;
    let multiplier = match suffix {
        "" | "s" => 1.0,
        "ms" => 0.001,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        "M" => 2592000.0,
        "y" => 31536000.0,
        _ => {
            return Err(ConfigError::at(
                location.clone(),
                format!("invalid time value \"{value}\""),
            ));
        }
    };
    Ok(number * multiplier)
}
