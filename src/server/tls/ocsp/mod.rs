use std::io;

use super::*;
use crate::config::ResolverConfig;

mod cache;
mod certificate;
mod client;
mod refresh;
mod request;
mod response;
mod state;
mod url;

pub(in crate::server) use client::{ClientOcspOptions, ClientOcspVerifier};
pub(in crate::server) use state::StaplingState;

pub(super) struct StaplingOptions<'a> {
    pub(super) enabled: bool,
    pub(super) verify: bool,
    pub(super) file: Option<&'a Path>,
    pub(super) responder: Option<&'a str>,
    pub(super) trusted_certificate: Option<&'a Path>,
    pub(super) resolver: &'a ResolverConfig,
    pub(super) resolver_timeout: Duration,
}

pub(super) fn build_stapling(
    options: StaplingOptions<'_>,
    certificate: Arc<CertifiedKey>,
) -> io::Result<Option<Arc<StaplingState>>> {
    if !options.enabled {
        return Ok(None);
    }
    let Some(identity) = certificate::CertificateIdentity::load(
        &certificate.cert,
        options.trusted_certificate,
        options.verify,
    )?
    else {
        if options.file.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cannot determine the OCSP issuer for ssl_stapling_file",
            ));
        }
        return Ok(None);
    };
    if let Some(path) = options.file {
        let response = std::fs::read(path)?;
        response::validate_static(&identity, &response)?;
        return Ok(Some(StaplingState::new(certificate, Some(response))));
    }

    let responder = match options.responder {
        Some(responder) => responder.to_string(),
        None => {
            let Some(responder) = identity.responder()? else {
                return Ok(None);
            };
            responder
        }
    };
    let Ok(responder) = url::ResponderUrl::parse(&responder) else {
        return Ok(None);
    };
    // An IP responder does not need a DNS resolver. Keep hostname-based
    // responders disabled when no resolver was configured, matching nginx's
    // configuration-time behavior.
    if responder.host.parse::<std::net::IpAddr>().is_err() && !options.resolver.is_configured() {
        return Ok(None);
    }
    let resolver =
        crate::server::resolver::Resolver::new(options.resolver.clone(), options.resolver_timeout);
    let state = StaplingState::new(certificate, None);
    StaplingState::start(
        &state,
        Arc::new(identity),
        responder,
        options.trusted_certificate.map(Path::to_path_buf),
        resolver,
        options.resolver_timeout,
    );
    Ok(Some(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ResolverConfig;
    use crate::server::resolver::Resolver;

    #[tokio::test]
    async fn fetches_and_verifies_an_ocsp_response() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocsp");
        let chain = load_certificates(&fixture.join("server.crt")).unwrap();
        let identity = Arc::new(
            certificate::CertificateIdentity::load(&chain, Some(&fixture.join("ca.crt")), true)
                .unwrap()
                .unwrap(),
        );
        let response = std::fs::read(fixture.join("response.der")).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responder_response = response.clone();
        let responder = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&buffer[..length]);
            }
            assert!(request.starts_with(b"GET /ocsp/"));
            let headers = format!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
                responder_response.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&responder_response).await.unwrap();
        });

        let resolver = Resolver::new(
            ResolverConfig {
                servers: vec!["127.0.0.1:1".parse().unwrap()],
                valid: None,
                ipv6: false,
            },
            Duration::from_secs(1),
        );
        let response = request::fetch(
            identity,
            url::ResponderUrl::parse(&format!("http://{address}/ocsp")).unwrap(),
            Some(fixture.join("ca.crt")),
            resolver,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert!(!response.der.is_empty());
        assert!(response.next_update.is_some());
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn refresh_task_installs_the_fetched_response() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocsp");
        let chain = load_certificates(&fixture.join("server.crt")).unwrap();
        let identity = Arc::new(
            certificate::CertificateIdentity::load(&chain, Some(&fixture.join("ca.crt")), true)
                .unwrap()
                .unwrap(),
        );
        let response = std::fs::read(fixture.join("response.der")).unwrap();
        let certified = load_certificate_paths(
            Some(&fixture.join("server.crt")),
            Some(&fixture.join("server.key")),
            &[],
        )
        .unwrap();
        let state = state::StaplingState::new(certified, None);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let responder_response = response.clone();
        let responder = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let length = stream.read(&mut buffer).await.unwrap();
                assert_ne!(length, 0);
                request.extend_from_slice(&buffer[..length]);
            }
            let headers = format!(
                "HTTP/1.0 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
                responder_response.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&responder_response).await.unwrap();
        });

        state::StaplingState::start(
            &state,
            identity,
            url::ResponderUrl::parse(&format!("http://{address}/ocsp")).unwrap(),
            Some(fixture.join("ca.crt")),
            Resolver::new(
                ResolverConfig {
                    servers: vec!["127.0.0.1:1".parse().unwrap()],
                    valid: None,
                    ipv6: false,
                },
                Duration::from_secs(1),
            ),
            Duration::from_secs(2),
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if state.certificate().ocsp.as_deref() == Some(response.as_slice()) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        responder.await.unwrap();
    }
}
