use super::*;

pub(in crate::config::compile) fn parse_server_name(
    value: &str,
    location: &SourceLocation,
) -> ConfigResult<ServerName> {
    if let Some(pattern) = value.strip_prefix("~*") {
        let regex = RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
            .map_err(|err| {
                ConfigError::at(
                    location.clone(),
                    format!("invalid server_name regex: {err}"),
                )
            })?;
        return Ok(ServerName::Regex(regex));
    }
    if let Some(pattern) = value.strip_prefix('~') {
        let regex = Regex::new(pattern).map_err(|err| {
            ConfigError::at(
                location.clone(),
                format!("invalid server_name regex: {err}"),
            )
        })?;
        return Ok(ServerName::Regex(regex));
    }
    if let Some(suffix) = value.strip_prefix('.') {
        return Ok(ServerName::LeadingWildcard(
            format!(".{suffix}").to_ascii_lowercase(),
        ));
    }
    if let Some(prefix) = value.strip_suffix('.') {
        return Ok(ServerName::TrailingWildcard(
            format!("{prefix}.").to_ascii_lowercase(),
        ));
    }
    if value.starts_with("*.") {
        return Ok(ServerName::LeadingWildcard(value[1..].to_ascii_lowercase()));
    }
    if value.ends_with(".*") {
        return Ok(ServerName::TrailingWildcard(
            value[..value.len() - 1].to_ascii_lowercase(),
        ));
    }
    Ok(ServerName::Exact(value.to_ascii_lowercase()))
}
