use super::*;

pub(in crate::server) fn map_value(
    map: &MapVariable,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> String {
    if stack.iter().any(|name| name == &map.name) {
        return String::new();
    }
    stack.push(map.name.clone());
    let mut source = eval_variables_with_stack(&map.source, variables, stack);
    if map.hostnames {
        source = source.trim_end_matches('.').to_string();
    }
    let source_lower = source.to_ascii_lowercase();
    let exact = map.exact.get(&source_lower).cloned();
    let wildcard = if exact.is_none() {
        map.leading_wildcards
            .iter()
            .find(|(suffix, _)| {
                let bare = suffix.strip_prefix('.').unwrap_or(suffix);
                source_lower == bare || source_lower.ends_with(suffix)
            })
            .map(|(_, value)| value.clone())
            .or_else(|| {
                map.trailing_wildcards
                    .iter()
                    .find(|(prefix, _)| {
                        let bare = prefix.strip_suffix('.').unwrap_or(prefix);
                        source_lower == bare || source_lower.starts_with(prefix)
                    })
                    .map(|(_, value)| value.clone())
            })
    } else {
        None
    };
    let regex_match = if exact.is_none() && wildcard.is_none() {
        map.regexes.iter().find_map(|(regex, value)| {
            regex
                .captures(&source)
                .map(|captures| (value.clone(), captures))
        })
    } else {
        None
    };
    let value = exact
        .or(wildcard)
        .or_else(|| regex_match.as_ref().map(|(value, _)| value.clone()))
        .or_else(|| map.default.clone())
        .unwrap_or_default();
    let output = if let Some((_, captures)) = regex_match {
        let mut captured_variables = variables.clone();
        captured_variables.captures = captures
            .iter()
            .map(|capture| capture.map_or_else(String::new, |capture| capture.as_str().to_string()))
            .collect();
        eval_variables_with_stack(&value, &captured_variables, stack)
    } else {
        eval_variables_with_stack(&value, variables, stack)
    };
    stack.pop();
    output
}

pub(in crate::server) fn geo_value(
    geo: &GeoVariable,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> String {
    if stack.iter().any(|name| name == &geo.name) {
        return String::new();
    }
    stack.push(geo.name.clone());
    let source = eval_variables_with_stack(&geo.source, variables, stack);
    let address =
        source
            .split(',')
            .next()
            .and_then(parse_geo_address)
            .map(|address| match address {
                std::net::IpAddr::V6(address) => address
                    .to_ipv4_mapped()
                    .map(std::net::IpAddr::V4)
                    .unwrap_or(std::net::IpAddr::V6(address)),
                address => address,
            });
    let value = if geo.ranges {
        address
            .and_then(|address| match address {
                std::net::IpAddr::V4(address) => {
                    let address = u32::from(address);
                    geo.range_entries
                        .iter()
                        .find(|(range, _)| (range.start..=range.end).contains(&address))
                        .map(|(_, value)| value)
                }
                std::net::IpAddr::V6(_) => None,
            })
            .or(geo.default.as_ref())
    } else {
        address
            .and_then(|address| {
                geo.networks
                    .iter()
                    .find(|(network, _)| network.contains(address))
                    .map(|(_, value)| value)
            })
            .or(geo.default.as_ref())
    };
    let output = value.cloned().unwrap_or_default();
    stack.pop();
    output
}

pub(in crate::server) fn split_clients_value(
    split: &SplitClientsVariable,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> String {
    if stack.iter().any(|name| name == &split.name) {
        return String::new();
    }
    stack.push(split.name.clone());
    let source = eval_variables_with_stack(&split.source, variables, stack);
    let hash = nginx_murmur_hash2(source.as_bytes());
    let value = split
        .parts
        .iter()
        .find(|part| part.percent == 0 || hash < part.threshold)
        .map(|part| part.value.clone())
        .unwrap_or_default();
    stack.pop();
    value
}

pub(in crate::server) fn nginx_murmur_hash2(data: &[u8]) -> u32 {
    const MULTIPLIER: u32 = 0x5bd1_e995;
    let mut hash = data.len() as u32;
    let (chunks, remainder) = data.as_chunks::<4>();
    for chunk in chunks {
        let mut value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        value = value.wrapping_mul(MULTIPLIER);
        value ^= value >> 24;
        value = value.wrapping_mul(MULTIPLIER);
        hash = hash.wrapping_mul(MULTIPLIER);
        hash ^= value;
    }
    match remainder.len() {
        3 => {
            hash ^= u32::from(remainder[2]) << 16;
            hash ^= u32::from(remainder[1]) << 8;
            hash ^= u32::from(remainder[0]);
            hash = hash.wrapping_mul(MULTIPLIER);
        }
        2 => {
            hash ^= u32::from(remainder[1]) << 8;
            hash ^= u32::from(remainder[0]);
            hash = hash.wrapping_mul(MULTIPLIER);
        }
        1 => {
            hash ^= u32::from(remainder[0]);
            hash = hash.wrapping_mul(MULTIPLIER);
        }
        _ => {}
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(MULTIPLIER);
    hash ^ (hash >> 15)
}

pub(in crate::server) fn parse_geo_address(value: &str) -> Option<std::net::IpAddr> {
    let value = value.trim().trim_matches('"');
    if let Ok(address) = value.parse() {
        return Some(address);
    }
    if let Some(value) = value.strip_prefix('[')
        && let Some((address, _)) = value.split_once(']')
        && let Ok(address) = address.parse()
    {
        return Some(address);
    }
    let (address, port) = value.rsplit_once(':')?;
    if port.bytes().all(|byte| byte.is_ascii_digit()) {
        return address.parse().ok();
    }
    None
}
