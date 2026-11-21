use super::super::super::support::*;
use super::super::super::*;

pub(super) const DEFAULT_BUSY_BUFFERS_SIZE: usize = 8 * 1024;

pub(super) fn proxy_config(
    port: u16,
    upstream: &SocketAddr,
    location_directives: &str,
) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            proxy_pass http://{upstream};
                            {location_directives}
                            proxy_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    )
}

pub(super) fn response_body_len(response: &[u8]) -> usize {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map_or(0, |index| response.len().saturating_sub(index + 4))
}

pub(super) async fn read_request_head(stream: &mut TcpStream) {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
    }
}

pub(super) async fn assert_streams_after_threshold(
    location_directives: &str,
    port: u16,
    remote_port: u16,
    total: usize,
    first: usize,
) {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping buffered window test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_head(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\n\
                     Connection: close\r\n\r\n{}",
                    "a".repeat(first)
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        wait_for_release.await.unwrap();
        stream
            .write_all("b".repeat(total - first).as_bytes())
            .await
            .unwrap();
    });

    let (prefix, config) = proxy_config(port, &upstream, location_directives);
    let group = build_listener_groups(&config)[0].clone();
    let local = group.address.clone();
    let remote: SocketAddr = format!("127.0.0.1:{remote_port}").parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        test_state(&config),
    ));
    client
        .write_all(b"GET /window HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while response_body_len(&response) < DEFAULT_BUSY_BUFFERS_SIZE {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(
                read > 0,
                "starling closed before the buffered window arrived"
            );
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("configured buffering should stream after the busy threshold");

    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("buffered response should complete")
        .unwrap();
    let (headers, body) = split_http_response(&response);
    assert!(headers.starts_with("HTTP/1.1 200 OK\r\n"), "{headers}");
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));

    server_task.await.unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
