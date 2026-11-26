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

    let ignored = match directive.name.as_str() {
        "ssl_dhparam" => Some(
            "\"ssl_dhparam\" is ignored by the rustls TLS backend; \
             finite-field DHE is unavailable, use ECDHE cipher suites instead",
        ),
        "ssl_ecdh_curve" => Some(
            "\"ssl_ecdh_curve\" is ignored by the rustls TLS backend; \
             the backend's built-in X25519/P-256/P-384 preference is used",
        ),
        "ssl_early_data" if directive.args.first().is_some_and(|value| value == "on") => {
            Some("\"ssl_early_data on\" is not supported by the rustls TLS backend and was ignored")
        }
        "ssl_buffer_size" => Some(
            "\"ssl_buffer_size\" is ignored by the rustls TLS backend; \
             record buffering is managed by rustls",
        ),
        _ => None,
    };
    if let Some(message) = ignored {
        warnings.push(ConfigWarning::at(directive.location.clone(), message));
    }
}

fn is_legacy_tls_protocol(protocol: &str) -> bool {
    matches!(
        protocol.to_ascii_lowercase().as_str(),
        "sslv2" | "sslv3" | "tlsv1" | "tlsv1.1"
    )
}
