use super::state::State;
use super::*;
use std::fmt::Write as _;

use chrono::{Local, TimeZone, Utc};

pub(super) fn evaluate_text(
    text: &str,
    state: &State,
    variables: &Variables,
) -> Result<String, String> {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let character = text[index..].chars().next().unwrap();
        if character == '\\' {
            let next_index = index + 1;
            if next_index >= text.len() {
                output.push('\\');
                break;
            }
            let next = text[next_index..].chars().next().unwrap();
            if matches!(next, '\\' | '\'' | '"' | '$') {
                output.push(next);
                index = next_index + next.len_utf8();
            } else {
                output.push('\\');
                index = next_index;
            }
            continue;
        }
        if character != '$' {
            output.push(character);
            index += character.len_utf8();
            continue;
        }

        let name_start = index + 1;
        let (name, next_index) = if text[name_start..].starts_with('{') {
            let name_start = name_start + 1;
            let Some(end) = text[name_start..].find('}') else {
                return Err("the closing bracket in SSI variable is missing".into());
            };
            (&text[name_start..name_start + end], name_start + end + 1)
        } else {
            let mut end = name_start;
            while end < text.len()
                && (text.as_bytes()[end].is_ascii_alphanumeric() || text.as_bytes()[end] == b'_')
            {
                end += 1;
            }
            if end == name_start {
                return Err("invalid SSI variable name".into());
            }
            (&text[name_start..end], end)
        };
        if let Some(value) = lookup_variable(name, state, variables) {
            output.push_str(&value);
        }
        index = next_index;
    }
    Ok(output)
}

pub(super) fn lookup_variable(name: &str, state: &State, variables: &Variables) -> Option<String> {
    if name == "date_local" {
        return Some(format_timestamp(&state.timefmt, false));
    }
    if name == "date_gmt" {
        return Some(format_timestamp(&state.timefmt, true));
    }
    if let Some(value) = state.variables.get(name) {
        return Some(value.clone());
    }
    if let Some(value) = state.named_captures.get(name) {
        return Some(value.clone());
    }
    if let Some(index) = name.parse::<usize>().ok()
        && let Some(value) = state.captures.get(index)
    {
        return Some(value.clone());
    }
    if let Some(argument) = name.strip_prefix("arg_")
        && !variables.args.split('&').any(|value| {
            value
                .split_once('=')
                .map_or(value, |(name, _)| name)
                .eq_ignore_ascii_case(argument)
        })
    {
        return None;
    }
    if let Some(header) = name.strip_prefix("http_") {
        let header = header.replace('_', "-");
        if !variables
            .headers
            .iter()
            .any(|value| value.name.eq_ignore_ascii_case(&header))
        {
            return None;
        }
    }
    let mut stack = Vec::new();
    Some(variable_value(name, variables, &mut stack))
}

pub(super) fn entity_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            _ => output.push(character),
        }
    }
    output
}

pub(super) fn url_encode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if !(0x20..0x7f).contains(&byte)
            || matches!(
                byte,
                b' ' | b'"'
                    | b'#'
                    | b'%'
                    | b'\''
                    | b'<'
                    | b'>'
                    | b'\\'
                    | b'^'
                    | b'`'
                    | b'{'
                    | b'|'
                    | b'}'
            )
        {
            output.push('%');
            output.push_str(&format!("{byte:02X}"));
        } else {
            output.push(byte as char);
        }
    }
    output
}

pub(super) fn format_timestamp(format: &str, gmt: bool) -> String {
    if format == "%s" {
        return std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs().to_string())
            .unwrap_or_default();
    }
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let mut output = String::new();
    let result = if gmt {
        let Some(time) = Utc.timestamp_opt(seconds, 0).single() else {
            return String::new();
        };
        write!(output, "{}", time.format(&format.replace("%Z", "GMT")))
    } else {
        let Some(time) = Local.timestamp_opt(seconds, 0).earliest() else {
            return String::new();
        };
        write!(output, "{}", time.format(format))
    };
    if result.is_err() {
        String::new()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_encoding_matches_ssi_defaults() {
        assert_eq!(entity_encode(r#"a&<b>"#), "a&amp;&lt;b&gt;");
    }

    #[test]
    fn url_encoding_uses_nginx_html_escape_rules() {
        assert_eq!(url_encode("a/b?x=1&y=<tag>"), "a/b?x=1&y=%3Ctag%3E");
        assert_eq!(url_encode("a b\"'%"), "a%20b%22%27%25");
    }
}
