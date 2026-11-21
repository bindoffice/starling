use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

const CLIENT_TLS_CONFIG_CACHE_LIMIT: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ClientTlsConfigKey {
    verify: bool,
    server_name: bool,
    session_reuse: bool,
    trusted_certificate: Option<PathBuf>,
    verify_depth: u32,
    crl: Option<PathBuf>,
    client_certificate: Option<PathBuf>,
    client_certificate_key: Option<PathBuf>,
    client_certificate_passwords: Vec<Vec<u8>>,
    protocols: TlsProtocols,
    ciphers: TlsCipherConfig,
    alpn_protocols: Vec<Vec<u8>>,
}

impl From<ClientTlsOptions<'_>> for ClientTlsConfigKey {
    fn from(options: ClientTlsOptions<'_>) -> Self {
        Self {
            verify: options.verify,
            server_name: options.server_name,
            session_reuse: options.session_reuse,
            trusted_certificate: options.trusted_certificate.map(Path::to_path_buf),
            verify_depth: options.verify_depth,
            crl: options.crl.map(Path::to_path_buf),
            client_certificate: options.client_certificate.map(Path::to_path_buf),
            client_certificate_key: options.client_certificate_key.map(Path::to_path_buf),
            client_certificate_passwords: options.client_certificate_passwords.to_vec(),
            protocols: options.protocols,
            ciphers: options.ciphers.clone(),
            alpn_protocols: options.alpn_protocols.to_vec(),
        }
    }
}

#[derive(Default)]
pub(in crate::server) struct ClientTlsConfigCache {
    configs: Mutex<HashMap<ClientTlsConfigKey, Arc<tokio_rustls::rustls::ClientConfig>>>,
}

impl ClientTlsConfigCache {
    pub(in crate::server) fn get_or_build(
        &self,
        options: ClientTlsOptions<'_>,
    ) -> io::Result<Arc<tokio_rustls::rustls::ClientConfig>> {
        let key = ClientTlsConfigKey::from(options);
        if let Some(config) = self.configs().get(&key) {
            return Ok(config.clone());
        }

        let config = build_client_tls_config(options)?;
        let mut configs = self.configs();
        if configs.len() >= CLIENT_TLS_CONFIG_CACHE_LIMIT {
            configs.clear();
        }
        Ok(configs.entry(key).or_insert(config).clone())
    }

    pub(in crate::server) fn clear(&self) {
        self.configs().clear();
    }

    fn configs(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        HashMap<ClientTlsConfigKey, Arc<tokio_rustls::rustls::ClientConfig>>,
    > {
        self.configs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options<'a>(ciphers: &'a TlsCipherConfig, session_reuse: bool) -> ClientTlsOptions<'a> {
        ClientTlsOptions {
            verify: false,
            server_name: true,
            session_reuse,
            trusted_certificate: None,
            verify_depth: 1,
            crl: None,
            client_certificate: None,
            client_certificate_key: None,
            client_certificate_passwords: &[],
            protocols: TlsProtocols {
                tls12: true,
                tls13: false,
            },
            ciphers,
            alpn_protocols: &[],
        }
    }

    #[test]
    fn caches_equivalent_client_configs_and_clears_on_reload() {
        let ciphers = TlsCipherConfig::default();
        let cache = ClientTlsConfigCache::default();

        let first = cache.get_or_build(options(&ciphers, true)).unwrap();
        let second = cache.get_or_build(options(&ciphers, true)).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        let disabled = cache.get_or_build(options(&ciphers, false)).unwrap();
        assert!(!Arc::ptr_eq(&first, &disabled));

        cache.clear();
        let rebuilt = cache.get_or_build(options(&ciphers, true)).unwrap();
        assert!(!Arc::ptr_eq(&first, &rebuilt));
    }

    async fn tls12_handshake_kinds(
        session_reuse: bool,
    ) -> Vec<tokio_rustls::rustls::HandshakeKind> {
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let ciphers = TlsCipherConfig::default();
        let cache = ClientTlsConfigCache::default();
        let client_config = cache
            .get_or_build(options(&ciphers, session_reuse))
            .unwrap();
        let certificates = load_certificates(&fixture.join("test.crt")).unwrap();
        let private_key = load_private_key(&fixture.join("test.key"), &[]).unwrap();
        let server_config = Arc::new(
            tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&[
                &tokio_rustls::rustls::version::TLS12,
            ])
            .with_no_client_auth()
            .with_single_cert(certificates, private_key)
            .unwrap(),
        );
        let connector = TlsConnector::from(client_config);
        let mut kinds = Vec::new();

        for _ in 0..2 {
            let (client_io, server_io) = tokio::io::duplex(64 * 1024);
            let acceptor = TlsAcceptor::from(server_config.clone());
            let server = tokio::spawn(async move {
                let mut stream = acceptor.accept(server_io).await.unwrap();
                let mut ping = [0_u8; 4];
                stream.read_exact(&mut ping).await.unwrap();
                stream.write_all(&ping).await.unwrap();
                stream.shutdown().await.unwrap();
            });
            let domain = tokio_rustls::rustls::pki_types::ServerName::try_from("localhost")
                .unwrap()
                .to_owned();
            let mut client = connector.connect(domain, client_io).await.unwrap();
            kinds.push(client.get_ref().1.handshake_kind().unwrap());
            client.write_all(b"ping").await.unwrap();
            let mut pong = [0_u8; 4];
            client.read_exact(&mut pong).await.unwrap();
            assert_eq!(&pong, b"ping");
            server.await.unwrap();
        }
        kinds
    }

    #[tokio::test]
    async fn cached_client_config_reuses_tls12_sessions() {
        assert_eq!(
            tls12_handshake_kinds(true).await,
            [
                tokio_rustls::rustls::HandshakeKind::Full,
                tokio_rustls::rustls::HandshakeKind::Resumed,
            ]
        );
    }

    #[tokio::test]
    async fn disabled_session_reuse_forces_full_tls12_handshakes() {
        assert_eq!(
            tls12_handshake_kinds(false).await,
            [
                tokio_rustls::rustls::HandshakeKind::Full,
                tokio_rustls::rustls::HandshakeKind::Full,
            ]
        );
    }
}
