use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, Error, ProtocolVersion, SignatureScheme,
    StreamOwned,
};

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn selects_tls_virtual_hosts_and_protocols_by_sni() {
    let port = reserve_port();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let directory = TestDirectory::new("sni");
    let config = sni_config(port, &fixtures.join("test.crt"), &fixtures.join("test.key"));
    let _server = RunningServer::start(directory, port, &config);

    let default = tls_get_unverified(port, "default.test", "/");
    assert_status(&default.body, 200);
    assert_eq!(default.protocol, Some(ProtocolVersion::TLSv1_2));
    assert!(
        default.body.contains("default:default.test:default.test"),
        "default SNI server was not selected:\n{}",
        default.body
    );

    let named = tls_get_unverified(port, "named.test", "/");
    assert_status(&named.body, 200);
    assert_eq!(named.protocol, Some(ProtocolVersion::TLSv1_3));
    assert!(
        named.body.contains("named:named.test:named.test"),
        "named SNI server was not selected:\n{}",
        named.body
    );
}

fn sni_config(port: u16, certificate: &Path, key: &Path) -> String {
    format!(
        r#"
            daemon off;
            worker_processes 1;
            pid logs/nginx.pid;
            error_log logs/error.log notice;

            events {{
                worker_connections 128;
            }}

            http {{
                default_type text/plain;
                access_log logs/access.log;

                server {{
                    listen 127.0.0.1:{port} ssl default_server;
                    server_name default.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2;

                    location / {{
                        return 200 "default:$ssl_server_name:$server_name";
                    }}
                }}

                server {{
                    listen 127.0.0.1:{port} ssl;
                    server_name named.test;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.3;

                    location / {{
                        return 200 "named:$ssl_server_name:$server_name";
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display(),
        certificate.display(),
        key.display()
    )
}

struct TlsResponse {
    protocol: Option<ProtocolVersion>,
    body: String,
}

fn tls_get_unverified(port: u16, server_name: &str, target: &str) -> TlsResponse {
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AcceptAnyServerCertificate))
            .with_no_client_auth();
    let connection = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from(server_name.to_owned()).expect("TLS server name"),
    )
    .expect("create SNI TLS client");
    let socket = TcpStream::connect(("127.0.0.1", port)).expect("connect to SNI listener");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut stream = StreamOwned::new(connection, socket);
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {server_name}\r\nConnection: close\r\n\r\n"
    )
    .expect("write SNI HTTPS request");
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => response.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => panic!("read SNI HTTPS response: {error}"),
        }
    }
    TlsResponse {
        protocol: stream.conn.protocol_version(),
        body: String::from_utf8_lossy(&response).into_owned(),
    }
}

#[derive(Debug)]
struct AcceptAnyServerCertificate;

impl ServerCertVerifier for AcceptAnyServerCertificate {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
