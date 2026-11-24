use super::*;
use crate::config::{Captures, Regex};

#[derive(Default)]
pub(in crate::server) struct StreamCaptures {
    values: Vec<String>,
    named: Vec<(String, String)>,
}

impl StreamCaptures {
    pub(super) fn from_regex(regex: &Regex, captures: &Captures<'_>) -> Self {
        let values = captures
            .iter()
            .map(|capture| capture.map_or_else(String::new, |capture| capture.as_str().to_string()))
            .collect();
        let named = regex
            .capture_names()
            .flatten()
            .filter_map(|name| {
                captures
                    .name(name)
                    .map(|value| (name.to_string(), value.as_str().to_string()))
            })
            .collect();
        Self { values, named }
    }
}

pub(in crate::server) fn apply_stream_sets(sets: &[SetVariable], log: &mut StreamLogContext) {
    for set in sets {
        let value = eval_stream_log_variables(&set.value, log);
        log.variables.insert(set.name.clone(), value);
    }
}

pub(in crate::server) fn eval_stream_log_variables(
    template: &str,
    log: &StreamLogContext,
) -> String {
    let output = eval_stream_template(
        template,
        log,
        &mut Vec::new(),
        &StreamCaptures::default(),
        false,
    );
    String::from_utf8(output)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

pub(in crate::server) fn eval_stream_log_variables_bytes(
    template: &str,
    log: &StreamLogContext,
) -> Vec<u8> {
    eval_stream_template(
        template,
        log,
        &mut Vec::new(),
        &StreamCaptures::default(),
        true,
    )
}

pub(in crate::server) fn eval_stream_variables_with_stack(
    template: &str,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
) -> String {
    let output = eval_stream_template(template, log, stack, captures, false);
    String::from_utf8(output)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

pub(in crate::server) fn eval_stream_variable(
    name: &str,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
) -> String {
    stream_variable_value(name, log, stack, &StreamCaptures::default())
}

fn eval_stream_template(
    template: &str,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
    binary: bool,
) -> Vec<u8> {
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
        if binary {
            if name == "binary_remote_addr" {
                output.extend_from_slice(&binary_address_bytes(&log.remote.variable_address()));
            } else {
                output.extend_from_slice(
                    stream_variable_value(name, log, stack, captures).as_bytes(),
                );
            }
        } else {
            output.extend_from_slice(stream_variable_value(name, log, stack, captures).as_bytes());
        }
    }
    output
}

fn stream_variable_value(
    name: &str,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
) -> String {
    match name {
        "remote_addr" => log.remote.variable_address(),
        "binary_remote_addr" => log.remote.variable_address(),
        "remote_port" => log.remote.variable_port(),
        "realip_remote_addr" => log.realip_remote.variable_address(),
        "realip_remote_port" => log.realip_remote.variable_port(),
        "server_addr" => log.local.variable_address(),
        "server_port" => log.local.variable_port(),
        "server_name" => log.server_name.clone(),
        "limit_conn_status" => log.limit_conn_status.clone(),
        "protocol" => log.protocol.into(),
        "status" => format!("{:03}", log.status),
        "bytes_sent" => log.bytes_sent.to_string(),
        "bytes_received" => log.bytes_received.to_string(),
        "session_time" => format_stream_duration(Some(log.started.elapsed())),
        "connection" => log.remote.variable_port(),
        "upstream_addr" => log.upstream_addr.clone(),
        "upstream_bytes_sent" => log.upstream_bytes_sent.to_string(),
        "upstream_bytes_received" => log.upstream_bytes_received.to_string(),
        "upstream_connect_time" => format_stream_duration(log.upstream_connect_time),
        "upstream_first_byte_time" => format_stream_duration(log.upstream_first_byte_time),
        "upstream_session_time" => format_stream_duration(log.upstream_session_time),
        "proxy_protocol_addr" => log
            .proxy_protocol_addr
            .map(|address| address.to_string())
            .unwrap_or_default(),
        "proxy_protocol_port" => log
            .proxy_protocol_port
            .map(|port| port.to_string())
            .unwrap_or_default(),
        "proxy_protocol_server_addr" => log
            .proxy_protocol_server_addr
            .map(|address| address.to_string())
            .unwrap_or_default(),
        "proxy_protocol_server_port" => log
            .proxy_protocol_server_port
            .map(|port| port.to_string())
            .unwrap_or_default(),
        "ssl_preread_protocol" => log
            .ssl_preread
            .as_ref()
            .map(|preread| preread.protocol.clone())
            .unwrap_or_default(),
        "ssl_preread_server_name" => log
            .ssl_preread
            .as_ref()
            .map(|preread| preread.server_name.clone())
            .unwrap_or_default(),
        "ssl_preread_alpn_protocols" => log
            .ssl_preread
            .as_ref()
            .map(|preread| preread.alpn_protocols.clone())
            .unwrap_or_default(),
        name if name.starts_with("proxy_protocol_tlv_") => {
            proxy_protocol_tlv_value(name, log.proxy_protocol_tlvs.as_deref())
        }
        "ssl_protocol" => log
            .tls
            .as_ref()
            .map(|tls| tls.protocol.clone())
            .unwrap_or_default(),
        "ssl_cipher" => log
            .tls
            .as_ref()
            .map(|tls| tls.cipher.clone())
            .unwrap_or_default(),
        "ssl_server_name" => log
            .tls
            .as_ref()
            .map(|tls| tls.server_name.clone())
            .unwrap_or_default(),
        "ssl_alpn_protocol" => log
            .tls
            .as_ref()
            .map(|tls| tls.alpn_protocol.clone())
            .unwrap_or_default(),
        "ssl_client_verify" => log
            .tls
            .as_ref()
            .map(|tls| {
                if tls.client_certificate.is_some() {
                    "SUCCESS"
                } else {
                    "NONE"
                }
            })
            .unwrap_or_default()
            .into(),
        name if name.starts_with("ssl_client_") => log
            .tls
            .as_ref()
            .and_then(|tls| tls.client_certificate.as_ref())
            .map(|certificate| match name {
                "ssl_client_cert" => certificate.certificate.clone(),
                "ssl_client_escaped_cert" => certificate.escaped_certificate.clone(),
                "ssl_client_fingerprint" => certificate.fingerprint.clone(),
                "ssl_client_serial" => certificate.serial.clone(),
                "ssl_client_s_dn" => certificate.subject_dn.clone(),
                "ssl_client_i_dn" => certificate.issuer_dn.clone(),
                "ssl_client_s_dn_legacy" => certificate.subject_dn_legacy.clone(),
                "ssl_client_i_dn_legacy" => certificate.issuer_dn_legacy.clone(),
                "ssl_client_v_start" => certificate.valid_start.clone(),
                "ssl_client_v_end" => certificate.valid_end.clone(),
                "ssl_client_v_remain" => certificate.valid_remain.clone(),
                _ => String::new(),
            })
            .unwrap_or_default(),
        "hostname" => hostname(),
        "nginx_version" => "1.31.9".into(),
        "pid" => std::process::id().to_string(),
        "msec" => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            format!("{}.{:03}", now.as_secs(), now.subsec_millis())
        }
        "time_local" => nginx_access_timestamp(SystemTime::now()),
        "time_iso8601" => iso_timestamp(SystemTime::now()),
        name if name.len() == 1 && name.as_bytes()[0].is_ascii_digit() => captures
            .values
            .get((name.as_bytes()[0] - b'0') as usize)
            .cloned()
            .unwrap_or_default(),
        name if let Some((_, value)) = captures.named.iter().find(|(key, _)| key == name) => {
            value.clone()
        }
        name => {
            if let Some(value) = log.variables.get(name) {
                return value.clone();
            }
            if let Some(map) = log.maps.get(name) {
                return stream_map_value(map, log, stack, captures);
            }
            if let Some(geo) = log.geos.get(name) {
                return stream_geo_value(geo, log, stack, captures);
            }
            if let Some(split) = log.split_clients.get(name) {
                return stream_split_clients_value(split, log, stack, captures);
            }
            String::new()
        }
    }
}
