mod directives;
mod http2_warnings;
mod proxy_tls;
mod socket_warnings;
mod tls_warnings;
mod warnings;

pub(super) use directives::{try_apply_compat_directive, validate_certificate_cache};
pub(super) use proxy_tls::{reject_proxy_ssl_conf_command, reject_ssl_conf_command};
pub(super) use warnings::collect_compat_warnings;
