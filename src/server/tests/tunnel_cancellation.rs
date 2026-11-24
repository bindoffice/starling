use super::support::*;
use super::*;
use tokio::net::{TcpListener, TcpStream};

#[tokio::test]
async fn client_reset_during_tls_handshake_cancels_the_upstream_connection() {
    let backend = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping tunnel cancellation test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind tunnel cancellation backend: {error}"),
    };
    let backend_address = backend.local_addr().unwrap();
    let frontend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let frontend_address = frontend.local_addr().unwrap();
    let (accepted_tx, accepted_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        let (mut stream, _) = backend.accept().await.unwrap();
        accepted_tx.send(()).unwrap();
        let mut buffer = [0_u8; 1024];
        loop {
            match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer)).await {
                Ok(Ok(0)) | Ok(Err(_)) => break,
                Ok(Ok(_)) => {}
                Err(_) => panic!("client reset did not close the TLS upstream connection"),
            }
        }
    });
    let source = format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                server {{
                    listen {frontend_address};
                    location / {{
                        tunnel_pass https://{backend_address};
                        tunnel_connect_timeout 30s;
                        tunnel_next_upstream off;
                        proxy_ssl_verify off;
                    }}
                }}
            }}
        "#
    );
    let (prefix, config) = test_config(&source, &[]);
    let group = build_listener_groups(&config)[0].clone();
    let state = test_state(&config);
    let server_task = tokio::spawn(async move {
        let (stream, remote) = frontend.accept().await.unwrap();
        let (stream, control) = ClientSocketControl::attach_tcp(stream).unwrap();
        let local = stream
            .local_addr()
            .map(ListenAddress::Inet)
            .unwrap_or_else(|_| group.address.clone());
        handle_io_with_reset(
            stream,
            ConnectionAddresses::direct(ListenAddress::Inet(remote), local),
            group,
            state,
            None,
            control,
        )
        .await;
    });
    let client = TcpStream::connect(frontend_address).await.unwrap();
    client
        .writable()
        .await
        .expect("client socket should become writable");
    let mut client = client;
    client
        .write_all(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n")
        .await
        .unwrap();
    accepted_rx.await.unwrap();
    socket2::SockRef::from(&client)
        .set_linger(Some(Duration::ZERO))
        .unwrap();
    drop(client);

    tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("client reset should stop the tunnel connection")
        .unwrap();
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
