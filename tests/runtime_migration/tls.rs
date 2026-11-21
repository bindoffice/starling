use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn loads_nginx_style_tls_config_and_reloads_https() {
    let port = reserve_port();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let directory = TestDirectory::new("tls");
    let certificate = fixtures.join("test.crt");
    let key = fixtures.join("test.key");
    let config = tls_config(port, &certificate, &key, "revision-one");
    let mut server = RunningServer::start(directory, port, &config);

    let response = tls_get(port, "/secure", &fixtures.join("ca.crt"));
    assert_status(&response, 200);
    assert!(
        response.contains("tls:https:on:localhost:revision-one"),
        "TLS variables were not evaluated:\n{response}"
    );

    server.write_config(&tls_config(port, &certificate, &key, "revision-two"));
    server.reload();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = tls_get(port, "/secure", &fixtures.join("ca.crt"));
        if response.contains("revision-two") {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "TLS reload did not apply the new configuration:\n{response}"
        );
        thread::sleep(Duration::from_millis(50));
    }
    server.assert_running();
}

fn tls_config(port: u16, certificate: &Path, key: &Path, revision: &str) -> String {
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
                    listen 127.0.0.1:{port} ssl;
                    server_name localhost;
                    ssl_certificate {};
                    ssl_certificate_key {};
                    ssl_protocols TLSv1.2;

                    location / {{
                        return 200 "tls:$scheme:$https:$ssl_server_name:{revision}";
                    }}
                }}
            }}
        "#,
        certificate.display(),
        key.display()
    )
}

fn tls_get(port: u16, target: &str, ca_certificate: &Path) -> String {
    let mut roots = RootCertStore::empty();
    let mut reader = BufReader::new(File::open(ca_certificate).expect("open test CA"));
    for certificate in rustls_pemfile::certs(&mut reader) {
        roots
            .add(certificate.expect("parse test CA"))
            .expect("add test CA");
    }
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let connection = ClientConnection::new(
        Arc::new(config),
        ServerName::try_from("localhost").expect("TLS server name"),
    )
    .expect("create TLS client");
    let socket = TcpStream::connect(("127.0.0.1", port)).expect("connect to TLS listener");
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut stream = StreamOwned::new(connection, socket);
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .expect("write HTTPS request");
    let mut response = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => response.extend_from_slice(&chunk[..count]),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => panic!("read HTTPS response: {error}"),
        }
    }
    String::from_utf8_lossy(&response).into_owned()
}
