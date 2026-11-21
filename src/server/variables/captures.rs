use super::*;
use crate::config::{Captures, Regex};

pub(in crate::server) fn expand_captures(
    template: &str,
    captures: &Captures<'_>,
    variables: &Variables,
) -> String {
    let mut output = String::new();
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'$' {
            if index + 1 < bytes.len() && bytes[index + 1].is_ascii_digit() {
                let capture_index = (bytes[index + 1] - b'0') as usize;
                output.push_str(
                    captures
                        .get(capture_index)
                        .map(|value| value.as_str())
                        .unwrap_or(""),
                );
                index += 2;
                continue;
            }
            if index + 2 < bytes.len() && bytes[index + 1] == b'{' {
                let start = index + 2;
                let Some(end) = template[start..].find('}').map(|offset| start + offset) else {
                    output.push('$');
                    index += 1;
                    continue;
                };
                let name = &template[start..end];
                if name.len() == 1 && name.as_bytes()[0].is_ascii_digit() {
                    let capture_index = (name.as_bytes()[0] - b'0') as usize;
                    output.push_str(
                        captures
                            .get(capture_index)
                            .map(|value| value.as_str())
                            .unwrap_or(""),
                    );
                    index = end + 1;
                    continue;
                }
                if let Some(value) = captures.name(name) {
                    output.push_str(value.as_str());
                    index = end + 1;
                    continue;
                }
            }
            let rest = &template[index..];
            let next = rest[1..]
                .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .map(|offset| offset + 1)
                .unwrap_or(rest.len());
            output.push_str(&eval_variables(&rest[..next], variables));
            index += next;
        } else {
            let character = template[index..].chars().next().unwrap();
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

pub(in crate::server) fn set_regex_captures(
    variables: &mut Variables,
    regex: &Regex,
    captures: &Captures<'_>,
) {
    variables.captures = captures
        .iter()
        .map(|capture| capture.map_or_else(String::new, |capture| capture.as_str().to_string()))
        .collect();
    for name in regex.capture_names().flatten() {
        if let Some(value) = captures.name(name) {
            variables
                .custom
                .insert(name.to_string(), value.as_str().to_string());
        }
    }
}
