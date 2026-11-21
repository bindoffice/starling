use super::StreamSslPreread;
use super::client_hello::parse_client_hello;
use super::wire::ssl_version_name;

#[derive(Debug)]
pub(super) enum ParseSslPreread {
    Complete(Option<StreamSslPreread>),
    NeedMore,
}

pub(super) fn parse_ssl_preread(buffer: &[u8]) -> ParseSslPreread {
    if buffer.len() < 5 {
        return ParseSslPreread::NeedMore;
    }
    if buffer[0] & 0x80 != 0 && buffer[2] == 1 && matches!(buffer[3], 0 | 3) {
        let context = StreamSslPreread {
            protocol: ssl_version_name(buffer[3], buffer[4]).to_string(),
            ..StreamSslPreread::default()
        };
        return ParseSslPreread::Complete(Some(context));
    }
    if buffer[0] != 0x16 || buffer[1] != 3 {
        return ParseSslPreread::Complete(None);
    }

    let mut handshake = Vec::new();
    let mut offset = 0;
    loop {
        if buffer.len().saturating_sub(offset) < 5 {
            return ParseSslPreread::NeedMore;
        }
        if buffer[offset] != 0x16 || buffer[offset + 1] != 3 {
            return ParseSslPreread::Complete(None);
        }
        let record_len = u16::from_be_bytes([buffer[offset + 3], buffer[offset + 4]]) as usize;
        let record_end = offset + 5 + record_len;
        if buffer.len() < record_end {
            return ParseSslPreread::NeedMore;
        }
        handshake.extend_from_slice(&buffer[offset + 5..record_end]);
        offset = record_end;

        if handshake.len() < 4 {
            if offset == buffer.len() {
                return ParseSslPreread::NeedMore;
            }
            continue;
        }
        if handshake[0] != 1 {
            return ParseSslPreread::Complete(None);
        }
        let body_len = ((handshake[1] as usize) << 16)
            | ((handshake[2] as usize) << 8)
            | handshake[3] as usize;
        let handshake_len = 4 + body_len;
        if handshake.len() < handshake_len {
            if offset == buffer.len() {
                return ParseSslPreread::NeedMore;
            }
            continue;
        }
        return ParseSslPreread::Complete(parse_client_hello(&handshake[4..handshake_len]));
    }
}
