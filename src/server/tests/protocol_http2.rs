use super::support::*;
use super::*;

mod access_log;
mod concurrency;
mod framing;
mod lifecycle;
mod request_body;
mod request_body_cancellation;
mod request_body_content_length;
mod request_body_gateway_reset;
mod request_body_h2_upstream_reset;
mod request_retry_cancel;
mod response_reset;
mod streaming;

pub(super) async fn bind_test_tcp(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

#[tokio::test]
async fn serves_http2_prior_knowledge_on_plaintext_listener() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18460 http2;
                server_name localhost;
                add_header X-Protocol h2 always;
                location = /h2 {
                    return 201 "$request_method:$scheme:$https:$server_protocol\n";
                }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    assert!(!group.ssl);
    assert!(group.http2);
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45682".parse().unwrap();
    let (client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    let (mut sender, connection) = h2::client::handshake(client).await.unwrap();
    let driver = tokio::spawn(async move {
        connection.await.unwrap();
    });
    let request = http::Request::builder()
        .method("GET")
        .uri("http://localhost/h2")
        .body(())
        .unwrap();
    let (response, _) = sender.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.version(), http::Version::HTTP_2);
    assert!(response.headers().get("connection").is_none());
    assert_eq!(
        response
            .headers()
            .get("x-protocol")
            .and_then(|value| value.to_str().ok()),
        Some("h2")
    );
    let body = read_h2_response_body(response.into_body()).await;
    assert_eq!(body, b"GET:http::HTTP/2.0\n");

    drop(sender);
    driver.abort();
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .unwrap()
        .unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn falls_back_to_http1_on_http2_server_directive() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18461;
                http2 on;
                location / { return 200 "plain-http1\n"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    assert!(!group.ssl);
    assert!(group.http2);
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45683".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let server_task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
        group,
        state,
    ));

    client
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    let mut response = Vec::new();
    if let Err(err) = client.read_to_end(&mut response).await {
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("plain-http1\n"), "{response}");

    server_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn read_h2_response_body(mut body: h2::RecvStream) -> Vec<u8> {
    let mut output = Vec::new();
    while let Some(data) = body.data().await {
        let data = data.unwrap();
        body.flow_control().release_capacity(data.len()).unwrap();
        output.extend_from_slice(&data);
    }
    output
}
