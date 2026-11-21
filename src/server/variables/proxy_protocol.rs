pub(in crate::server) fn proxy_protocol_tlv_value(name: &str, tlvs: Option<&[u8]>) -> String {
    const PREFIX: &str = "proxy_protocol_tlv_";
    let Some(tlvs) = tlvs else {
        return String::new();
    };
    let name = &name[PREFIX.len()..];
    if let Some(name) = name.strip_prefix("ssl_") {
        let Some(ssl) = find_proxy_protocol_tlv(tlvs, 0x20) else {
            return String::new();
        };
        if ssl.len() < 5 {
            return String::new();
        }
        if name == "verify" {
            return u32::from_be_bytes([ssl[1], ssl[2], ssl[3], ssl[4]]).to_string();
        }
        let kind = match name {
            "version" => 0x21,
            "cn" => 0x22,
            "cipher" => 0x23,
            "sig_alg" => 0x24,
            "key_alg" => 0x25,
            _ => return String::new(),
        };
        return find_proxy_protocol_tlv(&ssl[5..], kind)
            .map(String::from_utf8_lossy)
            .map(|value| value.into_owned())
            .unwrap_or_default();
    }
    let kind = match name {
        "alpn" => 0x01,
        "authority" => 0x02,
        "unique_id" => 0x05,
        "ssl" => 0x20,
        "netns" => 0x30,
        _ if name.starts_with("0x") => {
            let Ok(kind) = u8::from_str_radix(&name[2..], 16) else {
                return String::new();
            };
            kind
        }
        _ => return String::new(),
    };
    find_proxy_protocol_tlv(tlvs, kind)
        .map(String::from_utf8_lossy)
        .map(|value| value.into_owned())
        .unwrap_or_default()
}

pub(in crate::server) fn find_proxy_protocol_tlv(mut tlvs: &[u8], kind: u8) -> Option<&[u8]> {
    while !tlvs.is_empty() {
        if tlvs.len() < 3 {
            return None;
        }
        let length = u16::from_be_bytes([tlvs[1], tlvs[2]]) as usize;
        let value = tlvs.get(3..3 + length)?;
        if tlvs[0] == kind {
            return Some(value);
        }
        tlvs = &tlvs[3 + length..];
    }
    None
}
