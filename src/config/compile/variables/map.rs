use super::*;

pub(in crate::config::compile) fn compile_map(directive: &Directive) -> ConfigResult<MapVariable> {
    if directive.args.len() != 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"map\" directive",
        ));
    }
    let source = directive.args[0].clone();
    let name = parse_variable_name(&directive.args[1], &directive.location, "map")?;

    let mut map = MapVariable {
        name,
        source,
        default: None,
        hostnames: false,
        volatile: false,
        exact: HashMap::new(),
        leading_wildcards: Vec::new(),
        trailing_wildcards: Vec::new(),
        regexes: Vec::new(),
    };
    for child in &directive.children {
        validate_no_block(child)?;
        if child.args.is_empty() {
            match child.name.as_str() {
                "hostnames" => {
                    map.hostnames = true;
                    continue;
                }
                "volatile" => {
                    map.volatile = true;
                    continue;
                }
                _ => {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        format!("invalid number of the map parameters in \"{}\"", child.name),
                    ));
                }
            }
        }
        if child.args.len() != 1 {
            return Err(ConfigError::at(
                child.location.clone(),
                "invalid number of the map parameters",
            ));
        }
        let value = child.args[0].clone();
        if child.name == "default" {
            if map.default.replace(value).is_some() {
                return Err(ConfigError::at(
                    child.location.clone(),
                    "duplicate default map parameter",
                ));
            }
            continue;
        }

        let key = &child.name;
        if let Some(pattern) = key.strip_prefix('~') {
            let (pattern, case_insensitive) = match pattern.strip_prefix('*') {
                Some(pattern) => (pattern, true),
                None => (pattern, false),
            };
            let regex = RegexBuilder::new(pattern)
                .case_insensitive(case_insensitive)
                .build()
                .map_err(|err| {
                    ConfigError::at(
                        child.location.clone(),
                        format!("invalid map regular expression: {err}"),
                    )
                })?;
            map.regexes.push((regex, value));
            continue;
        }

        let key = key.strip_prefix('\\').unwrap_or(key);
        let normalized = key.to_ascii_lowercase();
        if map.hostnames && normalized.starts_with("*.") {
            map.leading_wildcards
                .push((normalized[1..].to_string(), value));
        } else if map.hostnames && normalized.ends_with(".*") {
            map.trailing_wildcards
                .push((normalized[..normalized.len() - 1].to_string(), value));
        } else if map.exact.insert(normalized, value).is_some() {
            return Err(ConfigError::at(
                child.location.clone(),
                format!("conflicting map parameter \"{key}\""),
            ));
        }
    }
    map.leading_wildcards
        .sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
    map.trailing_wildcards
        .sort_by_key(|(key, _)| std::cmp::Reverse(key.len()));
    Ok(map)
}
