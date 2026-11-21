use super::*;

#[tokio::test]
async fn fastcgi_maps_truncated_streaming_body_to_400_without_terminal_stdin() {
    run_truncated_request_body(GatewayKind::FastCgi, 18730).await;
}

#[tokio::test]
async fn uwsgi_maps_truncated_streaming_body_to_400_after_the_read_prefix() {
    run_truncated_request_body(GatewayKind::Uwsgi, 18731).await;
}

#[tokio::test]
async fn scgi_maps_truncated_streaming_body_to_400_after_the_read_prefix() {
    run_truncated_request_body(GatewayKind::Scgi, 18732).await;
}

#[derive(Clone, Copy)]
enum GatewayKind {
    FastCgi,
    Uwsgi,
    Scgi,
}

impl GatewayKind {
    fn name(self) -> &'static str {
        match self {
            Self::FastCgi => "fastcgi",
            Self::Uwsgi => "uwsgi",
            Self::Scgi => "scgi",
        }
    }
}

struct CapturedBody {
    bytes: Vec<u8>,
    terminal_record: bool,
}

async fn run_truncated_request_body(kind: GatewayKind, listen_port: u16) {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping {} request framing test: TCP binding is not permitted",
                kind.name()
            );
            return;
        }
        Err(error) => panic!("bind {} upstream: {error}", kind.name()),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        match kind {
            GatewayKind::FastCgi => capture_fastcgi_body(&mut stream).await,
            GatewayKind::Uwsgi => {
                read_uwsgi_header(&mut stream).await;
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).await.unwrap();
                CapturedBody {
                    bytes,
                    terminal_record: false,
                }
            }
            GatewayKind::Scgi => {
                read_scgi_header(&mut stream).await;
                let mut bytes = Vec::new();
                stream.read_to_end(&mut bytes).await.unwrap();
                CapturedBody {
                    bytes,
                    terminal_record: false,
                }
            }
        }
    });

    let response = send_truncated_body_request(kind, upstream, listen_port).await;
    assert!(response.starts_with("HTTP/1.1 400 "), "{response}");
    let captured = backend.await.unwrap();
    assert_eq!(captured.bytes, b"part");
    assert!(
        !captured.terminal_record,
        "{} must not send a successful body terminator after a request framing error",
        kind.name()
    );
}

async fn capture_fastcgi_body(stream: &mut TcpStream) -> CapturedBody {
    let mut bytes = Vec::new();
    let mut terminal_record = false;
    loop {
        let mut header = [0_u8; 8];
        match stream.read_exact(&mut header).await {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => panic!("read FastCGI record header: {error}"),
        }
        let length = u16::from_be_bytes([header[4], header[5]]) as usize;
        let padding = header[6] as usize;
        let mut content = vec![0_u8; length];
        stream.read_exact(&mut content).await.unwrap();
        if padding > 0 {
            let mut padding = vec![0_u8; padding];
            stream.read_exact(&mut padding).await.unwrap();
        }
        if header[1] == FCGI_STDIN {
            if content.is_empty() {
                terminal_record = true;
            } else {
                bytes.extend_from_slice(&content);
            }
        }
    }
    CapturedBody {
        bytes,
        terminal_record,
    }
}

async fn send_truncated_body_request(
    kind: GatewayKind,
    upstream: SocketAddr,
    listen_port: u16,
) -> String {
    let name = kind.name();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{listen_port};
                        location / {{
                            {name}_pass {upstream};
                            {name}_request_buffering off;
                            {name}_send_timeout 5s;
                            {name}_read_timeout 5s;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = format!("127.0.0.1:{}", listen_port + 27_000)
        .parse()
        .unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(
            b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 6\r\n\
              Connection: close\r\n\r\npart",
        )
        .await
        .unwrap();
    client.shutdown().await.unwrap();

    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut response))
        .await
        .expect("truncated request body should receive a response")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should close after the request framing error")
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
    String::from_utf8(response).unwrap()
}
