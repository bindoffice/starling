use super::cache_shared::{invalid_arguments, invalid_value, parse_on_off, parse_positive_usize};
use super::*;

const MIN_KEYS_ZONE_BYTES: usize = 8 * 1024;

pub(in crate::config::compile) fn compile_cache_path(
    directive: &Directive,
    prefix: &Path,
) -> ConfigResult<ProxyCachePathConfig> {
    validate_no_block(directive)?;
    if directive.args.len() < 2 {
        return Err(invalid_arguments(directive));
    }

    let path = resolve_path(prefix, &directive.args[0]);
    let mut levels = [0; 3];
    let mut keys_zone = None;
    let mut keys_zone_size = None;
    let mut inactive = Duration::from_secs(10 * 60);
    let mut max_size = None;
    let mut use_temp_path = true;
    let mut loader_files = 100;
    let mut loader_sleep = Duration::from_millis(50);
    let mut loader_threshold = Duration::from_millis(200);
    let mut manager_files = 100;
    let mut manager_sleep = Duration::from_millis(50);
    let mut manager_threshold = Duration::from_millis(200);
    let mut min_free = None;

    for parameter in &directive.args[1..] {
        let Some((name, value)) = parameter.split_once('=') else {
            return Err(invalid_parameter(directive, parameter));
        };
        if value.is_empty() {
            return Err(invalid_parameter(directive, parameter));
        }
        match name {
            "levels" => {
                levels = parse_cache_levels(value, directive)?;
            }
            "keys_zone" => {
                if keys_zone.is_some() {
                    return Err(invalid_parameter(directive, parameter));
                }
                let Some((name, size)) = value.split_once(':') else {
                    return Err(invalid_parameter(directive, parameter));
                };
                if name.is_empty() || name == "off" {
                    return Err(invalid_parameter(directive, parameter));
                }
                let size = parse_size(size, &directive.location)
                    .ok()
                    .and_then(|size| usize::try_from(size).ok())
                    .filter(|size| *size > 0)
                    .ok_or_else(|| invalid_parameter(directive, parameter))?;
                if size < MIN_KEYS_ZONE_BYTES {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("zone \"{parameter}\" is too small"),
                    ));
                }
                keys_zone = Some(name.to_string());
                keys_zone_size = Some(size);
            }
            "inactive" => {
                inactive = parse_duration(value, &directive.location)?;
            }
            "max_size" => {
                max_size = Some(parse_size(value, &directive.location)?);
            }
            "use_temp_path" => {
                use_temp_path = parse_on_off(value, directive)?;
            }
            "loader_files" => {
                loader_files = parse_positive_usize(value, directive)?;
            }
            "loader_sleep" => {
                loader_sleep = parse_duration(value, &directive.location)?;
            }
            "loader_threshold" => {
                loader_threshold = parse_duration(value, &directive.location)?;
            }
            "manager_files" => {
                manager_files = parse_positive_usize(value, directive)?;
            }
            "manager_sleep" => {
                manager_sleep = parse_duration(value, &directive.location)?;
            }
            "manager_threshold" => {
                manager_threshold = parse_duration(value, &directive.location)?;
            }
            "min_free" => {
                min_free = Some(parse_size(value, &directive.location)?);
            }
            _ => return Err(invalid_parameter(directive, parameter)),
        }
    }

    let Some(keys_zone) = keys_zone else {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("{} requires a keys_zone parameter", directive.name),
        ));
    };
    Ok(ProxyCachePathConfig {
        path,
        levels,
        keys_zone,
        keys_zone_size: keys_zone_size.expect("keys_zone size accompanies name"),
        inactive,
        max_size,
        use_temp_path,
        loader_files,
        loader_sleep,
        loader_threshold,
        manager_files,
        manager_sleep,
        manager_threshold,
        min_free,
    })
}

fn parse_cache_levels(value: &str, directive: &Directive) -> ConfigResult<[usize; 3]> {
    let parts = value.split(':').collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 3 {
        return Err(invalid_value(directive, value));
    }
    let mut levels = [0; 3];
    let mut hashed_len = 0;
    for (index, part) in parts.iter().enumerate() {
        let level = part
            .parse::<usize>()
            .ok()
            .filter(|level| *level > 0)
            .ok_or_else(|| invalid_value(directive, value))?;
        levels[index] = level;
        hashed_len += level + 1;
    }
    if hashed_len > 10 + parts.len() {
        return Err(invalid_value(directive, value));
    }
    Ok(levels)
}

fn invalid_parameter(directive: &Directive, value: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid parameter \"{value}\" in \"{}\" directive",
            directive.name
        ),
    )
}
