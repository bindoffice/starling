use super::*;

#[tokio::test]
async fn resolves_runtime_ports_and_all_implicit_host_addresses() {
    let listener = match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => listener,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping implicit proxy target test: TCP binding is not permitted");
            return;
        }
        Err(error) => panic!("bind upstream: {error}"),
    };
    let upstream = listener.local_addr().unwrap();
    let backend = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
            }
            let body = format!("upstream={upstream}\n");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18232;
                    location = /variable-port {
                        proxy_pass http://127.0.0.1:$arg_port;
                    }
                    location = /resolved-host {
                        proxy_pass http://localhost:$arg_port;
                    }
                }
            }
        "#,
        &[],
    );

    for path in ["variable-port", "resolved-host"] {
        let response = request_body(
            &config,
            &format!(
                "GET /{path}?port={} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                upstream.port()
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(
            response.ends_with(&format!("upstream={upstream}\n")),
            "{response}"
        );
    }

    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
