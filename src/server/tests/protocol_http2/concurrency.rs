use super::super::support::*;
use super::super::*;
use super::read_h2_response_body;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_repeated_concurrent_http2_streams_on_one_connection() {
    let source = r#"
        worker_processes 1;
        events { worker_connections 64; }
        http {
            server {
                listen 127.0.0.1:18464 http2;
                server_name localhost;
                location = /one { return 200 "one"; }
                location = /two { return 200 "two"; }
            }
        }
    "#;
    let (prefix, config) = test_config(source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let remote: SocketAddr = "127.0.0.1:45686".parse().unwrap();
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

    for _ in 0..64 {
        let first = sender
            .send_request(
                http::Request::builder()
                    .uri("http://localhost/one")
                    .body(())
                    .unwrap(),
                true,
            )
            .unwrap();
        let second = sender
            .send_request(
                http::Request::builder()
                    .uri("http://localhost/two")
                    .body(())
                    .unwrap(),
                true,
            )
            .unwrap();
        let (first, second) = tokio::join!(first.0, second.0);
        let first = first.unwrap();
        let second = second.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(read_h2_response_body(first.into_body()).await, b"one");
        assert_eq!(read_h2_response_body(second.into_body()).await, b"two");
    }

    drop(sender);
    driver.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), server_task).await;
    let _ = std::fs::remove_dir_all(prefix);
}
