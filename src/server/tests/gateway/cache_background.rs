use super::*;

#[tokio::test]
async fn uwsgi_background_update_returns_stale_and_refreshes_cache() {
    run_background_update("uwsgi", 18156).await;
}

#[tokio::test]
async fn scgi_background_update_returns_stale_and_refreshes_cache() {
    run_background_update("scgi", 18157).await;
}

async fn run_background_update(kind: &str, listen_port: u16) {
    let kind = kind.to_owned();
    let Some(upstream) = bind_gateway_upstream(&kind).await else {
        return;
    };
    let upstream_address = upstream.local_addr().unwrap();
    let (update_started, update_started_rx) = tokio::sync::oneshot::channel();
    let (release_update, release_update_rx) = tokio::sync::oneshot::channel();
    let backend_kind = kind.clone();
    let backend = tokio::spawn(async move {
        let (mut stream, _) = upstream.accept().await.unwrap();
        read_gateway_request(&backend_kind, &mut stream).await;
        write_gateway_response(&mut stream, "old").await;

        let (mut stream, _) = upstream.accept().await.unwrap();
        let params = read_gateway_request(&backend_kind, &mut stream).await;
        assert_eq!(params.get("REQUEST_URI").map(String::as_str), Some("/item"));
        update_started.send(()).unwrap();
        release_update_rx.await.unwrap();
        write_gateway_response(&mut stream, "new").await;
    });
    let (prefix, config) = test_config(
        &gateway_config(&kind, &upstream_address.to_string(), listen_port),
        &[],
    );
    let state = test_state(&config);
    let request = "GET /item HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";

    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Cache-Status: MISS\r\n"), "{first}");
    assert!(first.ends_with("old"), "{first}");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let stale_config = config.clone();
    let stale_state = state.clone();
    let stale =
        tokio::spawn(
            async move { request_body_with_state(&stale_config, stale_state, request).await },
        );
    update_started_rx.await.unwrap();
    let stale = tokio::time::timeout(Duration::from_secs(1), stale)
        .await
        .expect("stale response should not wait for the background update")
        .unwrap();
    assert!(stale.contains("X-Cache-Status: STALE\r\n"), "{stale}");
    assert!(stale.ends_with("old"), "{stale}");

    let updating = request_body_with_state(&config, state.clone(), request).await;
    assert!(
        updating.contains("X-Cache-Status: UPDATING\r\n"),
        "{updating}"
    );
    assert!(updating.ends_with("old"), "{updating}");

    release_update.send(()).unwrap();
    backend.await.unwrap();

    let mut last = String::new();
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        last = request_body_with_state(&config, state.clone(), request).await;
        if last.contains("X-Cache-Status: HIT\r\n") {
            break;
        }
    }
    assert!(last.contains("X-Cache-Status: HIT\r\n"), "{last}");
    assert!(last.ends_with("new"), "{last}");

    let _ = std::fs::remove_dir_all(prefix);
}

async fn bind_gateway_upstream(kind: &str) -> Option<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {kind} background cache test: binding TCP is not permitted");
            None
        }
        Err(error) => panic!("bind {kind} upstream: {error}"),
    }
}

async fn read_gateway_request(
    kind: &str,
    stream: &mut tokio::net::TcpStream,
) -> HashMap<String, String> {
    let data = match kind {
        "uwsgi" => {
            let mut header = [0_u8; 4];
            stream.read_exact(&mut header).await.unwrap();
            let length = u16::from_le_bytes([header[1], header[2]]) as usize;
            let mut params = vec![0_u8; length];
            stream.read_exact(&mut params).await.unwrap();
            params
        }
        "scgi" => {
            let mut length = Vec::new();
            loop {
                let mut byte = [0_u8; 1];
                stream.read_exact(&mut byte).await.unwrap();
                if byte[0] == b':' {
                    break;
                }
                length.push(byte[0]);
            }
            let length = std::str::from_utf8(&length)
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let mut params = vec![0_u8; length];
            stream.read_exact(&mut params).await.unwrap();
            let mut comma = [0_u8; 1];
            stream.read_exact(&mut comma).await.unwrap();
            assert_eq!(comma, [b',']);
            params
        }
        _ => panic!("unsupported gateway {kind}"),
    };
    parse_params(&data)
}

fn parse_params(data: &[u8]) -> HashMap<String, String> {
    let parts = data.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut params = HashMap::new();
    for index in (0..parts.len() - 1).step_by(2) {
        params.insert(
            String::from_utf8(parts[index].to_vec()).unwrap(),
            String::from_utf8(parts[index + 1].to_vec()).unwrap(),
        );
    }
    params
}

async fn write_gateway_response(stream: &mut tokio::net::TcpStream, body: &str) {
    let headers = if body == "new" {
        "Cache-Control: max-age=60\r\n"
    } else {
        ""
    };
    stream
        .write_all(
            format!(
                "Status: 200 OK\r\nContent-Type: text/plain\r\n{headers}Content-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    stream.shutdown().await.unwrap();
}

fn gateway_config(kind: &str, upstream: &str, listen_port: u16) -> String {
    format!(
        r#"
            worker_processes 1;
            events {{ worker_connections 64; }}
            http {{
                {kind}_cache_path cache keys_zone=background_cache:1m;
                server {{
                    listen 127.0.0.1:{listen_port};
                    add_header X-Cache-Status $upstream_cache_status always;
                    location / {{
                        {kind}_pass {upstream};
                        {kind}_param REQUEST_METHOD $request_method;
                        {kind}_param REQUEST_URI $request_uri;
                        {kind}_cache background_cache;
                        {kind}_cache_key "$request_method$host$request_uri";
                        {kind}_cache_valid 200 1s;
                        {kind}_cache_use_stale updating;
                        {kind}_cache_background_update on;
                        {kind}_cache_lock on;
                    }}
                }}
            }}
        "#
    )
}
