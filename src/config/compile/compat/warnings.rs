use super::super::*;
use super::http2_warnings::collect_http2_warning;
use super::socket_warnings::collect_socket_warning;
use super::tls_warnings::collect_tls_warning;

pub(in crate::config::compile) fn collect_compat_warnings(
    directives: &[Directive],
) -> Vec<ConfigWarning> {
    let mut warnings = Vec::new();
    collect_directive_warnings(directives, &mut warnings);
    warnings
}

fn collect_directive_warnings(directives: &[Directive], warnings: &mut Vec<ConfigWarning>) {
    for directive in directives {
        collect_tls_warning(directive, warnings);
        collect_http2_warning(directive, warnings);
        collect_socket_warning(directive, warnings);
        collect_directive_warnings(&directive.children, warnings);
    }
}
