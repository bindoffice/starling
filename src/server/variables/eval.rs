use super::*;

pub(in crate::server) use bytes::binary_address_bytes;

pub(in crate::server) fn eval_variables(template: &str, variables: &Variables) -> String {
    eval_variables_with_stack(template, variables, &mut Vec::new())
}

pub(in crate::server) fn eval_variables_with_stack(
    template: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> String {
    let output = eval_template(template, variables, stack, false);
    String::from_utf8(output)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

pub(in crate::server) fn eval_variables_bytes(template: &str, variables: &Variables) -> Vec<u8> {
    eval_template(template, variables, &mut Vec::new(), true)
}

fn eval_template(
    template: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
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
            bytes::append_variable_bytes(name, variables, stack, &mut output);
        } else {
            output.extend_from_slice(variable_value(name, variables, stack).as_bytes());
        }
    }
    output
}

pub(in crate::server) fn variable_value(
    name: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> String {
    match name {
        "uri" | "document_uri" => variables.uri.clone(),
        "fastcgi_script_name" => variables.fastcgi_script_name.clone(),
        "fastcgi_path_info" => variables.fastcgi_path_info.clone(),
        "uwsgi_script_name" => variables.uwsgi_script_name.clone(),
        "scgi_script_name" => variables.scgi_script_name.clone(),
        "request_uri" => variables.request_target.clone(),
        "args" | "query_string" => variables.args.clone(),
        "is_args" => {
            if variables.args.is_empty() {
                String::new()
            } else {
                "?".into()
            }
        }
        "request_method" => variables.method.clone(),
        "server_protocol" => variables.version.clone(),
        "request" => format!(
            "{} {} {}",
            variables.method, variables.request_target, variables.version
        ),
        "host" | "http_host" => variables.host.clone(),
        "server_name" => variables.server_name.clone(),
        "remote_addr" => variables.remote_addr.clone(),
        "binary_remote_addr" => variables.remote_addr.clone(),
        "binary_remote_port" => variables.remote_port.clone(),
        "remote_port" => variables.remote_port.clone(),
        "server_addr" => variables.server_addr.clone(),
        "server_port" => variables.server_port.clone(),
        "request_port" => variables.request_port.clone(),
        "is_request_port" => variables.is_request_port.clone(),
        "scheme" => variables.scheme.clone(),
        "https" => variables.https.clone(),
        "http2" => variables.http2.clone(),
        "pipe" => variables.pipe.clone(),
        "connection" => variables.connection.clone(),
        "connection_requests" => variables.connection_requests.clone(),
        "connection_time" => variables.connection_time.clone(),
        "ssl_protocol" => variables.ssl_protocol.clone(),
        "ssl_cipher" => variables.ssl_cipher.clone(),
        "ssl_server_name" => variables.ssl_server_name.clone(),
        "ssl_alpn_protocol" => variables.ssl_alpn_protocol.clone(),
        "ssl_client_verify" => variables.ssl_client_verify.clone(),
        "ssl_client_cert" => variables.ssl_client_cert.clone(),
        "ssl_client_escaped_cert" => variables.ssl_client_escaped_cert.clone(),
        "ssl_client_fingerprint" => variables.ssl_client_fingerprint.clone(),
        "ssl_client_serial" => variables.ssl_client_serial.clone(),
        "ssl_client_s_dn" => variables.ssl_client_s_dn.clone(),
        "ssl_client_i_dn" => variables.ssl_client_i_dn.clone(),
        "ssl_client_s_dn_legacy" => variables.ssl_client_s_dn_legacy.clone(),
        "ssl_client_i_dn_legacy" => variables.ssl_client_i_dn_legacy.clone(),
        "ssl_client_v_start" => variables.ssl_client_v_start.clone(),
        "ssl_client_v_end" => variables.ssl_client_v_end.clone(),
        "ssl_client_v_remain" => variables.ssl_client_v_remain.clone(),
        "hostname" => variables.hostname.to_string(),
        "request_id" => variables.request_id.clone(),
        "realip_remote_addr" => variables.realip_remote_addr.clone(),
        "realip_remote_port" => variables.realip_remote_port.clone(),
        "proxy_protocol_addr" => variables.proxy_protocol_addr.clone(),
        "proxy_protocol_port" => variables.proxy_protocol_port.clone(),
        "proxy_protocol_server_addr" => variables.proxy_protocol_server_addr.clone(),
        "proxy_protocol_server_port" => variables.proxy_protocol_server_port.clone(),
        name if name.starts_with("proxy_protocol_tlv_") => {
            proxy_protocol_tlv_value(name, variables.proxy_protocol_tlvs.as_deref())
        }
        "status" => variables.status.to_string(),
        "bytes_sent" => variables.bytes_sent.to_string(),
        "body_bytes_sent" => variables.body_bytes_sent.to_string(),
        "content_length" => variables.content_length.clone(),
        "content_type" => variables.content_type.clone(),
        "request_body" => variables.request_body.clone(),
        "request_body_file" => variables.request_body_file.clone(),
        "request_length" => variables.request_length.clone(),
        "request_time" => format!("{:.3}", variables.started.elapsed().as_secs_f64()),
        "date_local" => crate::server::response::default_date(false),
        "date_gmt" => crate::server::response::default_date(true),
        "upstream_status" => variables.upstream_status.clone(),
        "upstream_cache_status" => variables.upstream_cache_status.clone(),
        "upstream_cache_last_modified" => variables.upstream_cache_last_modified.clone(),
        "upstream_cache_etag" => variables.upstream_cache_etag.clone(),
        "upstream_response_time" => variables.upstream_response_time.clone(),
        "limit_req_status" => variables.limit_req_status.clone(),
        "limit_conn_status" => variables.limit_conn_status.clone(),
        "invalid_referer" => variables.invalid_referer.clone(),
        "secure_link" => variables.secure_link.clone(),
        "secure_link_expires" => variables.secure_link_expires.clone(),
        "msie" => variables.msie.clone(),
        "modern_browser" => variables.modern_browser.clone(),
        "ancient_browser" => variables.ancient_browser.clone(),
        "uid_got" => userid_variable(&variables.userid, variables.userid.got, false),
        "uid_set" => userid_variable(&variables.userid, variables.userid.set, true),
        "uid_reset" => variables
            .custom
            .get("uid_reset")
            .cloned()
            .unwrap_or_default(),
        "connections_active" => variables.stats.active.load(Ordering::Relaxed).to_string(),
        "connections_reading" => variables.stats.reading.load(Ordering::Relaxed).to_string(),
        "connections_writing" => variables.stats.writing.load(Ordering::Relaxed).to_string(),
        "connections_waiting" => variables.stats.waiting.load(Ordering::Relaxed).to_string(),
        "document_root" | "realpath_root" => variables.document_root.display().to_string(),
        "request_filename" => variables.request_filename.display().to_string(),
        "proxy_host" => variables.proxy_host.clone(),
        "proxy_port" => variables.proxy_port.clone(),
        "proxy_add_x_forwarded_for" => {
            let forwarded = variables
                .headers
                .iter()
                .find(|header| header.name.eq_ignore_ascii_case("x-forwarded-for"))
                .map(|header| header.value.as_str())
                .unwrap_or_default();
            if forwarded.is_empty() {
                variables.remote_addr.clone()
            } else {
                format!("{forwarded}, {}", variables.remote_addr)
            }
        }
        "nginx_version" => "1.31.9".into(),
        "pid" => std::process::id().to_string(),
        "upstream_addr" => variables.upstream_addr.clone(),
        "remote_user" => variables.remote_user.clone(),
        "time_local" => nginx_access_timestamp(SystemTime::now()),
        "time_iso8601" => iso_timestamp(SystemTime::now()),
        "msec" => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default();
            format!("{}.{:03}", now.as_secs(), now.subsec_millis())
        }
        name if name.len() == 1 && name.as_bytes()[0].is_ascii_digit() => {
            let index = name.as_bytes()[0] - b'0';
            variables
                .captures
                .get(index as usize)
                .cloned()
                .unwrap_or_default()
        }
        "limit_rate" => variables
            .custom
            .get(name)
            .and_then(|value| parse_size_value(value))
            .unwrap_or(0)
            .to_string(),
        name if name.starts_with("http_") => {
            if let Some(value) = variables.custom.get(name) {
                return value.clone();
            }
            let header_name = name["http_".len()..].replace('_', "-");
            combined_request_header(
                &variables.headers,
                &header_name,
                header_name.eq_ignore_ascii_case("cookie"),
            )
        }
        name if name.starts_with("upstream_http_") => {
            response_header_value(&variables.upstream_headers, &name["upstream_http_".len()..])
        }
        name if name.starts_with("upstream_cookie_") => {
            let cookie_name = &name["upstream_cookie_".len()..];
            upstream_cookie_value(&variables.upstream_headers, cookie_name)
        }
        name if name.starts_with("upstream_trailer_") => response_header_value(
            &variables.upstream_trailers,
            &name["upstream_trailer_".len()..],
        ),
        "sent_http_connection" => variables.sent_connection.clone(),
        "sent_http_keep_alive" => variables.sent_keep_alive.clone(),
        "sent_http_transfer_encoding" => variables.sent_transfer_encoding.clone(),
        name if name.starts_with("sent_http_") => {
            response_header_value(&variables.sent_headers, &name["sent_http_".len()..])
        }
        name if name.starts_with("sent_trailer_") => {
            response_header_value(&variables.sent_trailers, &name["sent_trailer_".len()..])
        }
        name if name.starts_with("arg_") => {
            let argument_name = &name["arg_".len()..];
            variables
                .args
                .split('&')
                .find_map(|argument| {
                    let (name, value) = argument.split_once('=')?;
                    name.eq_ignore_ascii_case(argument_name)
                        .then(|| value.to_string())
                })
                .unwrap_or_default()
        }
        name if name.starts_with("cookie_") => {
            let cookie_name = &name["cookie_".len()..];
            variables
                .headers
                .iter()
                .filter(|header| header.name.eq_ignore_ascii_case("cookie"))
                .find_map(|header| cookie_value_from_header(&header.value, cookie_name))
                .unwrap_or_default()
        }
        _ => {
            if let Some(value) = variables.custom.get(name) {
                return value.clone();
            }
            if let Some(map) = variables.maps.get(name) {
                return map_value(map, variables, stack);
            }
            if let Some(geo) = variables.geos.get(name) {
                return geo_value(geo, variables, stack);
            }
            if let Some(split) = variables.split_clients.get(name) {
                return split_clients_value(split, variables, stack);
            }
            if variables.uninitialized_variable_warn
                && let Some(logger) = &variables.rewrite_logger
            {
                logger.log(
                    LogLevel::Warn,
                    format!("using uninitialized \"{name}\" variable"),
                );
            }
            String::new()
        }
    }
}

pub(in crate::server) fn variable_value_for_log(
    name: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> Option<String> {
    if log_variable_missing(name, variables) {
        None
    } else if name == "remote_user" {
        remote_user_from_request(variables)
    } else {
        Some(variable_value(name, variables, stack))
    }
}

pub(in crate::server) fn variable_bytes_for_log(
    name: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
) -> Option<Vec<u8>> {
    if name == "binary_remote_addr" {
        return Some(binary_address_bytes(&variables.remote_addr));
    }
    variable_value_for_log(name, variables, stack)
        .map(|value| crate::http::header_value_bytes(&value).into_owned())
}

fn log_variable_missing(name: &str, variables: &Variables) -> bool {
    if variables.custom.contains_key(name) {
        return false;
    }
    if name == "remote_user" {
        return remote_user_from_request(variables).is_none();
    }
    if name == "http_host" {
        return !has_request_header(&variables.headers, "host");
    }
    if let Some(header_name) = name.strip_prefix("http_") {
        let header_name = header_name.replace('_', "-");
        return !has_request_header(&variables.headers, &header_name);
    }
    if name == "sent_http_connection" {
        return variables.sent_connection.is_empty();
    }
    if name == "sent_http_keep_alive" {
        return variables.sent_keep_alive.is_empty();
    }
    if name == "sent_http_transfer_encoding" {
        return variables.sent_transfer_encoding.is_empty();
    }
    if name == "limit_req_status" {
        return variables.limit_req_status.is_empty();
    }
    if name == "limit_conn_status" {
        return variables.limit_conn_status.is_empty();
    }
    for (prefix, headers) in [
        ("sent_http_", variables.sent_headers.as_slice()),
        ("sent_trailer_", variables.sent_trailers.as_slice()),
        ("upstream_http_", variables.upstream_headers.as_slice()),
        ("upstream_trailer_", variables.upstream_trailers.as_slice()),
    ] {
        if let Some(header_name) = name.strip_prefix(prefix) {
            return !has_response_header(headers, &header_name.replace('_', "-"));
        }
    }
    if let Some(cookie_name) = name.strip_prefix("cookie_") {
        return !variables
            .headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("cookie"))
            .any(|header| cookie_value_from_header(&header.value, cookie_name).is_some());
    }
    if let Some(cookie_name) = name.strip_prefix("upstream_cookie_") {
        return !variables
            .upstream_headers
            .iter()
            .filter(|header| header.name.eq_ignore_ascii_case("set-cookie"))
            .any(|header| cookie_value_from_header(&header.value, cookie_name).is_some());
    }
    if let Some(argument_name) = name.strip_prefix("arg_") {
        return !variables.args.split('&').any(|argument| {
            argument
                .split_once('=')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case(argument_name))
        });
    }
    false
}

fn remote_user_from_request(variables: &Variables) -> Option<String> {
    variables
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("authorization"))
        .and_then(|header| parse_authorization_header(&header.value))
        .map(|credentials| credentials.username)
}

fn has_request_header(headers: &[Header], name: &str) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case(name)
            || header.name.eq_ignore_ascii_case(&name.replace('-', "_"))
    })
}

fn has_response_header(headers: &[Header], name: &str) -> bool {
    headers.iter().any(|header| {
        header.name.eq_ignore_ascii_case(name)
            || header.name.eq_ignore_ascii_case(&name.replace('-', "_"))
    })
}

fn combined_request_header(headers: &[Header], name: &str, semicolon: bool) -> String {
    let mut values = headers
        .iter()
        .filter(|header| {
            header.name.eq_ignore_ascii_case(name)
                || header.name.eq_ignore_ascii_case(&name.replace('-', "_"))
        })
        .map(|header| header.value.as_str());
    let Some(first) = values.next() else {
        return String::new();
    };
    let separator = if semicolon { "; " } else { ", " };
    let mut value = first.to_string();
    for next in values {
        value.push_str(separator);
        value.push_str(next);
    }
    value
}

fn cookie_value_from_header(value: &str, cookie_name: &str) -> Option<String> {
    value.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        name.eq_ignore_ascii_case(cookie_name)
            .then(|| value.to_string())
    })
}

fn upstream_cookie_value(headers: &[Header], cookie_name: &str) -> String {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("set-cookie"))
        .find_map(|header| cookie_value_from_header(&header.value, cookie_name))
        .unwrap_or_default()
}

fn userid_variable(state: &UserIdState, uid: [u32; 4], set: bool) -> String {
    let visible = if set {
        state.enable == UserIdEnable::V1 || state.enable == UserIdEnable::On
    } else {
        state.enable != UserIdEnable::Off
    };
    if !visible || uid[3] == 0 {
        return String::new();
    }
    format!(
        "{}={:08X}{:08X}{:08X}{:08X}",
        state.name, uid[0], uid[1], uid[2], uid[3]
    )
}

mod bytes;
