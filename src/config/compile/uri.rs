use super::*;

pub(super) fn normalize_location_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

pub fn normalize_uri(path: &str) -> Option<String> {
    normalize_uri_with_merge(path, true)
}

pub fn normalize_uri_with_merge(path: &str, merge_slashes: bool) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    if decoded.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for (index, part) in decoded.split('/').enumerate() {
        match part {
            "" if merge_slashes || index == 0 => {}
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let mut normalized = String::from("/");
    normalized.push_str(&parts.join("/"));
    if merge_slashes && decoded.ends_with('/') && normalized != "/" {
        normalized.push('/');
    }
    Some(normalized)
}

pub fn split_uri(target: &str) -> (&str, Option<&str>) {
    match target.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (target, None),
    }
}

pub fn match_location<'a>(
    locations: &'a [LocationConfig],
    uri: &str,
) -> Option<&'a LocationConfig> {
    for location in locations {
        if let LocationMatcher::Exact(path) = &location.matcher
            && path == uri
        {
            return Some(location);
        }
    }

    let mut best_prefix: Option<(&LocationConfig, usize)> = None;
    for location in locations {
        match &location.matcher {
            LocationMatcher::Prefix(path) | LocationMatcher::PrefixNoRegex(path)
                if uri.starts_with(path)
                    && best_prefix
                        .as_ref()
                        .is_none_or(|(_, length)| path.len() > *length) =>
            {
                best_prefix = Some((location, path.len()));
            }
            _ => {}
        }
    }
    if let Some((location, _)) = best_prefix
        && matches!(location.matcher, LocationMatcher::PrefixNoRegex(_))
    {
        return Some(location);
    }

    for location in locations {
        if let LocationMatcher::Regex(regex) = &location.matcher
            && regex.is_match(uri)
        {
            return Some(location);
        }
    }

    best_prefix.map(|(location, _)| location)
}
