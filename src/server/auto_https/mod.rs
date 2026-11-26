use super::*;
use sha1::{Digest, Sha1};

mod acme;
mod certificate;
mod store;

pub(in crate::server) use acme::auto_https_manager_loop;
#[cfg(test)]
pub(in crate::server) use certificate::certificate_paths;
pub(in crate::server) use certificate::{
    CertificateLock, CertificatePaths, CertificateSource, ensure_local_certificate,
    write_acme_certificate,
};
pub(in crate::server) use store::{ACME_CHALLENGE_PREFIX, AcmeOrder, AutoHttpsState};

use crate::config::{AutoHttpsChallenge, AutoHttpsConfig, ListenConfig, ServerName};

static AUTO_HTTPS_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Default)]
pub(in crate::server) struct AutoHttpsPreparation {
    pub(in crate::server) notices: Vec<(LogLevel, String)>,
}

pub(in crate::server) fn prepare_auto_https(
    config: &mut RuntimeConfig,
    state: &AutoHttpsState,
) -> io::Result<AutoHttpsPreparation> {
    let mut preparation = AutoHttpsPreparation::default();
    let settings = config.auto_https.clone();
    state.update_config(settings.clone());
    state.set_orders(Vec::new());
    if !settings.enabled {
        return Ok(preparation);
    }
    let Some(http) = config.http.as_mut() else {
        return Ok(preparation);
    };

    let mut orders = Vec::new();
    for server in &mut http.servers {
        let domains = certificate_domains(&server.names);
        if domains.is_empty() || !server_uses_auto_https(server, &settings) {
            continue;
        }
        let (local_domains, public_domains) = classify_domains(&domains);
        let explicit_certificate =
            server.context.ssl_certificate.is_some() || !server.context.ssl_certificates.is_empty();
        let explicit_key = server.context.ssl_certificate_key.is_some()
            || !server.context.ssl_certificate_keys.is_empty();
        if explicit_certificate != explicit_key {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "server {} has an incomplete ssl_certificate/ssl_certificate_key pair",
                    server.default_server_name
                ),
            ));
        }
        if !explicit_certificate {
            let paths =
                ensure_local_certificate(&settings.cert_dir, &domains, settings.renew_before)?;
            let mut context = (*server.context).clone();
            context.ssl_certificate = Some(paths.cert.clone());
            context.ssl_certificate_key = Some(paths.key.clone());
            server.context = Arc::new(context);
            add_https_listeners(server, &settings);

            let wildcard_public = public_domains.iter().any(|domain| domain.contains('*'));
            let acme_domains = if settings.challenge == AutoHttpsChallenge::Dns01 {
                public_domains
            } else {
                public_domains
                    .into_iter()
                    .filter(|domain| !domain.contains('*'))
                    .collect::<Vec<_>>()
            };
            if wildcard_public && settings.challenge == AutoHttpsChallenge::Http01 {
                preparation.notices.push((
                    LogLevel::Warn,
                    format!(
                        "server \"{}\" contains a public wildcard name; HTTP-01 cannot \
                         validate it, so automatic ACME issuance was skipped for that name",
                        server.default_server_name
                    ),
                ));
            }
            if !acme_domains.is_empty() && !local_domains.is_empty() {
                preparation.notices.push((
                    LogLevel::Warn,
                    format!(
                        "server \"{}\" mixes local and public names; automatic ACME issuance \
                         is skipped because one TLS certificate cannot cover both sets",
                        server.default_server_name
                    ),
                ));
            } else if !acme_domains.is_empty() {
                orders.push(AcmeOrder {
                    domains: acme_domains,
                    paths: paths.clone(),
                    staging: settings.staging,
                    directory_url: settings.acme_directory_url().to_string(),
                    ca_file: settings.acme_ca_file.clone(),
                    email: settings.email.clone(),
                    challenge: settings.challenge,
                    dns_hook: settings.dns_hook.clone(),
                    dns_propagation_delay: settings.dns_propagation_delay,
                });
            }
        } else {
            add_https_listeners(server, &settings);
        }

        let local_ca = settings.cert_dir.join("local-ca.crt");
        if !local_domains.is_empty() {
            preparation.notices.push((
                LogLevel::Notice,
                format!(
                    "automatic HTTPS enabled for local development names on server \"{}\"; \
                     trust {} to remove browser warnings",
                    server.default_server_name,
                    local_ca.display()
                ),
            ));
        }
    }
    state.set_orders(orders);
    Ok(preparation)
}

fn server_uses_auto_https(
    server: &crate::config::ServerConfig,
    settings: &AutoHttpsConfig,
) -> bool {
    server.listens.iter().any(|listen| {
        listen.udp
            || listen.ssl
            || listen.address.port() == settings.http_port
            || listen.address.port() == settings.https_port
    })
}

fn certificate_domains(names: &[ServerName]) -> Vec<String> {
    let mut domains = Vec::new();
    for name in names {
        let candidates = match name {
            ServerName::Exact(value) => vec![value.trim_end_matches('.').to_string()],
            ServerName::LeadingWildcard { display, .. } => {
                let bare = display.trim_end_matches('.').trim_start_matches("*.");
                if display.starts_with("*.") {
                    vec![format!("*.{bare}")]
                } else {
                    vec![bare.to_string(), format!("*.{bare}")]
                }
            }
            ServerName::TrailingWildcard { display, .. } => {
                vec![display.trim_end_matches('.').to_string()]
            }
            ServerName::Regex { .. } => continue,
        };
        for domain in candidates {
            if !domain.is_empty() && !domains.contains(&domain) {
                domains.push(domain);
            }
        }
    }
    domains
}

fn classify_domains(domains: &[String]) -> (Vec<String>, Vec<String>) {
    let mut local = Vec::new();
    let mut public = Vec::new();
    for domain in domains {
        if is_local_domain(domain) {
            local.push(domain.clone());
        } else {
            public.push(domain.clone());
        }
    }
    (local, public)
}

pub(in crate::server) fn is_local_domain(domain: &str) -> bool {
    let domain = domain
        .trim_end_matches('.')
        .trim_start_matches("*.")
        .to_ascii_lowercase();
    if domain.is_empty()
        || domain == "localhost"
        || domain.ends_with(".localhost")
        || domain == "localhost.localdomain"
        || domain.ends_with(".localhost.localdomain")
        || domain.ends_with(".local")
        || domain == "localdomain"
        || domain.ends_with(".localdomain")
        || domain.ends_with(".test")
        || domain.ends_with(".invalid")
        || domain.ends_with(".example")
        || domain == "internal"
        || domain.ends_with(".internal")
        || domain == "home.arpa"
        || domain.ends_with(".home.arpa")
        || domain == "example.com"
        || domain.ends_with(".example.com")
        || domain == "example.net"
        || domain.ends_with(".example.net")
        || domain == "example.org"
        || domain.ends_with(".example.org")
        || domain.parse::<IpAddr>().is_ok()
        || !domain.contains('.')
    {
        return true;
    }
    false
}

fn add_https_listeners(server: &mut crate::config::ServerConfig, settings: &AutoHttpsConfig) {
    let mut additions = Vec::new();
    for listen in server.listens.clone() {
        if listen.udp || listen.ssl || listen.address.port() != settings.http_port {
            continue;
        }
        let Some(address) = listen.address.socket_addr() else {
            continue;
        };
        let mut https_address = address;
        https_address.set_port(settings.https_port);
        if let Some(existing) = server
            .listens
            .iter_mut()
            .find(|candidate| candidate.address.socket_addr() == Some(https_address))
        {
            existing.ssl = true;
            continue;
        }
        additions.push(ListenConfig {
            address: crate::config::ListenAddress::Inet(https_address),
            ssl: true,
            http2: listen.http2,
            quic: false,
            udp: false,
            proxy_protocol: listen.proxy_protocol,
            default_server: listen.default_server,
            socket_options: listen.socket_options.clone(),
            socket_options_set: listen.socket_options_set,
        });
    }
    for listen in &mut server.listens {
        if listen.address.port() == settings.https_port && !listen.udp {
            listen.ssl = true;
        }
    }
    server.listens.extend(additions);
}
