use super::super::*;

pub(super) fn collect_socket_warning(directive: &Directive, warnings: &mut Vec<ConfigWarning>) {
    if !matches!(
        directive.name.as_str(),
        "send_lowat" | "proxy_send_lowat" | "fastcgi_send_lowat" | "tunnel_send_lowat"
    ) {
        return;
    }

    // Nginx only applies SO_SNDLOWAT where the platform exposes it. The
    // current Tokio/socket2 backend does not expose that socket option, so
    // keep the directives accepted but make the no-op visible in config
    // diagnostics instead of silently pretending they took effect.
    warnings.push(ConfigWarning::at(
        directive.location.clone(),
        format!(
            "\"{}\" is accepted for compatibility but is not applied by the current socket backend",
            directive.name
        ),
    ));
}
