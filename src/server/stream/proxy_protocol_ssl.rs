use super::*;

const TLV_ALPN: u8 = 0x01;
const TLV_AUTHORITY: u8 = 0x02;
const TLV_SSL: u8 = 0x20;
const TLV_SSL_VERSION: u8 = 0x21;
const TLV_SSL_CIPHER: u8 = 0x23;

const CLIENT_SSL: u8 = 0x01;
const CLIENT_CERT_CONN: u8 = 0x02;
const CLIENT_CERT_SESS: u8 = 0x04;

pub(in crate::server) fn append_stream_proxy_protocol_v2_tls(
    tlvs: &mut Vec<u8>,
    tls: Option<&TlsConnectionInfo>,
) {
    let Some(tls) = tls else {
        return;
    };
    if !tls.server_name.is_empty() {
        push_tlv(tlvs, TLV_AUTHORITY, tls.server_name.as_bytes());
    }
    if !tls.alpn_protocol.is_empty() {
        push_tlv(tlvs, TLV_ALPN, tls.alpn_protocol.as_bytes());
    }

    let mut ssl = Vec::new();
    let mut client = CLIENT_SSL;
    if tls.client_certificate.is_some() {
        client |= CLIENT_CERT_SESS;
        if !tls.session_reused {
            client |= CLIENT_CERT_CONN;
        }
    }
    ssl.push(client);
    let verify = if tls.client_certificate_verified {
        0_u32
    } else {
        1_u32
    };
    ssl.extend_from_slice(&verify.to_be_bytes());
    if !tls.protocol.is_empty() {
        push_tlv(&mut ssl, TLV_SSL_VERSION, tls.protocol.as_bytes());
    }
    if !tls.cipher.is_empty() {
        push_tlv(&mut ssl, TLV_SSL_CIPHER, tls.cipher.as_bytes());
    }
    push_tlv(tlvs, TLV_SSL, &ssl);
}

fn push_tlv(tlvs: &mut Vec<u8>, kind: u8, value: &[u8]) {
    let Ok(length) = u16::try_from(value.len()) else {
        return;
    };
    tlvs.push(kind);
    tlvs.extend_from_slice(&length.to_be_bytes());
    tlvs.extend_from_slice(value);
}
