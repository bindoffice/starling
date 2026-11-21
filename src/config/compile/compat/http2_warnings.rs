use super::super::*;

pub(super) fn collect_http2_warning(directive: &Directive, warnings: &mut Vec<ConfigWarning>) {
    if !matches!(
        directive.name.as_str(),
        "http2_max_field_size" | "http2_max_header_size"
    ) {
        return;
    }
    warnings.push(ConfigWarning::at(
        directive.location.clone(),
        format!(
            "the \"{}\" directive is obsolete, use the \"large_client_header_buffers\" directive instead",
            directive.name
        ),
    ));
}
