use super::support::*;
use super::*;

#[derive(Clone, Copy)]
enum Gateway {
    FastCgi,
    Uwsgi,
    Scgi,
}

impl Gateway {
    fn directive(self) -> &'static str {
        match self {
            Self::FastCgi => "fastcgi",
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::FastCgi => "FastCGI",
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }

    fn config(self, first: SocketAddr, second: SocketAddr, port: u16) -> String {
        let directive = self.directive();
        format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    upstream gateway_retry {{
                        server {first};
                        server {second};
                    }}
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {directive}_pass gateway_retry;
                            {directive}_request_buffering off;
                            {directive}_buffering off;
                            {directive}_next_upstream error timeout non_idempotent;
                            {directive}_next_upstream_tries 2;
                            {directive}_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        )
    }
}

#[tokio::test]
async fn fastcgi_body_write_cancels_on_client_half_close() {
    run_case(Gateway::FastCgi, 18757).await;
}

#[tokio::test]
async fn uwsgi_body_write_cancels_on_client_half_close() {
    run_case(Gateway::Uwsgi, 18758).await;
}

#[tokio::test]
async fn scgi_body_write_cancels_on_client_half_close() {
    run_case(Gateway::Scgi, 18759).await;
}

async fn run_case(gateway: Gateway, port: u16) {
    let Some((first, frontend_listener)) = bind_listeners().await else {
        return;
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let frontend = frontend_listener.local_addr().unwrap();
    let (prefix_seen, prefix_seen_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        read_prefix(gateway, &mut stream).await;
        prefix_seen.send(()).unwrap();

        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("half-closed client should stop the upstream request body write");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "{} received unexpected bytes after the client half-close: {closed:?}",
            gateway.label()
        );
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!(
                "{} started a retry after the client half-closed",
                gateway.label()
            );
        }
    });

    let (prefix, config) = test_config(&gateway.config(first_address, second_address, port), &[]);
    let server_task = spawn_frontend(frontend_listener, &config);
    let mut client = TcpStream::connect(frontend).await.unwrap();
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), prefix_seen_rx)
        .await
        .expect("gateway upstream should receive the first request-body chunk")
        .unwrap();
    client.shutdown().await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("half-closed client should close the downstream connection")
        .unwrap();
    assert!(
        response.is_empty(),
        "{} half-close must not synthesize a response: {}",
        gateway.label(),
        String::from_utf8_lossy(&response)
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("connection handler should stop after the half-close")
        .unwrap();
    first_backend.await.unwrap();
    second_backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_prefix(gateway: Gateway, stream: &mut TcpStream) {
    match gateway {
        Gateway::FastCgi => loop {
            let (record_type, content) = read_fastcgi_test_record(stream).await;
            if record_type == FCGI_STDIN {
                assert_eq!(content, b"part");
                return;
            }
        },
        Gateway::Uwsgi => {
            read_uwsgi_header(stream).await;
            let mut body = [0_u8; 4];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"part");
        }
        Gateway::Scgi => {
            read_scgi_header(stream).await;
            let mut body = [0_u8; 4];
            stream.read_exact(&mut body).await.unwrap();
            assert_eq!(&body, b"part");
        }
    }
}
