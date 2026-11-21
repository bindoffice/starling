use super::*;

pub(super) fn parse_location_matcher(directive: &Directive) -> ConfigResult<LocationMatcher> {
    match directive.args.as_slice() {
        [value] if value.starts_with('=') => {
            let path = value.strip_prefix('=').unwrap_or_default().trim();
            if path.is_empty() {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "invalid location modifier",
                ));
            }
            Ok(LocationMatcher::Exact(normalize_location_path(path)))
        }
        [modifier, path] if modifier == "=" => {
            Ok(LocationMatcher::Exact(normalize_location_path(path)))
        }
        [value] if value.starts_with('^') => {
            let path = value.trim_start_matches('^').trim_start_matches('~');
            Ok(LocationMatcher::PrefixNoRegex(normalize_location_path(
                path,
            )))
        }
        [modifier, path] if modifier == "^~" => Ok(LocationMatcher::PrefixNoRegex(
            normalize_location_path(path),
        )),
        [modifier, pattern] if modifier == "~" || modifier == "~*" => {
            let regex = RegexBuilder::new(pattern)
                .case_insensitive(modifier == "~*")
                .build()
                .map_err(|err| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid regular expression: {err}"),
                    )
                })?;
            Ok(LocationMatcher::Regex(regex))
        }
        [pattern] if pattern.starts_with('~') => {
            let (case_insensitive, expression) =
                if let Some(expression) = pattern.strip_prefix("~*") {
                    (true, expression)
                } else {
                    (false, pattern.strip_prefix('~').unwrap_or(pattern))
                };
            let regex = RegexBuilder::new(expression)
                .case_insensitive(case_insensitive)
                .build()
                .map_err(|err| {
                    ConfigError::at(
                        directive.location.clone(),
                        format!("invalid regular expression: {err}"),
                    )
                })?;
            Ok(LocationMatcher::Regex(regex))
        }
        [name] if name.starts_with('@') => Ok(LocationMatcher::Named(name.clone())),
        [path] => Ok(LocationMatcher::Prefix(normalize_location_path(path))),
        _ => Err(ConfigError::at(
            directive.location.clone(),
            "invalid number of arguments in \"location\" directive",
        )),
    }
}
