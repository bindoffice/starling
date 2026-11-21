use super::*;

const FIRST_BODY: &[u8] = b"first";
const RESPONSE_START: &str =
    "Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 100\r\n\r\nfirst";

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

    fn label(self) -> &'static str {
        match self {
            Self::FastCgi => "FastCGI",
            Self::Uwsgi => "uWSGI",
            Self::Scgi => "SCGI",
        }
    }
}

#[derive(Clone, Copy)]
enum FailureMode {
    Truncate,
    Timeout,
}

#[tokio::test]
async fn fastcgi_truncation_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::FastCgi, FailureMode::Truncate, 18890).await;
}

#[tokio::test]
async fn fastcgi_timeout_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::FastCgi, FailureMode::Timeout, 18891).await;
}

#[tokio::test]
async fn uwsgi_truncation_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::Uwsgi, FailureMode::Truncate, 18892).await;
}

#[tokio::test]
async fn uwsgi_timeout_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::Uwsgi, FailureMode::Timeout, 18893).await;
}

#[tokio::test]
async fn scgi_truncation_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::Scgi, FailureMode::Truncate, 18894).await;
}

#[tokio::test]
async fn scgi_timeout_after_commit_closes_without_rewriting() {
    run_committed_failure(GatewayKind::Scgi, FailureMode::Timeout, 18895).await;
}

async fn run_committed_failure(kind: GatewayKind, failure: FailureMode, port: u16) {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!(
                "skipping {} committed failure test: TCP binding is not permitted",
                kind.label()
            );
            return;
        }
        Err(error) => panic!("bind {} upstream: {error}", kind.label()),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_gateway_request(kind, &mut stream).await;
        write_response_start(kind, &mut stream).await;
        stream.flush().await.unwrap();
        match failure {
            FailureMode::Truncate => stream.shutdown().await.unwrap(),
            FailureMode::Timeout => tokio::time::sleep(Duration::from_millis(300)).await,
        }
    });

    let name = kind.name();
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            {name}_pass {upstream};
                            {name}_buffering off;
                            {name}_read_timeout 100ms;
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
    let remote: SocketAddr = format!("127.0.0.1:{}", port + 1000).parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));
    client
        .write_all(b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let committed = read_committed_prefix(&mut client).await;
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), client.read_to_end(&mut rest))
        .await
        .expect("failed gateway should close the committed response")
        .unwrap();
    let response = [committed, rest].concat();
    let response_text = String::from_utf8_lossy(&response);
    assert!(
        response_text.starts_with("HTTP/1.1 200 OK\r\n"),
        "{response_text}"
    );
    assert!(response_text.contains("first"), "{response_text}");
    assert!(
        !response_text.contains("502 Bad Gateway"),
        "{response_text}"
    );
    assert!(
        !response_text.contains("504 Gateway Time-out"),
        "{response_text}"
    );

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("HTTP/1 server should stop after the gateway failure")
        .unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_gateway_request(kind: GatewayKind, stream: &mut TcpStream) {
    match kind {
        GatewayKind::FastCgi => loop {
            let (record_type, content) = read_fastcgi_test_record(stream).await;
            if record_type == FCGI_STDIN && content.is_empty() {
                break;
            }
        },
        GatewayKind::Uwsgi => read_uwsgi_header(stream).await,
        GatewayKind::Scgi => read_scgi_header(stream).await,
    }
}

async fn write_response_start(kind: GatewayKind, stream: &mut TcpStream) {
    match kind {
        GatewayKind::FastCgi => {
            write_fastcgi_record(stream, FCGI_STDOUT, RESPONSE_START.as_bytes())
                .await
                .unwrap();
        }
        GatewayKind::Uwsgi | GatewayKind::Scgi => {
            stream.write_all(RESPONSE_START.as_bytes()).await.unwrap();
        }
    }
}

async fn read_committed_prefix(client: &mut tokio::io::DuplexStream) -> Vec<u8> {
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !response
            .windows(FIRST_BODY.len())
            .any(|window| window == FIRST_BODY)
        {
            let mut buffer = [0_u8; 1024];
            let read = client.read(&mut buffer).await.unwrap();
            assert!(read > 0, "starling closed before committing the response");
            response.extend_from_slice(&buffer[..read]);
        }
    })
    .await
    .expect("the first gateway body chunk should reach the client before failure");
    response
}
