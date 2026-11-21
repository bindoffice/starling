use super::super::support::*;
use super::super::*;
use super::support::*;

#[tokio::test]
async fn named_upstream_retries_file_backed_fastcgi_request_body_from_the_start() {
    let Some(first) = bind_upstream("FastCGI request retry test").await else {
        return;
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = serve_fastcgi(
        first,
        b"Status: 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
    );
    let second_backend = serve_fastcgi(
        second,
        b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
    );

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:18581;
                        location / {{
                            fastcgi_pass file_retry;
                            fastcgi_next_upstream http_500 non_idempotent;
                            fastcgi_next_upstream_tries 2;
                            fastcgi_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed FastCGI request body should be removed after retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_retries_file_backed_uwsgi_request_body_from_the_start() {
    run_raw_gateway_retry(GatewayKind::Uwsgi, 18587).await;
}

#[tokio::test]
async fn named_upstream_retries_file_backed_scgi_request_body_from_the_start() {
    run_raw_gateway_retry(GatewayKind::Scgi, 18588).await;
}

#[derive(Clone, Copy)]
enum GatewayKind {
    FastCgi,
    Uwsgi,
    Scgi,
}

impl GatewayKind {
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
}

async fn run_raw_gateway_retry(gateway: GatewayKind, listen_port: u16) {
    let label = format!("{} request retry test", gateway.directive());
    let Some(first) = bind_upstream(&label).await else {
        return;
    };
    let second = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = serve_raw_gateway(
        first,
        gateway,
        b"Status: 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n",
    );
    let second_backend = serve_raw_gateway(
        second,
        gateway,
        b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
    );
    let directive = gateway.directive();

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:{listen_port};
                        location / {{
                            {directive}_pass file_retry;
                            {directive}_next_upstream http_500 non_idempotent;
                            {directive}_next_upstream_tries 2;
                            {directive}_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_body(
        &config,
        "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\nConnection: close\r\n\r\npartmore",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed gateway request body should be removed after retry"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn named_upstream_retries_file_backed_gateway_request_body_after_read_timeout() {
    run_gateway_failure_retries(GatewayFailure::Timeout).await;
}

#[tokio::test]
async fn named_upstream_retries_file_backed_gateway_request_body_after_invalid_response() {
    run_gateway_failure_retries(GatewayFailure::InvalidHeader).await;
}

#[tokio::test]
async fn named_upstream_retries_file_backed_gateway_request_body_after_connect_failure() {
    for (gateway, listen_port) in [
        (GatewayKind::FastCgi, 18664),
        (GatewayKind::Uwsgi, 18665),
        (GatewayKind::Scgi, 18666),
    ] {
        run_gateway_connect_failure_retry(gateway, listen_port).await;
    }
}

#[derive(Clone, Copy)]
enum GatewayFailure {
    Timeout,
    InvalidHeader,
}

impl GatewayFailure {
    fn flag(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::InvalidHeader => "invalid_header",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Timeout => "read timeout",
            Self::InvalidHeader => "invalid response",
        }
    }
}

async fn run_gateway_failure_retries(failure: GatewayFailure) {
    let gateways = match failure {
        GatewayFailure::Timeout => [
            (GatewayKind::FastCgi, 18650),
            (GatewayKind::Uwsgi, 18651),
            (GatewayKind::Scgi, 18652),
        ],
        GatewayFailure::InvalidHeader => [
            (GatewayKind::FastCgi, 18653),
            (GatewayKind::Uwsgi, 18654),
            (GatewayKind::Scgi, 18655),
        ],
    };
    for (gateway, listen_port) in gateways {
        run_gateway_failure_retry(gateway, failure, listen_port).await;
    }
}

async fn run_gateway_failure_retry(
    gateway: GatewayKind,
    failure: GatewayFailure,
    listen_port: u16,
) {
    let label = format!("{} {} retry test", gateway.label(), failure.label());
    let Some(first) = bind_upstream(&label).await else {
        return;
    };
    let Some(second) = bind_upstream(&label).await else {
        return;
    };
    let first_address = first.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    let first_backend = serve_gateway(first, gateway, GatewayReply::Failure(failure));
    let second_backend = serve_gateway(
        second,
        gateway,
        GatewayReply::Response(
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
        ),
    );
    let directive = gateway.directive();
    let flag = failure.flag();

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {first_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:{listen_port};
                        location / {{
                            {directive}_pass file_retry;
                            {directive}_read_timeout 20ms;
                            {directive}_next_upstream {flag} non_idempotent;
                            {directive}_next_upstream_tries 2;
                            {directive}_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "{} {} retry should finish",
            gateway.label(),
            failure.label()
        )
    });
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(first_backend.await.unwrap(), b"partmore");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed {} request body should be removed after {} retry",
        gateway.label(),
        failure.label()
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn run_gateway_connect_failure_retry(gateway: GatewayKind, listen_port: u16) {
    let label = format!("{} connect retry test", gateway.label());
    let Some(unavailable) = bind_upstream(&label).await else {
        return;
    };
    let Some(second) = bind_upstream(&label).await else {
        return;
    };
    let unavailable_address = unavailable.local_addr().unwrap();
    let second_address = second.local_addr().unwrap();
    drop(unavailable);
    let second_backend = serve_gateway(
        second,
        gateway,
        GatewayReply::Response(
            b"Status: 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\nok",
        ),
    );
    let directive = gateway.directive();

    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    upstream file_retry {{
                        server {unavailable_address};
                        server {second_address};
                    }}
                    server {{
                        listen 127.0.0.1:{listen_port};
                        location / {{
                            {directive}_pass file_retry;
                            {directive}_next_upstream error non_idempotent;
                            {directive}_next_upstream_tries 2;
                            {directive}_buffering off;
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        request_body(
            &config,
            "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
             Connection: close\r\n\r\npartmore",
        ),
    )
    .await
    .unwrap_or_else(|_| panic!("{} connect retry should finish", gateway.label()));
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("ok"), "{response}");
    assert_eq!(second_backend.await.unwrap(), b"partmore");
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed {} request body should be removed after connect retry",
        gateway.label()
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_upstream(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

fn serve_fastcgi(
    listener: TcpListener,
    response: &'static [u8],
) -> tokio::task::JoinHandle<Vec<u8>> {
    serve_gateway(
        listener,
        GatewayKind::FastCgi,
        GatewayReply::Response(response),
    )
}

#[derive(Clone, Copy)]
enum GatewayReply {
    Response(&'static [u8]),
    Failure(GatewayFailure),
}

fn serve_gateway(
    listener: TcpListener,
    gateway: GatewayKind,
    reply: GatewayReply,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let body = match gateway {
            GatewayKind::FastCgi => {
                let mut body = Vec::new();
                loop {
                    let (record_type, content) = read_fastcgi_test_record(&mut stream).await;
                    if record_type != FCGI_STDIN {
                        continue;
                    }
                    if content.is_empty() {
                        break;
                    }
                    body.extend_from_slice(&content);
                }
                body
            }
            GatewayKind::Uwsgi | GatewayKind::Scgi => {
                match gateway {
                    GatewayKind::Uwsgi => read_uwsgi_header(&mut stream).await,
                    GatewayKind::Scgi => read_scgi_header(&mut stream).await,
                    GatewayKind::FastCgi => unreachable!(),
                }
                let mut body = [0_u8; 8];
                stream.read_exact(&mut body).await.unwrap();
                body.to_vec()
            }
        };
        match reply {
            GatewayReply::Response(response) => match gateway {
                GatewayKind::FastCgi => {
                    write_fastcgi_record(&mut stream, FCGI_STDOUT, response)
                        .await
                        .unwrap();
                    write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
                        .await
                        .unwrap();
                    stream.flush().await.unwrap();
                }
                GatewayKind::Uwsgi | GatewayKind::Scgi => {
                    stream.write_all(response).await.unwrap();
                    stream.shutdown().await.unwrap();
                }
            },
            GatewayReply::Failure(GatewayFailure::Timeout) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = stream.shutdown().await;
            }
            GatewayReply::Failure(GatewayFailure::InvalidHeader) => match gateway {
                GatewayKind::FastCgi => {
                    write_fastcgi_record(&mut stream, FCGI_END_REQUEST, &[0; 8])
                        .await
                        .unwrap();
                    stream.flush().await.unwrap();
                }
                GatewayKind::Uwsgi | GatewayKind::Scgi => {
                    stream.write_all(b"Status: bogus\r\n\r\n").await.unwrap();
                    stream.shutdown().await.unwrap();
                }
            },
        }
        body
    })
}

fn serve_raw_gateway(
    listener: TcpListener,
    gateway: GatewayKind,
    response: &'static [u8],
) -> tokio::task::JoinHandle<Vec<u8>> {
    serve_gateway(listener, gateway, GatewayReply::Response(response))
}
