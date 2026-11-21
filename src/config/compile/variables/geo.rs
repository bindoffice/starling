use super::*;

pub(in crate::config::compile) fn compile_geo(directive: &Directive) -> ConfigResult<GeoVariable> {
    if directive.args.is_empty() || directive.args.len() > 2 {
        return Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"geo\" directive",
        ));
    }
    let (source, name) = if directive.args.len() == 1 {
        (
            "$remote_addr".to_string(),
            parse_variable_name(&directive.args[0], &directive.location, "geo")?,
        )
    } else {
        if !directive.args[0].starts_with('$') {
            return Err(ConfigError::at(
                directive.location.clone(),
                "invalid source address in \"geo\" directive",
            ));
        }
        (
            directive.args[0].clone(),
            parse_variable_name(&directive.args[1], &directive.location, "geo")?,
        )
    };
    let mut geo = GeoVariable {
        name,
        source,
        default: None,
        networks: Vec::new(),
        ranges: false,
        range_entries: Vec::new(),
    };
    for child in &directive.children {
        validate_no_block(child)?;
        match child.name.as_str() {
            "default" => {
                if child.args.len() != 1 {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"geo\" default",
                    ));
                }
                if geo.default.replace(child.args[0].clone()).is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate default in \"geo\" directive",
                    ));
                }
            }
            "ranges" => {
                if geo.ranges {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"ranges\" directive is duplicate in \"geo\" context",
                    ));
                }
                if !geo.networks.is_empty() || !geo.range_entries.is_empty() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "\"ranges\" directive must be the first directive inside \"geo\" block",
                    ));
                }
                geo.ranges = true;
            }
            "volatile" => {}
            "proxy" | "proxy_recursive" => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("\"{}\" is not implemented in \"geo\" context", child.name),
                ));
            }
            "delete" => {
                if child.args.len() != 1 {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"geo\" delete",
                    ));
                }
                if geo.ranges {
                    let range = parse_ip_range(&child.args[0], &child.location)?;
                    geo.range_entries.retain(|(existing, _)| existing != &range);
                } else {
                    let network = parse_ip_network(&child.args[0], &child.location)?;
                    geo.networks.retain(|(existing, _)| existing != &network);
                }
            }
            _ => {
                if child.args.is_empty() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of the geo parameters",
                    ));
                }
                let value = child.args.join(" ");
                if geo.ranges {
                    let range = parse_ip_range(&child.name, &child.location)?;
                    if geo
                        .range_entries
                        .iter()
                        .any(|(existing, _)| existing == &range)
                    {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("duplicate range in \"geo\" directive: {}", child.name),
                        ));
                    }
                    geo.range_entries.push((range, value));
                } else {
                    let network = parse_ip_network(&child.name, &child.location)?;
                    if geo
                        .networks
                        .iter()
                        .any(|(existing, _)| existing == &network)
                    {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("duplicate network in \"geo\" directive: {}", child.name),
                        ));
                    }
                    geo.networks.push((network, value));
                }
            }
        }
    }
    geo.networks
        .sort_by_key(|(network, _)| std::cmp::Reverse(network.prefix));
    geo.range_entries.sort_by_key(|(range, _)| range.start);
    for pair in geo.range_entries.windows(2) {
        if pair[0].0.end >= pair[1].0.start {
            return Err(ConfigError::at(
                directive.location.clone(),
                "overlapping geo ranges",
            ));
        }
    }
    Ok(geo)
}
