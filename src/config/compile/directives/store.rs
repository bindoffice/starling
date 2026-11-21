use super::*;

pub(in crate::config::compile) fn apply_store_value(
    directive: &Directive,
    store: &mut StoreConfig,
    seen: &mut bool,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if *seen {
        return Err(duplicate_error(directive));
    }
    *seen = true;
    let value = one_arg(directive)?;
    if value.is_empty() {
        return Err(ConfigError::at(directive.location.clone(), "empty path"));
    }
    *store = match value {
        "off" => StoreConfig::Off,
        "on" => StoreConfig::On,
        path => StoreConfig::Path(path.to_string()),
    };
    Ok(())
}

pub(in crate::config::compile) fn apply_store_access(
    directive: &Directive,
    access: &mut u32,
    seen: &mut bool,
) -> ConfigResult<()> {
    if *seen {
        return Err(duplicate_error(directive));
    }
    *seen = true;
    *access = parse_file_access(directive, &directive.name)?;
    Ok(())
}

fn duplicate_error(directive: &Directive) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!("\"{}\" directive is duplicate", directive.name),
    )
}
