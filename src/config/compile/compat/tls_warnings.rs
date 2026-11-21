use super::super::*;

pub(super) fn collect_tls_warning(directive: &Directive, warnings: &mut Vec<ConfigWarning>) {
    if matches!(
        directive.name.as_str(),
        "ssl_protocols" | "proxy_ssl_protocols" | "grpc_ssl_protocols" | "uwsgi_ssl_protocols"
    ) {
        for protocol in &directive.args {
            if is_legacy_tls_protocol(protocol) {
                warnings.push(ConfigWarning::at(
                    directive.location.clone(),
                    format!(
                        "\"{}\" value \"{protocol}\" is unavailable in the rustls backend; \
                         only TLSv1.2/TLSv1.3 are enabled for this context",
                        directive.name
                    ),
                ));
            }
        }
    }

    if matches!(
        directive.name.as_str(),
        "ssl_ciphers" | "proxy_ssl_ciphers" | "grpc_ssl_ciphers" | "uwsgi_ssl_ciphers"
    ) && let Some(value) = directive.args.first()
        && let Ok(unsupported) = unsupported_cipher_inclusions(value)
    {
        for cipher in unsupported {
            warnings.push(ConfigWarning::at(
                directive.location.clone(),
                format!(
                    "\"{}\" cipher \"{cipher}\" is unavailable in the rustls backend and \
                     was omitted",
                    directive.name
                ),
            ));
        }
    }
}

fn is_legacy_tls_protocol(protocol: &str) -> bool {
    matches!(
        protocol.to_ascii_lowercase().as_str(),
        "sslv2" | "sslv3" | "tlsv1" | "tlsv1.1"
    )
}
