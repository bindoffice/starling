use std::collections::HashSet;

use super::*;

const BUILTIN_VARIABLES: &[&str] = &[
    "ancient_browser",
    "arg_",
    "args",
    "binary_remote_addr",
    "binary_remote_port",
    "body_bytes_sent",
    "bytes_sent",
    "connection",
    "connection_requests",
    "connection_time",
    "connections_active",
    "connections_reading",
    "connections_waiting",
    "connections_writing",
    "content_length",
    "content_type",
    "cookie_",
    "date_gmt",
    "date_local",
    "document_root",
    "document_uri",
    "fastcgi_path_info",
    "fastcgi_script_name",
    "grpc_internal_trailers",
    "gzip_ratio",
    "host",
    "hostname",
    "http2",
    "http3",
    "http_",
    "https",
    "invalid_referer",
    "is_args",
    "is_request_port",
    "limit_conn_status",
    "limit_rate",
    "limit_req_status",
    "modern_browser",
    "msec",
    "msie",
    "nginx_version",
    "pid",
    "pipe",
    "proxy_add_x_forwarded_for",
    "proxy_host",
    "proxy_port",
    "proxy_protocol_addr",
    "proxy_protocol_port",
    "proxy_protocol_server_addr",
    "proxy_protocol_server_port",
    "proxy_protocol_tlv_",
    "query_string",
    "realip_remote_addr",
    "realip_remote_port",
    "realpath_root",
    "remote_addr",
    "remote_port",
    "remote_user",
    "request",
    "request_body",
    "request_body_file",
    "request_completion",
    "request_filename",
    "request_id",
    "request_length",
    "request_method",
    "request_port",
    "request_time",
    "request_uri",
    "scheme",
    "scgi_script_name",
    "secure_link",
    "secure_link_expires",
    "sent_http_",
    "sent_trailer_",
    "server_addr",
    "server_name",
    "server_port",
    "server_protocol",
    "slice_range",
    "ssl_alpn_protocol",
    "ssl_cipher",
    "ssl_ciphers",
    "ssl_client_cert",
    "ssl_client_escaped_cert",
    "ssl_client_fingerprint",
    "ssl_client_i_dn",
    "ssl_client_i_dn_legacy",
    "ssl_client_raw_cert",
    "ssl_client_s_dn",
    "ssl_client_s_dn_legacy",
    "ssl_client_serial",
    "ssl_client_sigalg",
    "ssl_client_v_end",
    "ssl_client_v_remain",
    "ssl_client_v_start",
    "ssl_client_verify",
    "ssl_curve",
    "ssl_curves",
    "ssl_early_data",
    "ssl_ech_outer_server_name",
    "ssl_ech_status",
    "ssl_protocol",
    "ssl_server_name",
    "ssl_session_id",
    "ssl_session_reused",
    "ssl_sigalg",
    "ssl_sigalgs",
    "status",
    "tcpinfo_rcv_space",
    "tcpinfo_rtt",
    "tcpinfo_rttvar",
    "tcpinfo_snd_cwnd",
    "time_iso8601",
    "time_local",
    "uid_got",
    "uid_reset",
    "uid_set",
    "upstream_addr",
    "upstream_bytes_received",
    "upstream_bytes_sent",
    "upstream_cache_etag",
    "upstream_cache_last_modified",
    "upstream_cache_status",
    "upstream_connect_time",
    "upstream_cookie_",
    "upstream_header_time",
    "upstream_http_",
    "upstream_response_length",
    "upstream_response_time",
    "upstream_status",
    "upstream_trailer_",
    "uri",
    "uwsgi_script_name",
];

pub(in crate::config::compile) fn validate_http_variables(
    directives: &[Directive],
) -> ConfigResult<()> {
    let mut defined = HashSet::new();
    collect_defined_variables(directives, &mut defined);
    collect_named_captures(directives, &mut defined);
    validate_directive_variables(directives, &defined)
}

fn collect_defined_variables(directives: &[Directive], defined: &mut HashSet<String>) {
    for directive in directives {
        match directive.name.as_str() {
            "set" => add_defined(directive.args.first(), defined),
            "auth_request_set" => add_defined(directive.args.first(), defined),
            "map" => add_defined(directive.args.get(1), defined),
            "split_clients" => add_defined(directive.args.get(1), defined),
            "geo" => {
                let index = if directive.args.len() == 1 { 0 } else { 1 };
                add_defined(directive.args.get(index), defined);
            }
            _ => {}
        }
        collect_defined_variables(&directive.children, defined);
    }
}

fn add_defined(value: Option<&String>, defined: &mut HashSet<String>) {
    let Some(value) = value.and_then(|value| value.strip_prefix('$')) else {
        return;
    };
    if !value.is_empty() {
        defined.insert(value.to_ascii_lowercase());
    }
}

fn collect_named_captures(directives: &[Directive], defined: &mut HashSet<String>) {
    for directive in directives {
        for argument in &directive.args {
            add_named_captures(argument, defined);
        }
        for child in &directive.children {
            add_named_captures(&child.name, defined);
        }
        collect_named_captures(&directive.children, defined);
    }
}

fn add_named_captures(pattern: &str, defined: &mut HashSet<String>) {
    let bytes = pattern.as_bytes();
    let mut index = 0;
    while index + 3 < bytes.len() {
        if bytes[index] != b'(' || bytes[index + 1] != b'?' {
            index += 1;
            continue;
        }
        let name_start = if bytes[index + 2] == b'<' {
            index + 3
        } else if bytes[index + 2] == b'P' && bytes[index + 3] == b'<' {
            index + 4
        } else {
            index += 2;
            continue;
        };
        let mut end = name_start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        if end > name_start {
            defined.insert(pattern[name_start..end].to_ascii_lowercase());
        }
        index = end;
    }
}

fn validate_directive_variables(
    directives: &[Directive],
    defined: &HashSet<String>,
) -> ConfigResult<()> {
    for directive in directives {
        for argument in variable_arguments(directive) {
            validate_template(argument, &directive.location, defined)?;
        }
        if !matches!(directive.name.as_str(), "geo" | "split_clients" | "types") {
            validate_directive_variables(&directive.children, defined)?;
        }
    }
    Ok(())
}

fn variable_arguments(directive: &Directive) -> Vec<&String> {
    match directive.name.as_str() {
        "set" | "auth_request_set" | "rewrite" | "proxy_redirect" => {
            directive.args.iter().skip(1).collect()
        }
        "map" | "split_clients" => directive.args.first().into_iter().collect(),
        "geo" => {
            if directive.args.len() == 2 {
                directive.args.first().into_iter().collect()
            } else {
                Vec::new()
            }
        }
        "if" => {
            let regex_pattern = directive
                .args
                .iter()
                .position(|operator| matches!(operator.as_str(), "~" | "~*" | "!~" | "!~*"))
                .map(|operator| operator + 1);
            directive
                .args
                .iter()
                .enumerate()
                .filter_map(|(index, argument)| (Some(index) != regex_pattern).then_some(argument))
                .collect()
        }
        "proxy_cookie_flags" => {
            if directive
                .args
                .first()
                .is_some_and(|arg| arg.starts_with('~'))
            {
                directive.args.iter().skip(2).collect()
            } else {
                directive.args.iter().collect()
            }
        }
        "proxy_cookie_domain" | "proxy_cookie_path" => {
            if directive
                .args
                .first()
                .is_some_and(|arg| arg.starts_with('~'))
            {
                directive.args.iter().skip(1).collect()
            } else {
                directive.args.iter().collect()
            }
        }
        "location"
        | "server_name"
        | "listen"
        | "types"
        | "gzip_http_version"
        | "valid_referers"
        | "fastcgi_split_path_info" => Vec::new(),
        _ => directive.args.iter().collect(),
    }
}

fn validate_template(
    template: &str,
    location: &SourceLocation,
    defined: &HashSet<String>,
) -> ConfigResult<()> {
    let bytes = template.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'$' {
            index += 1;
            continue;
        }
        index += 1;
        if index >= bytes.len() {
            break;
        }
        let name = if bytes[index] == b'{' {
            index += 1;
            let start = index;
            while index < bytes.len() && bytes[index] != b'}' {
                index += 1;
            }
            if index >= bytes.len() {
                return Err(ConfigError::at(
                    location.clone(),
                    "the closing bracket in variable is missing",
                ));
            }
            let name = &template[start..index];
            index += 1;
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
        if name.is_empty() {
            continue;
        }
        if !is_known_variable(name, defined) {
            return Err(ConfigError::at(
                location.clone(),
                format!("unknown \"{}\" variable", name.to_ascii_lowercase()),
            ));
        }
    }
    Ok(())
}

fn is_known_variable(name: &str, defined: &HashSet<String>) -> bool {
    let normalized = name.to_ascii_lowercase();
    if normalized.len() == 1
        && normalized
            .as_bytes()
            .first()
            .is_some_and(|byte| (b'1'..=b'9').contains(byte))
    {
        return true;
    }
    if defined.contains(&normalized) {
        return true;
    }
    BUILTIN_VARIABLES.iter().any(|known| {
        if known.ends_with('_') {
            normalized.starts_with(known)
        } else {
            normalized == *known
        }
    })
}
