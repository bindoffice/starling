use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use openssl::x509::X509;

use super::cache::OcspCache;
use super::certificate::{CertificateIdentity, load_trusted_certificates};
use super::{request, url};
use crate::config::{ClientCertificateVerify, OcspMode, ResolverConfig};
use crate::server::resolver::Resolver;

pub(in crate::server) struct ClientOcspVerifier {
    mode: OcspMode,
    responder: Option<url::ResponderUrl>,
    trusted_certificates: Vec<X509>,
    responder_trusted_certificate: Option<PathBuf>,
    resolver: Resolver,
    timeout: Duration,
    cache: Option<Arc<OcspCache>>,
}

pub(in crate::server) struct ClientOcspOptions<'a> {
    pub(in crate::server) mode: OcspMode,
    pub(in crate::server) verify_client: ClientCertificateVerify,
    pub(in crate::server) responder: Option<&'a str>,
    pub(in crate::server) cache: Option<&'a str>,
    pub(in crate::server) client_certificate: Option<&'a Path>,
    pub(in crate::server) trusted_certificate: Option<&'a Path>,
    pub(in crate::server) resolver: &'a ResolverConfig,
    pub(in crate::server) resolver_timeout: Duration,
}

impl ClientOcspVerifier {
    pub(in crate::server) fn build(
        options: ClientOcspOptions<'_>,
    ) -> io::Result<Option<Arc<Self>>> {
        if options.mode == OcspMode::Off
            || matches!(options.verify_client, ClientCertificateVerify::Off)
        {
            return Ok(None);
        }
        let client_trusted_certificate = options
            .client_certificate
            .or(options.trusted_certificate)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no ssl_client_certificate or ssl_trusted_certificate for ssl_ocsp",
                )
            })?
            .to_path_buf();
        let trusted_certificates = load_trusted_certificates(&client_trusted_certificate)?;
        let responder = options
            .responder
            .map(url::ResponderUrl::parse)
            .transpose()?;
        Ok(Some(Arc::new(Self {
            mode: options.mode,
            responder,
            trusted_certificates,
            responder_trusted_certificate: options.trusted_certificate.map(Path::to_path_buf),
            resolver: Resolver::new(options.resolver.clone(), options.resolver_timeout),
            timeout: options.resolver_timeout,
            cache: OcspCache::from_spec(options.cache),
        })))
    }

    pub(in crate::server) async fn validate(
        &self,
        certificates: &[tokio_rustls::rustls::pki_types::CertificateDer<'_>],
    ) -> io::Result<()> {
        if certificates.is_empty() {
            return Ok(());
        }
        let certificates = certificates
            .iter()
            .map(|certificate| X509::from_der(certificate.as_ref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(super::certificate::openssl_error)?;
        let mut chain = certificates.clone();
        let mut seen = chain
            .iter()
            .map(|certificate| certificate.to_der())
            .collect::<Result<std::collections::HashSet<_>, _>>()
            .map_err(super::certificate::openssl_error)?;
        for certificate in self.trusted_certificates.iter().cloned() {
            if seen.insert(
                certificate
                    .to_der()
                    .map_err(super::certificate::openssl_error)?,
            ) {
                chain.push(certificate);
            }
        }

        for index in 0..chain.len() {
            if self.mode == OcspMode::Leaf && index > 0 {
                break;
            }
            if is_self_issued(&chain[index])? && (index > 0 || chain.len() == 1) {
                break;
            }
            let current = chain[index]
                .to_der()
                .map_err(super::certificate::openssl_error)?;
            let verification_certificates = certificates
                .iter()
                .filter(|certificate| certificate.to_der().ok().as_deref() != Some(&current))
                .cloned()
                .collect();
            let identity = match CertificateIdentity::from_parts(
                chain[index].clone(),
                verification_certificates,
                self.trusted_certificates.clone(),
                true,
            )? {
                Some(identity) => Arc::new(identity),
                None if index + 1 == chain.len() => break,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "cannot determine the issuer of a client certificate",
                    ));
                }
            };
            let responder = match &self.responder {
                Some(responder) => responder.clone(),
                None => {
                    let Some(responder) = identity.responder()? else {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            "no OCSP responder URL in the client certificate",
                        ));
                    };
                    url::ResponderUrl::parse(&responder)?
                }
            };
            let responder_key = responder.cache_key();
            let cache_key = identity.cache_key(&responder_key)?;
            if self
                .cache
                .as_ref()
                .is_some_and(|cache| cache.contains(&cache_key))
            {
                continue;
            }
            let response = request::fetch(
                identity,
                responder,
                self.responder_trusted_certificate.clone(),
                self.resolver.clone(),
                self.timeout,
            )
            .await
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("client certificate OCSP validation failed: {error}"),
                )
            })?;
            if let Some(cache) = &self.cache {
                cache.insert(cache_key, response.next_update);
            }
        }
        Ok(())
    }
}

fn is_self_issued(certificate: &X509) -> io::Result<bool> {
    Ok(certificate.issued(certificate) == openssl::x509::X509VerifyResult::OK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn validates_a_client_certificate_and_reuses_the_cache() {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocsp");
        let response = std::fs::read(fixture.join("response.der")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response_for_server = response.clone();
        let server = tokio::spawn(async move {
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
                "HTTP/1.1 200 OK\r\nContent-Type: application/ocsp-response\r\nContent-Length: {}\r\n\r\n",
                response_for_server.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&response_for_server).await.unwrap();
        });

        let cache = format!("shared:client-ocsp-{}:1m", address.port());
        let verifier = ClientOcspVerifier::build(ClientOcspOptions {
            mode: OcspMode::Leaf,
            verify_client: ClientCertificateVerify::On,
            responder: Some(&format!("http://{address}/ocsp")),
            cache: Some(&cache),
            client_certificate: Some(&fixture.join("ca.crt")),
            trusted_certificate: None,
            resolver: &ResolverConfig {
                servers: vec!["127.0.0.1:1".parse().unwrap()],
                valid: None,
                ipv6: false,
            },
            resolver_timeout: Duration::from_secs(2),
        })
        .unwrap()
        .unwrap();
        let certificates =
            crate::server::tls::load_certificates(&fixture.join("server.crt")).unwrap();
        verifier.validate(&certificates).await.unwrap();
        verifier.validate(&certificates).await.unwrap();
        server.await.unwrap();
    }
}
