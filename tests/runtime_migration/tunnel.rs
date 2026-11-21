use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use super::support::{RunningServer, TestDirectory, assert_status, reserve_port};

#[test]
#[ignore = "requires local TCP bind; run with --ignored"]
fn server_level_tunnel_uses_named_upstream_failover_and_half_close() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind tunnel backend");
    let backend_port = listener.local_addr().unwrap().port();
    let backend = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept tunnel connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();

        let mut payload = [0_u8; 5];
        stream.read_exact(&mut payload).expect("tunnel payload");
        assert_eq!(&payload, b"hello");

        let mut extra = [0_u8; 1];
        let read = stream.read(&mut extra).expect("wait for tunnel half-close");
        assert_eq!(read, 0, "client half-close did not reach the backend");

        stream.write_all(b"world").expect("write tunnel response");
        stream.shutdown(Shutdown::Both).unwrap();
    });

    let port = reserve_port();
    let directory = TestDirectory::new("tunnel");
    let server = RunningServer::start(directory, port, &tunnel_config(port, backend_port));
    let response = connect_request(port);
    assert_status(&response, 200);
    assert!(
        response.ends_with("world"),
        "tunnel response body missing:\n{response}"
    );

    backend.join().unwrap();
    drop(server);
}

fn connect_request(port: u16) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect tunnel client");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .write_all(
            b"CONNECT example.test:443 HTTP/1.1\r\n\
              Host: example.test:443\r\n\
              \r\n\
              hello",
        )
        .unwrap();
    stream.shutdown(Shutdown::Write).unwrap();

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read tunnel response");
    String::from_utf8(response).expect("UTF-8 tunnel response")
}

fn tunnel_config(port: u16, backend_port: u16) -> String {
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
                upstream tunnel_backend {{
                    server 127.0.0.1:1;
                    server 127.0.0.1:{backend_port};
                }}

                server {{
                    listen 127.0.0.1:{port};
                    server_name localhost;
                    tunnel_connect_timeout 100ms;
                    tunnel_read_timeout 5s;
                    tunnel_next_upstream error;
                    tunnel_next_upstream_tries 2;
                    tunnel_pass tunnel_backend;

                    location / {{}}
                }}
            }}
        "#
    )
}
