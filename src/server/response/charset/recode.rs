use crate::config::CharsetConversion;

pub(super) fn convert(input: Vec<u8>, conversion: CharsetConversion<'_>) -> Vec<u8> {
    match conversion {
        CharsetConversion::ToBytes(table) => {
            input.into_iter().map(|byte| table[byte as usize]).collect()
        }
        CharsetConversion::ToUtf8(table) => {
            let mut output = Vec::with_capacity(input.len());
            for byte in input {
                output.extend_from_slice(&table[byte as usize]);
            }
            output
        }
        CharsetConversion::FromUtf8(table) => convert_from_utf8(&input, |character| {
            table.get(&character).copied().filter(|byte| *byte != 0)
        }),
    }
}

fn convert_from_utf8(input: &[u8], mapped: impl Fn(u32) -> Option<u8>) -> Vec<u8> {
    let mut output = Vec::with_capacity(input.len());
    let mut position = 0;
    while position < input.len() {
        let byte = input[position];
        if byte < 0x80 {
            output.push(byte);
            position += 1;
            continue;
        }
        let width = match byte {
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => {
                output.push(b'?');
                position += 1;
                continue;
            }
        };
        let end = position.saturating_add(width);
        let Some(character) = input
            .get(position..end)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .and_then(|text| text.chars().next())
        else {
            output.push(b'?');
            position += 1;
            continue;
        };
        let codepoint = character as u32;
        if codepoint <= 0xffff
            && let Some(mapped) = mapped(codepoint)
        {
            output.push(mapped);
        } else {
            output.extend_from_slice(format!("&#{codepoint};").as_bytes());
        }
        position = end;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_entities_for_unmapped_utf8_characters() {
        assert_eq!(
            convert_from_utf8("A\u{4e2d}".as_bytes(), |_| None),
            b"A&#20013;"
        );
    }
}
