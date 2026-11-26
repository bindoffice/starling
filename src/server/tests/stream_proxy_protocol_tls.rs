use crate::config::StreamProxyProtocol;
use crate::server::stream::{
    StreamProxyProtocolTransport, build_stream_proxy_protocol_header_with_tls,
};
use crate::server::tls::ClientCertificateInfo;

use super::*;

#[test]
fn emits_tls_authority_alpn_and_ssl_tlvs() {
    let source = ListenAddress::Inet("203.0.113.20:45678".parse().unwrap());
    let destination = ListenAddress::Inet("127.0.0.1:13340".parse().unwrap());
    let tls = TlsConnectionInfo {
        protocol: "TLSv1.3".into(),
        cipher: "TLS_AES_128_GCM_SHA256".into(),
        server_name: "api.example.test".into(),
        alpn_protocol: "h2".into(),
        client_certificate: None,
        client_certificate_verified: false,
        client_certificate_error: None,
        session_reused: false,
    };
    let header = build_stream_proxy_protocol_header_with_tls(
        StreamProxyProtocol::V2,
        &source,
        &destination,
        StreamProxyProtocolTransport::Stream,
        Some(&tls),
    );

    assert_eq!(header[12], 0x21);
    assert_eq!(header[13], 0x11);
    let payload_length = u16::from_be_bytes([header[14], header[15]]) as usize;
    assert_eq!(payload_length, header.len() - 16);
    let tlvs = proxy_protocol_tlvs(&header);
    assert_eq!(find_tlv(tlvs, 0x02), Some(b"api.example.test".as_slice()));
    assert_eq!(find_tlv(tlvs, 0x01), Some(b"h2".as_slice()));

    let ssl = find_tlv(tlvs, 0x20).expect("SSL TLV");
    assert_eq!(&ssl[..5], &[0x01, 0, 0, 0, 1]);
    let ssl_tlvs = &ssl[5..];
    assert_eq!(find_tlv(ssl_tlvs, 0x21), Some(b"TLSv1.3".as_slice()));
    assert_eq!(
        find_tlv(ssl_tlvs, 0x23),
        Some(b"TLS_AES_128_GCM_SHA256".as_slice())
    );
    assert_crc32c(&header);
}

#[test]
fn emits_client_certificate_flags_and_verification_status() {
    let source = ListenAddress::Inet("203.0.113.20:45678".parse().unwrap());
    let destination = ListenAddress::Inet("127.0.0.1:13340".parse().unwrap());
    let mut tls = TlsConnectionInfo {
        protocol: "TLSv1.2".into(),
        cipher: "ECDHE-RSA-AES128-GCM-SHA256".into(),
        server_name: String::new(),
        alpn_protocol: String::new(),
        client_certificate: Some(client_certificate()),
        client_certificate_verified: true,
        client_certificate_error: None,
        session_reused: false,
    };

    let fresh = build_stream_proxy_protocol_header_with_tls(
        StreamProxyProtocol::V2,
        &source,
        &destination,
        StreamProxyProtocolTransport::Stream,
        Some(&tls),
    );
    let ssl = find_tlv(proxy_protocol_tlvs(&fresh), 0x20).expect("SSL TLV");
    assert_eq!(&ssl[..5], &[0x07, 0, 0, 0, 0]);

    tls.session_reused = true;
    tls.client_certificate_verified = false;
    let resumed = build_stream_proxy_protocol_header_with_tls(
        StreamProxyProtocol::V2,
        &source,
        &destination,
        StreamProxyProtocolTransport::Stream,
        Some(&tls),
    );
    let ssl = find_tlv(proxy_protocol_tlvs(&resumed), 0x20).expect("SSL TLV");
    assert_eq!(&ssl[..5], &[0x05, 0, 0, 0, 1]);
}

fn client_certificate() -> ClientCertificateInfo {
    ClientCertificateInfo {
        certificate: String::new(),
        escaped_certificate: String::new(),
        fingerprint: String::new(),
        serial: String::new(),
        subject_dn: String::new(),
        issuer_dn: String::new(),
        subject_dn_legacy: String::new(),
        issuer_dn_legacy: String::new(),
        valid_start: String::new(),
        valid_end: String::new(),
        valid_remain: String::new(),
    }
}

fn proxy_protocol_tlvs(header: &[u8]) -> &[u8] {
    let payload_length = u16::from_be_bytes([header[14], header[15]]) as usize;
    let payload = &header[16..16 + payload_length];
    let address_length = match header[13] >> 4 {
        0x01 => 12,
        0x02 => 36,
        _ => 0,
    };
    &payload[address_length..]
}

fn find_tlv(mut tlvs: &[u8], kind: u8) -> Option<&[u8]> {
    while tlvs.len() >= 3 {
        let length = u16::from_be_bytes([tlvs[1], tlvs[2]]) as usize;
        let value = &tlvs[3..3 + length];
        if tlvs[0] == kind {
            return Some(value);
        }
        tlvs = &tlvs[3 + length..];
    }
    None
}

fn assert_crc32c(header: &[u8]) {
    let advertised = u32::from_be_bytes(header[header.len() - 4..].try_into().unwrap());
    let mut zeroed = header.to_vec();
    zeroed[header.len() - 4..].fill(0);
    assert_eq!(advertised, test_crc32c(&zeroed));
}

fn test_crc32c(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0x82f6_3b78 & mask);
        }
    }
    !crc
}
