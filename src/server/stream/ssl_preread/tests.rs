use super::record::{ParseSslPreread, parse_ssl_preread};

fn client_hello(protocol: [u8; 2], host: &str, alpn: &[&str], tls13: bool) -> Vec<u8> {
    let mut extensions = Vec::new();
    if !host.is_empty() {
        let mut sni = Vec::new();
        sni.extend_from_slice(&(host.len() as u16 + 3).to_be_bytes());
        sni.push(0);
        sni.extend_from_slice(&(host.len() as u16).to_be_bytes());
        sni.extend_from_slice(host.as_bytes());
        extensions.extend_from_slice(&0_u16.to_be_bytes());
        extensions.extend_from_slice(&(sni.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&sni);
    }
    if !alpn.is_empty() {
        let mut protocols = Vec::new();
        for protocol in alpn {
            protocols.push(protocol.len() as u8);
            protocols.extend_from_slice(protocol.as_bytes());
        }
        let mut data = Vec::new();
        data.extend_from_slice(&(protocols.len() as u16).to_be_bytes());
        data.extend_from_slice(&protocols);
        extensions.extend_from_slice(&16_u16.to_be_bytes());
        extensions.extend_from_slice(&(data.len() as u16).to_be_bytes());
        extensions.extend_from_slice(&data);
    }
    if tls13 {
        extensions.extend_from_slice(&43_u16.to_be_bytes());
        extensions.extend_from_slice(&3_u16.to_be_bytes());
        extensions.extend_from_slice(&[2, 3, 4]);
    }

    let mut body = Vec::new();
    body.extend_from_slice(&protocol);
    body.extend_from_slice(&[0_u8; 32]);
    body.push(0);
    body.extend_from_slice(&[0, 2, 0x13, 0x01]);
    body.extend_from_slice(&[1, 0]);
    body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    body.extend_from_slice(&extensions);

    let mut handshake = Vec::new();
    handshake.push(1);
    let length = body.len();
    handshake.extend_from_slice(&[(length >> 16) as u8, (length >> 8) as u8, length as u8]);
    handshake.extend_from_slice(&body);

    let mut record = Vec::new();
    record.extend_from_slice(&[0x16, 3, 1]);
    record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
    record.extend_from_slice(&handshake);
    record
}

#[test]
fn parses_tls12_sni_and_alpn() {
    let hello = client_hello([3, 3], "Example.Test", &["h2", "http/1.1"], false);
    let ParseSslPreread::Complete(Some(context)) = parse_ssl_preread(&hello) else {
        panic!("ClientHello should be parsed");
    };
    assert_eq!(context.protocol, "TLSv1.2");
    assert_eq!(context.server_name, "example.test");
    assert_eq!(context.alpn_protocols, "h2,http/1.1");
}

#[test]
fn parses_tls13_supported_versions() {
    let hello = client_hello([3, 3], "example.test", &[], true);
    let ParseSslPreread::Complete(Some(context)) = parse_ssl_preread(&hello) else {
        panic!("ClientHello should be parsed");
    };
    assert_eq!(context.protocol, "TLSv1.3");
}

#[test]
fn declines_non_tls_and_waits_for_incomplete_records() {
    assert!(matches!(
        parse_ssl_preread(b"hello"),
        ParseSslPreread::Complete(None)
    ));
    let hello = client_hello([3, 3], "example.test", &[], false);
    assert!(matches!(
        parse_ssl_preread(&hello[..10]),
        ParseSslPreread::NeedMore
    ));
}
