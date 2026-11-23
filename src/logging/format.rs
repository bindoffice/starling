use crate::config::LogEscapeMode;

pub fn render_log_format(
    template: &str,
    escape: LogEscapeMode,
    mut variable: impl FnMut(&str) -> Option<String>,
) -> String {
    let mut output = Vec::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            let character = template[index..].chars().next().unwrap();
            let mut encoded = [0; 4];
            output.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            index += character.len_utf8();
            continue;
        }

        index += 1;
        if index >= bytes.len() {
            output.push(b'$');
            break;
        }
        let name = if bytes[index] == b'{' {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index] != b'}' {
                index += 1;
            }
            let name = &template[start..index];
            if index < bytes.len() {
                index += 1;
            }
            name
        } else {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            &template[start..index]
        };
        match variable(name) {
            Some(value) => append_escaped(&mut output, value.as_bytes(), escape),
            None => output.push(b'-'),
        }
    }
    String::from_utf8(output)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn append_escaped(output: &mut Vec<u8>, value: &[u8], escape: LogEscapeMode) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    match escape {
        LogEscapeMode::None => output.extend_from_slice(value),
        LogEscapeMode::Default => {
            for &byte in value {
                if matches!(byte, 0x00..=0x1f | b'"' | b'\\' | 0x7f..=0xff) {
                    output.extend_from_slice(b"\\x");
                    output.push(HEX[(byte >> 4) as usize]);
                    output.push(HEX[(byte & 0x0f) as usize]);
                } else {
                    output.push(byte);
                }
            }
        }
        LogEscapeMode::Json => {
            for &byte in value {
                match byte {
                    b'"' | b'\\' => {
                        output.push(b'\\');
                        output.push(byte);
                    }
                    b'\n' => output.extend_from_slice(b"\\n"),
                    b'\r' => output.extend_from_slice(b"\\r"),
                    b'\t' => output.extend_from_slice(b"\\t"),
                    0x08 => output.extend_from_slice(b"\\b"),
                    0x0c => output.extend_from_slice(b"\\f"),
                    0x00..=0x1f => {
                        output.extend_from_slice(b"\\u00");
                        output.push(HEX[(byte >> 4) as usize]);
                        output.push(HEX[(byte & 0x0f) as usize]);
                    }
                    _ => output.push(byte),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_nginx_default_escaping_to_variables_only() {
        let value = "quote\"slash\\line\nnonascii-\u{e9}";
        let rendered = render_log_format("prefix:$value", LogEscapeMode::Default, |_| {
            Some(value.to_string())
        });
        assert_eq!(
            rendered,
            "prefix:quote\\x22slash\\x5Cline\\x0Anonascii-\\xC3\\xA9"
        );
    }

    #[test]
    fn applies_nginx_json_escaping() {
        let value = "quote\"slash\\line\ncontrol\u{1f}tab\t\u{e9}";
        let rendered =
            render_log_format("$value", LogEscapeMode::Json, |_| Some(value.to_string()));
        assert_eq!(
            rendered,
            "quote\\\"slash\\\\line\\ncontrol\\u001Ftab\\t\u{e9}"
        );
    }

    #[test]
    fn leaves_values_unchanged_when_escaping_is_disabled() {
        let value = "quote\"slash\\line\n\u{e9}";
        let rendered =
            render_log_format("$value", LogEscapeMode::None, |_| Some(value.to_string()));
        assert_eq!(rendered, value);
    }

    #[test]
    fn supports_braced_variable_names() {
        let rendered = render_log_format("${request_id}", LogEscapeMode::Default, |name| {
            Some(format!("value-{name}"))
        });
        assert_eq!(rendered, "value-request_id");
    }

    #[test]
    fn renders_missing_variables_as_dash() {
        let rendered = render_log_format("$missing", LogEscapeMode::Default, |_| None);
        assert_eq!(rendered, "-");
    }
}
