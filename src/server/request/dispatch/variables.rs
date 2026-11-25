use super::*;

pub(super) fn apply_connection_variables(
    variables: &mut Variables,
    connection: &ConnectionAddresses,
) {
    if let Some(tls) = &connection.tls {
        variables.ssl_protocol = tls.protocol.clone();
        variables.ssl_cipher = tls.cipher.clone();
        variables.ssl_server_name = tls.server_name.clone();
        variables.ssl_alpn_protocol = tls.alpn_protocol.clone();
        variables.ssl_client_verify = if tls.client_certificate.is_some() {
            "SUCCESS".into()
        } else {
            "NONE".into()
        };
        if let Some(certificate) = &tls.client_certificate {
            variables.ssl_client_cert = certificate.certificate.clone();
            variables.ssl_client_escaped_cert = certificate.escaped_certificate.clone();
            variables.ssl_client_fingerprint = certificate.fingerprint.clone();
            variables.ssl_client_serial = certificate.serial.clone();
            variables.ssl_client_s_dn = certificate.subject_dn.clone();
            variables.ssl_client_i_dn = certificate.issuer_dn.clone();
            variables.ssl_client_s_dn_legacy = certificate.subject_dn_legacy.clone();
            variables.ssl_client_i_dn_legacy = certificate.issuer_dn_legacy.clone();
            variables.ssl_client_v_start = certificate.valid_start.clone();
            variables.ssl_client_v_end = certificate.valid_end.clone();
            variables.ssl_client_v_remain = certificate.valid_remain.clone();
        }
    }
    variables.realip_remote_addr = Some(connection.remote_addr_text.clone());
    variables.realip_remote_port = Some(connection.remote_port_text.clone());
    variables.proxy_protocol_port = connection
        .proxy_protocol_port
        .map(|port| port.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_server_addr = connection
        .proxy_protocol_server_addr
        .map(|address| address.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_server_port = connection
        .proxy_protocol_server_port
        .map(|port| port.to_string())
        .unwrap_or_default();
    variables.proxy_protocol_tlvs = connection.proxy_protocol_tlvs.clone();
    variables.proxy_protocol_addr = connection
        .proxy_protocol_addr
        .map(|address| address.to_string())
        .unwrap_or_default();
}
