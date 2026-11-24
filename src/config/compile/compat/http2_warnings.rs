use super::super::*;

pub(super) fn collect_http2_warning(directive: &Directive, warnings: &mut Vec<ConfigWarning>) {
    let replacement = match directive.name.as_str() {
        "http2_max_requests" => Some("keepalive_requests"),
        "http2_max_field_size" | "http2_max_header_size" => Some("large_client_header_buffers"),
        "http2_recv_timeout" => Some("client_header_timeout"),
        "http2_idle_timeout" => Some("keepalive_timeout"),
        "http2_max_concurrent_pushes" | "http2_push_preload" | "http2_push" => None,
        _ => return,
    };
    let message = match replacement {
        Some(replacement) => format!(
            "the \"{}\" directive is obsolete, use the \"{replacement}\" directive instead",
            directive.name
        ),
        None => format!("the \"{}\" directive is obsolete, ignored", directive.name),
    };
    warnings.push(ConfigWarning::at(directive.location.clone(), message));
}
