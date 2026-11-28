use super::*;

#[tokio::test]
async fn streams_value_body_before_end_trailer() {
    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18149;
                    location / {
                        set $memcached_key item;
                        memcached_pass 127.0.0.1:11211;
                    }
                }
            }
        "#,
        &[],
    );
    let server = &config.http.as_ref().unwrap().servers[0];
    let context = &server.locations[0].context;
    let request = Request {
        method: "GET".into(),
        target: "/item".into(),
        version: "HTTP/1.1".into(),
        headers: (vec![Header {
            name: "Host".into(),
            value: "localhost".into(),
        }])
        .into(),
        body_stream: None,
        body: Vec::new(),
        remote_addr: "127.0.0.1:45678".into(),
        local_addr: "127.0.0.1:18149".into(),
        keep_alive: false,
    };
    let remote: SocketAddr = "127.0.0.1:45678".parse().unwrap();
    let local: SocketAddr = "127.0.0.1:18149".parse().unwrap();
    let variables = Variables::new(&request, "localhost", server, &local, &remote);
    let key = memcached_upstream_pool_key("127.0.0.1", 11211, context, &variables);
    let (client, mut backend) = tokio::io::duplex(64 * 1024);
    let connection = UpstreamConnection::new(Box::new(client), key);
    let (continue_tx, continue_rx) = tokio::sync::oneshot::channel();
    let backend_task = tokio::spawn(async move {
        let mut line = Vec::new();
        let mut reader = BufReader::new(&mut backend);
        reader.read_until(b'\n', &mut line).await.unwrap();
        assert_eq!(line, b"get item\r\n");
        reader
            .get_mut()
            .write_all(b"VALUE item 0 11\r\nfirst")
            .await
            .unwrap();
        reader.get_mut().flush().await.unwrap();
        continue_rx.await.unwrap();
        reader
            .get_mut()
            .write_all(b"second\r\nEND\r\n")
            .await
            .unwrap();
        reader.get_mut().flush().await.unwrap();
    });

    let exchange = memcached_exchange_connection_streaming(
        connection,
        b"item",
        &request,
        context,
        None,
        0,
        Arc::new(UpstreamKeepalivePool::default()),
    )
    .await
    .unwrap();
    let MemcachedExchange::Streamed { body, gzip } = exchange else {
        panic!("memcached value should be streamed");
    };
    assert!(!gzip);
    let ResponseBody::Stream(mut body) = body else {
        panic!("memcached response body should be a stream");
    };
    assert_eq!(body.length(), Some(11));
    assert_eq!(next_stream_bytes(&mut body, 5).await, b"first");
    continue_tx.send(()).unwrap();
    assert_eq!(next_stream_bytes(&mut body, 6).await, b"second");
    assert!(body.next().await.unwrap().is_none());
    backend_task.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

async fn next_data(stream: &mut ResponseStream) -> Vec<u8> {
    match tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("memcached body timed out")
        .unwrap()
    {
        Some(ResponseStreamItem::Data(data)) => data.to_vec(),
        Some(ResponseStreamItem::Trailers(_)) => panic!("unexpected memcached trailers"),
        None => panic!("memcached stream ended before data"),
    }
}

async fn next_stream_bytes(stream: &mut ResponseStream, length: usize) -> Vec<u8> {
    let mut body = Vec::with_capacity(length);
    while body.len() < length {
        body.extend(next_data(stream).await);
    }
    assert_eq!(body.len(), length);
    body
}
