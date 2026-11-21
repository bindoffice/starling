use super::*;

pub(super) fn parse_temp_path(
    directive: &Directive,
    prefix: &Path,
) -> ConfigResult<TempPathConfig> {
    if !(1..=4).contains(&directive.args.len()) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }

    let mut levels = [0; 3];
    let mut hashed_len = 0_usize;
    for (index, value) in directive.args[1..].iter().enumerate() {
        let level = value.parse::<usize>().ok().filter(|level| *level > 0);
        let Some(level) = level else {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "invalid value \"{value}\" in \"{}\" directive",
                    directive.name
                ),
            ));
        };
        levels[index] = level;
        hashed_len = level
            .checked_add(1)
            .and_then(|level| hashed_len.checked_add(level))
            .ok_or_else(|| {
                ConfigError::at(
                    directive.location.clone(),
                    format!("invalid value in \"{}\" directive", directive.name),
                )
            })?;
    }

    let level_count = directive.args.len() - 1;
    if hashed_len > 10 + level_count {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("invalid value in \"{}\" directive", directive.name),
        ));
    }

    Ok(TempPathConfig {
        path: resolve_path(prefix, &directive.args[0]),
        levels,
    })
}
