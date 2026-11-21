use super::*;

#[tokio::test]
async fn real_ip_header_proxy_protocol_uses_the_proxy_source_address() {
    let (prefix, config) = test_config(
        r#"
                worker_processes 1;
                events { worker_connections 64; }
                http {
                    server {
                        listen 127.0.0.1:18448 proxy_protocol;
                        set_real_ip_from 127.0.0.0/8;
                        real_ip_header proxy_protocol;
                        location / {
                            return 200 "$remote_addr|$realip_remote_addr\n";
                        }
                    }
                }
            "#,
        &[],
    );
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let local = group.address.clone();
    let transport_remote: SocketAddr = "127.0.0.1:45679".parse().unwrap();
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let task = tokio::spawn(handle_http_connection(
        server,
        ConnectionAddresses::direct(ListenAddress::Inet(transport_remote), local),
        group,
        state,
    ));
    client
        .write_all(
            b"PROXY TCP4 203.0.113.7 192.0.2.1 45678 80\r\n\
                  GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let mut response = Vec::new();
    client.read_to_end(&mut response).await.unwrap();
    let response = String::from_utf8(response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(response.ends_with("203.0.113.7|127.0.0.1\n"), "{response}");
    task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
