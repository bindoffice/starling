use super::super::*;

pub(super) const DEFAULT_BUFFER_SIZE: usize = 4 * 1024;

pub(super) fn proxy_config(port: u16, location_directives: &str) -> (PathBuf, Arc<RuntimeConfig>) {
    test_config(
        &format!(
            r#"
                worker_processes 1;
                events {{ worker_connections 64; }}
                http {{
                    server {{
                        listen 127.0.0.1:{port};
                        location / {{
                            proxy_pass http://backend.test/;
                            proxy_http_version 2;
                            proxy_read_timeout 5s;
                            {location_directives}
                        }}
                    }}
                }}
            "#
        ),
        &[],
    )
}

pub(super) async fn connect(client_io: tokio::io::DuplexStream) -> ProxyV2Connection {
    ProxyV2Connection::connect(
        Box::new(client_io),
        http2_test_key(),
        Duration::from_secs(1),
    )
    .await
    .unwrap()
}

pub(super) fn outgoing(path: &str) -> ProxyRequestMessage {
    ProxyRequestMessage {
        method: "GET".into(),
        path: path.into(),
        headers: vec![("Host".into(), "backend.test".into())],
        body: Vec::new(),
        body_stream: None,
        body_chunked: false,
        body_limit: 0,
    }
}

pub(super) async fn next_data(stream: &mut ResponseStream) -> Vec<u8> {
    let item = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("response stream item should arrive")
        .unwrap()
        .expect("response stream ended unexpectedly");
    let ResponseStreamItem::Data(data) = item else {
        panic!("expected response data, got trailers");
    };
    data.to_vec()
}

pub(super) async fn collect_stream(stream: &mut ResponseStream) -> (Vec<u8>, Vec<Header>) {
    let mut body = Vec::new();
    let mut trailers = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(item) = stream.next().await.unwrap() {
            match item {
                ResponseStreamItem::Data(data) => body.extend_from_slice(&data),
                ResponseStreamItem::Trailers(items) => trailers = items,
            }
        }
    })
    .await
    .expect("response stream should complete");
    (body, trailers)
}

pub(super) fn has_trailer(trailers: &[Header], name: &str, value: &str) -> bool {
    trailers.iter().any(|header| {
        header.name.eq_ignore_ascii_case(name) && header.value.eq_ignore_ascii_case(value)
    })
}
