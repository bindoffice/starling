use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;

use super::support::*;
use super::*;

#[tokio::test]
async fn proxies_grpcs_over_tls_with_h2_alpn() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let certified = load_certificate_paths(
        Some(fixture.join("test.crt").as_path()),
        Some(fixture.join("test.key").as_path()),
        &[],
    )
    .unwrap();
    let resolver = Arc::new(CertificateResolver {
        exact: HashMap::new(),
        wildcard: Vec::new(),
        default: certified,
        stapling: None,
    });
    let versions = rustls_protocol_versions(TlsProtocols {
        tls12: true,
        tls13: true,
    });
    let mut server_config =
        tokio_rustls::rustls::ServerConfig::builder_with_protocol_versions(&versions)
            .with_no_client_auth()
            .with_cert_resolver(resolver);
    server_config.alpn_protocols = vec![b"h2".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(server_config));

    let Some(upstream) = bind_grpc_listener("gRPC TLS upstream test").await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let stream = acceptor.accept(stream).await.unwrap();
        assert_eq!(stream.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
        let mut connection = h2::server::handshake(stream).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            let mut request = request.into_body();
            while request.data().await.is_some() {}
            let response = http::Response::builder()
                .status(200)
                .header("Content-Type", "application/grpc")
                .body(())
                .unwrap();
            let mut stream = respond.send_response(response, false).unwrap();
            stream
                .send_data(Bytes::from_static(b"secure-grpc"), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
            stream.send_trailers(trailers).unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_grpc_h2(connection).await;
    });
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:18575;
                        location / {{
                            grpc_pass grpcs://{upstream_address};
                            grpc_ssl_name localhost;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let response = request_body(
        &config,
        "POST /pkg.Service/Tls HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: application/grpc\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    backend.await.unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.contains("secure-grpc"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}
