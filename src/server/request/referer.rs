use super::*;
use crate::config::ValidReferers;

pub(in crate::server) fn invalid_referer_value(
    config: &ValidReferers,
    referer: Option<&str>,
    server_names: &[ServerName],
) -> String {
    if !config.is_configured() {
        return String::new();
    }
    let Some(referer) = referer else {
        return invalid_value(!config.no_referer);
    };
    let Some(authority) = referer_authority(referer) else {
        return invalid_value(!config.blocked_referer);
    };
    let host_end = authority.find(['/', ':']).unwrap_or(authority.len());
    let host = &authority[..host_end];
    if host.is_empty() || host.len() > 256 {
        return invalid_value(true);
    }

    if config.server_names && server_names.iter().any(|name| name.matches(host)) {
        return String::new();
    }
    let path = authority
        .find('/')
        .map(|position| &authority[position..])
        .unwrap_or_default();
    for rule in &config.hosts {
        if !rule.host.matches(host) {
            continue;
        }
        if rule.uri.as_deref().is_none_or(|uri| path.starts_with(uri)) {
            return String::new();
        }
    }
    if config.regexes.iter().any(|regex| regex.is_match(authority)) {
        return String::new();
    }
    invalid_value(true)
}

fn referer_authority(referer: &str) -> Option<&str> {
    if referer
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
    {
        referer.get(7..)
    } else if referer
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
    {
        referer.get(8..)
    } else {
        None
    }
}

fn invalid_value(invalid: bool) -> String {
    if invalid { "1".into() } else { String::new() }
}
