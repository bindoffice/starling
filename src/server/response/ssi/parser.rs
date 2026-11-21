const COMMAND_MAX: usize = 32;
const PARAMETER_NAME_MAX: usize = 32;

#[derive(Debug, Clone)]
pub(super) struct SsiTag {
    pub(super) command: String,
    pub(super) params: Vec<SsiParam>,
}

#[derive(Debug, Clone)]
pub(super) struct SsiParam {
    pub(super) name: String,
    pub(super) value: String,
}

#[derive(Debug, Clone)]
pub(super) enum Segment {
    Text(Vec<u8>),
    Directive { raw: Vec<u8>, tag: Option<SsiTag> },
}

pub(super) fn parse(input: &[u8], value_length: u64) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let Some(relative) = find_tag_start(&input[cursor..]) else {
            push_text(&mut segments, &input[cursor..]);
            break;
        };
        let start = cursor + relative;
        push_text(&mut segments, &input[cursor..start]);

        let content_start = start + 5;
        let Some(end) = find_tag_end(input, content_start) else {
            push_text(&mut segments, &input[start..]);
            break;
        };
        let tag_end = end + 3;
        let raw = input[start..tag_end].to_vec();
        let parsed = parse_tag(&input[content_start..end], value_length);
        segments.push(Segment::Directive { raw, tag: parsed });
        cursor = tag_end;
    }
    if input.is_empty() {
        return Vec::new();
    }
    segments
}

fn push_text(segments: &mut Vec<Segment>, text: &[u8]) {
    if !text.is_empty() {
        segments.push(Segment::Text(text.to_vec()));
    }
}

fn find_tag_start(input: &[u8]) -> Option<usize> {
    input.windows(5).position(|window| window == b"<!--#")
}

fn find_tag_end(input: &[u8], start: usize) -> Option<usize> {
    let mut cursor = start;
    let mut quote = None;
    let mut escaped = false;
    while cursor + 2 < input.len() {
        let byte = input[cursor];
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == active {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if byte == b'"' || byte == b'\'' {
            quote = Some(byte);
            cursor += 1;
            continue;
        }
        if input[cursor..].starts_with(b"-->") {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

fn parse_tag(input: &[u8], value_length: u64) -> Option<SsiTag> {
    let input = std::str::from_utf8(input).ok()?;
    let bytes = input.as_bytes();
    let mut cursor = 0;
    skip_space(bytes, &mut cursor);
    let command_start = cursor;
    while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if command_start == cursor || cursor - command_start > COMMAND_MAX {
        return None;
    }
    let command = input[command_start..cursor].to_string();
    let mut params = Vec::new();
    loop {
        skip_space(bytes, &mut cursor);
        if cursor == bytes.len() {
            break;
        }

        let name_start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'='
        {
            cursor += 1;
        }
        if name_start == cursor || cursor - name_start > PARAMETER_NAME_MAX {
            return None;
        }
        let name = input[name_start..cursor].to_string();
        skip_space(bytes, &mut cursor);
        if bytes.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        skip_space(bytes, &mut cursor);
        let quote = *bytes.get(cursor)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        cursor += 1;

        let mut value = String::new();
        let mut closed = false;
        while cursor < bytes.len() {
            let character = input[cursor..].chars().next().unwrap();
            cursor += character.len_utf8();
            if character == '\\' {
                let next = input[cursor..].chars().next()?;
                value.push(next);
                cursor += next.len_utf8();
                continue;
            }
            if character as u32 == u32::from(quote) {
                closed = true;
                break;
            }
            value.push(character);
        }
        if !closed || value.len() as u64 > value_length {
            return None;
        }
        params.push(SsiParam { name, value });
    }
    Some(SsiTag { command, params })
}

fn skip_space(bytes: &[u8], cursor: &mut usize) {
    while *cursor < bytes.len() && bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_and_quoted_directives() {
        let segments = parse(
            br#"before <!--# echo var="name" encoding='url' --> after"#,
            255,
        );
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], Segment::Text(text) if text == b"before "));
        let Segment::Directive { tag, .. } = &segments[1] else {
            panic!("expected directive");
        };
        let tag = tag.as_ref().unwrap();
        assert_eq!(tag.command, "echo");
        assert_eq!(tag.params[0].name, "var");
        assert_eq!(tag.params[0].value, "name");
        assert_eq!(tag.params[1].value, "url");
        assert!(matches!(&segments[2], Segment::Text(text) if text == b" after"));
    }

    #[test]
    fn ignores_markers_inside_quoted_values() {
        let segments = parse(br#"<!--# echo var="a-->" -->x"#, 255);
        let Segment::Directive { tag, raw } = &segments[0] else {
            panic!("expected directive");
        };
        assert_eq!(tag.as_ref().unwrap().params[0].value, "a-->");
        assert_eq!(raw, br#"<!--# echo var="a-->" -->"#);
    }

    #[test]
    fn malformed_directive_is_retained_for_error_reporting() {
        let segments = parse(br#"<!--# echo var=name -->"#, 255);
        let Segment::Directive { tag, .. } = &segments[0] else {
            panic!("expected directive");
        };
        assert!(tag.is_none());
    }
}
