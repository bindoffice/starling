use super::support::*;
use super::*;

const BODY: &[u8] = b"partmore";

#[tokio::test]
async fn auth_request_preserves_file_backed_request_body_for_the_main_upstream() {
    run_auth_request_body(true).await;
}

#[tokio::test]
async fn denied_auth_request_cleans_up_file_backed_request_body() {
    run_auth_request_body(false).await;
}

async fn run_auth_request_body(allowed: bool) {
    let Some(main_listener) = bind_listener("auth_request main backend").await else {
        return;
    };
    let main_address = main_listener.local_addr().unwrap();
    let main_backend = tokio::spawn(async move {
        if !allowed {
            let accepted =
                tokio::time::timeout(Duration::from_millis(300), main_listener.accept()).await;
            assert!(
                accepted.is_err(),
                "denied auth_request unexpectedly reached the main upstream"
            );
            return;
        }
        let (mut stream, _) = main_listener.accept().await.unwrap();
        let request = read_request(&mut stream).await;
        assert!(
            request.starts_with(b"POST /main HTTP/1.1\r\n"),
            "{request:?}"
        );
        assert!(request.ends_with(BODY), "{request:?}");
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });

    let port = if allowed { 18898 } else { 18899 };
    let auth_status = if allowed { 204 } else { 401 };
    let (prefix, config) = test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    client_body_buffer_size 4;
                    client_body_temp_path body_temp;
                    server {{
                        listen 127.0.0.1:{port};
                        location = /auth {{
                            internal;
                            return {auth_status};
                        }}
                        location /main {{
                            auth_request /auth;
                            proxy_pass http://{main_address};
                        }}
                    }}
                }}
            "#
        ),
        &[],
    );

    let response = request_bytes(
        &config,
        "POST /main HTTP/1.1\r\nHost: localhost\r\nContent-Length: 8\r\n\
         Connection: close\r\n\r\npartmore",
    )
    .await;
    let response_text = String::from_utf8_lossy(&response);
    if allowed {
        assert!(
            response_text.starts_with("HTTP/1.1 200 OK\r\n"),
            "{response_text}"
        );
        assert!(response_text.ends_with("ok"), "{response_text}");
    } else {
        assert!(
            response_text.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
            "{response_text}"
        );
    }

    main_backend.await.unwrap();
    wait_for_temp_cleanup(&prefix.join("body_temp")).await;
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "file-backed request body was not removed after auth_request handling"
    );
    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label} test: TCP binding is not permitted");
            None
        }
        Err(error) => panic!("bind {label}: {error}"),
    }
}

async fn wait_for_temp_cleanup(path: &Path) {
    for _ in 0..100 {
        if !contains_generated_temp_file(path) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before headers");
        request.extend_from_slice(&buffer[..read]);
    };
    let headers = std::str::from_utf8(&request[..header_end]).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .unwrap_or(0);
    while request.len() < header_end + length {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "upstream request closed before the complete body");
        request.extend_from_slice(&buffer[..read]);
    }
    request
}
