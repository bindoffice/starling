use tokio::net::TcpStream;

use super::support::{RunningServer, TestDirectory, reserve_port};

#[tokio::test]
#[ignore = "requires local TCP bind; run with --ignored"]
async fn serves_concurrent_prior_knowledge_http2_streams() {
    let port = reserve_port();
    let directory = TestDirectory::new("http2");
    let config = http2_config(port);
    let _server = RunningServer::start(directory, port, &config);

    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to HTTP/2 listener");
    let (mut client, connection) = h2::client::handshake(stream)
        .await
        .expect("HTTP/2 handshake");
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let first_request = http::Request::builder()
        .method("GET")
        .uri(format!("http://localhost:{port}/h2/one"))
        .body(())
        .unwrap();
    let second_request = http::Request::builder()
        .method("GET")
        .uri(format!("http://localhost:{port}/h2/two"))
        .body(())
        .unwrap();
    let (first_response, _) = client
        .send_request(first_request, true)
        .expect("send first HTTP/2 request");
    let (second_response, _) = client
        .send_request(second_request, true)
        .expect("send second HTTP/2 request");
    let (first_response, second_response) = tokio::join!(first_response, second_response);
    let first_response = first_response.expect("first HTTP/2 response headers");
    let second_response = second_response.expect("second HTTP/2 response headers");
    assert_eq!(first_response.status(), http::StatusCode::OK);
    assert_eq!(second_response.status(), http::StatusCode::OK);

    let first_body = read_body(first_response.into_body()).await;
    let second_body = read_body(second_response.into_body()).await;
    assert_eq!(first_body, "http2-stream-one", "first HTTP/2 body");
    assert_eq!(second_body, "http2-stream-two", "second HTTP/2 body");
}

#[tokio::test]
#[ignore = "requires local TCP bind; run with --ignored"]
async fn sigquit_gracefully_closes_http2_with_goaway() {
    let port = reserve_port();
    let directory = TestDirectory::new("http2-sigquit");
    let config = http2_config(port);
    let mut server = RunningServer::start(directory, port, &config);

    let stream = TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to HTTP/2 listener");
    let (mut client, connection) = h2::client::handshake(stream)
        .await
        .expect("HTTP/2 handshake");
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });

    let request = http::Request::builder()
        .uri(format!("http://localhost:{port}/h2/one"))
        .body(())
        .unwrap();
    let (response, _) = client
        .send_request(request, true)
        .expect("send initial HTTP/2 request");
    let response = tokio::time::timeout(std::time::Duration::from_secs(2), response)
        .await
        .expect("initial HTTP/2 response should arrive")
        .expect("initial HTTP/2 response headers");
    assert_eq!(response.status(), http::StatusCode::OK);
    assert_eq!(read_body(response.into_body()).await, "http2-stream-one");

    server.shutdown_gracefully();
    server.wait_for_exit(std::time::Duration::from_secs(5));

    let request = http::Request::builder()
        .uri(format!("http://localhost:{port}/h2/two"))
        .body(())
        .unwrap();
    let rejected = match client.send_request(request, true) {
        Err(_) => true,
        Ok((response, _)) => tokio::time::timeout(std::time::Duration::from_secs(1), response)
            .await
            .map(|response| response.is_err())
            .unwrap_or(true),
    };
    assert!(
        rejected,
        "new HTTP/2 streams must be rejected after SIGQUIT GOAWAY"
    );

    drop(client);
    driver.abort();
}

async fn read_body(mut body: h2::RecvStream) -> String {
    let mut received = Vec::new();
    while let Some(chunk) = body.data().await {
        received.extend_from_slice(&chunk.expect("HTTP/2 response body"));
    }
    String::from_utf8(received).unwrap()
}

fn http2_config(port: u16) -> String {
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
                    listen 127.0.0.1:{port} http2;
                    server_name localhost;

                    location = /h2/one {{
                        return 200 "http2-stream-one";
                    }}

                    location = /h2/two {{
                        return 200 "http2-stream-two";
                    }}
                }}
            }}
        "#
    )
}
