use super::StreamSslPreread;
use super::wire::{ssl_version_name, take, take_u8, take_u16};

pub(super) fn parse_client_hello(body: &[u8]) -> Option<StreamSslPreread> {
    if body.len() < 35 {
        return None;
    }
    let mut context = StreamSslPreread {
        protocol: ssl_version_name(body[0], body[1]).to_string(),
        ..StreamSslPreread::default()
    };
    let mut position = 34;
    let session_id_len = take_u8(body, &mut position)? as usize;
    take(body, &mut position, session_id_len)?;
    let cipher_suites_len = take_u16(body, &mut position)? as usize;
    take(body, &mut position, cipher_suites_len)?;
    let compression_len = take_u8(body, &mut position)? as usize;
    take(body, &mut position, compression_len)?;
    if position == body.len() {
        return Some(context);
    }
    let extensions_len = take_u16(body, &mut position)? as usize;
    let extensions = take(body, &mut position, extensions_len)?;
    if position != body.len() {
        return None;
    }

    parse_extensions(extensions, &mut context)?;
    Some(context)
}

fn parse_extensions(extensions: &[u8], context: &mut StreamSslPreread) -> Option<()> {
    let mut position = 0;
    let mut seen_sni = false;
    let mut seen_alpn = false;
    while position < extensions.len() {
        let extension_type = take_u16(extensions, &mut position)?;
        let extension_len = take_u16(extensions, &mut position)? as usize;
        let extension = take(extensions, &mut position, extension_len)?;
        match extension_type {
            0 => {
                if seen_sni {
                    return None;
                }
                seen_sni = true;
                context.server_name = parse_sni(extension)?;
            }
            16 => {
                if seen_alpn {
                    return None;
                }
                seen_alpn = true;
                context.alpn_protocols = parse_alpn(extension)?;
            }
            43 if parse_supported_versions(extension) => {
                context.protocol = "TLSv1.3".into();
            }
            _ => {}
        }
    }
    Some(())
}

fn parse_sni(data: &[u8]) -> Option<String> {
    if data.len() < 5 {
        return None;
    }
    let names_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if names_len + 2 != data.len() || data[2] != 0 {
        return None;
    }
    let host_len = u16::from_be_bytes([data[3], data[4]]) as usize;
    if host_len == 0 || 5 + host_len > data.len() {
        return None;
    }
    let host = std::str::from_utf8(&data[5..5 + host_len]).ok()?;
    Some(host.trim_end_matches('.').to_ascii_lowercase())
}

fn parse_alpn(data: &[u8]) -> Option<String> {
    if data.len() < 2 {
        return None;
    }
    let protocols_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if protocols_len + 2 != data.len() {
        return None;
    }
    let mut protocols = Vec::new();
    let mut position = 2;
    while position < data.len() {
        let length = take_u8(data, &mut position)? as usize;
        if length == 0 {
            return None;
        }
        let protocol = take(data, &mut position, length)?;
        protocols.push(std::str::from_utf8(protocol).ok()?);
    }
    Some(protocols.join(","))
}

fn parse_supported_versions(data: &[u8]) -> bool {
    if data.len() < 3 || data[0] as usize + 1 != data.len() {
        return false;
    }
    data[1..].as_chunks::<2>().0.contains(&[3, 4])
}
