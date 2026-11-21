use super::*;

pub(in crate::server) fn stream_map_value(
    map: &MapVariable,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
) -> String {
    if stack.iter().any(|name| name == &map.name) {
        return String::new();
    }
    stack.push(map.name.clone());

    let mut source = eval_stream_variables_with_stack(&map.source, log, stack, captures);
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
                .map(|matched| (regex, value.clone(), matched))
        })
    } else {
        None
    };
    let value = exact
        .or(wildcard)
        .or_else(|| regex_match.as_ref().map(|(_, value, _)| value.clone()))
        .or_else(|| map.default.clone())
        .unwrap_or_default();
    let output = if let Some((regex, _, matched)) = regex_match {
        let captures = StreamCaptures::from_regex(regex, &matched);
        eval_stream_variables_with_stack(&value, log, stack, &captures)
    } else {
        eval_stream_variables_with_stack(&value, log, stack, captures)
    };

    stack.pop();
    output
}
