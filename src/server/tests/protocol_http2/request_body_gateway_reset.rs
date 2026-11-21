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
                        listen 127.0.0.1:{port} http2;
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
async fn rst_stream_during_fastcgi_body_write_cancels_without_retry() {
    run_case(Gateway::FastCgi, 18671).await;
}

#[tokio::test]
async fn rst_stream_during_uwsgi_body_write_cancels_without_retry() {
    run_case(Gateway::Uwsgi, 18672).await;
}

#[tokio::test]
async fn rst_stream_during_scgi_body_write_cancels_without_retry() {
    run_case(Gateway::Scgi, 18673).await;
}

async fn run_case(gateway: Gateway, port: u16) {
    let Some(first) = bind_test_tcp("gateway reset backend").await else {
        return;
    };
    let Some(second) = bind_test_tcp("gateway reset retry backend").await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let (prefix_seen, prefix_seen_rx) = tokio::sync::oneshot::channel();
    let first_backend = tokio::spawn(async move {
        let (mut stream, _) = first.accept().await.unwrap();
        read_prefix(gateway, &mut stream).await;
        prefix_seen.send(()).unwrap();

        let mut byte = [0_u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut byte))
            .await
            .expect("HTTP/2 reset should close the streaming gateway request");
        assert!(
            matches!(closed, Ok(0) | Err(_)),
            "{} received unexpected bytes after reset: {closed:?}",
            gateway.label()
        );
    });
    let second_backend = tokio::spawn(async move {
        if tokio::time::timeout(Duration::from_millis(500), second.accept())
            .await
            .is_ok()
        {
            panic!(
                "{} started a retry after the HTTP/2 client reset",
                gateway.label()
            );
        }
    });

    let source = gateway.config(first_address, second_address, port);
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45689".parse().unwrap();
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server_io,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .header("content-length", "8")
        .body(())
        .unwrap();
    let (response, mut request_stream) = sender.send_request(request, false).unwrap();
    request_stream
        .send_data(Bytes::from_static(b"part"), false)
        .unwrap();
    prefix_seen_rx.await.unwrap();
    request_stream.send_reset(h2::Reason::CANCEL);

    assert!(
        tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .expect("reset response future should complete")
            .is_err()
    );

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
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

async fn read_uwsgi_header<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await.unwrap();
    let length = u16::from_le_bytes([header[1], header[2]]) as usize;
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}

async fn read_scgi_header<R: AsyncRead + Unpin>(reader: &mut R) {
    let mut length = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        reader.read_exact(&mut byte).await.unwrap();
        if byte[0] == b':' {
            break;
        }
        length.push(byte[0]);
    }
    let length = std::str::from_utf8(&length).unwrap().parse().unwrap();
    let mut parameters = vec![0_u8; length];
    reader.read_exact(&mut parameters).await.unwrap();
}
