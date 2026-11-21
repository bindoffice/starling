use super::*;

#[tokio::test]
async fn streams_http2_request_data_before_the_client_finishes() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (first_chunk_tx, first_chunk_rx) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/upload");
        let mut body = request.into_body();

        let first = {
            let next = body.data();
            tokio::pin!(next);
            loop {
                tokio::select! {
                    data = &mut next => break data.unwrap().unwrap(),
                    accepted = connection.accept() => {
                        assert!(accepted.is_some(), "connection closed before request data");
                    }
                }
            }
        };
        assert_eq!(&first[..], b"part");
        first_chunk_tx.send(()).unwrap();

        let rest = {
            let next = body.data();
            tokio::pin!(next);
            loop {
                tokio::select! {
                    data = &mut next => break data.unwrap().unwrap(),
                    accepted = connection.accept() => {
                        assert!(accepted.is_some(), "connection closed before request body ended");
                    }
                }
            }
        };
        assert_eq!(&rest[..], b"remain");

        let response = http::Response::builder().status(200).body(()).unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream.send_data(Bytes::from_static(b"ok"), true).unwrap();
        shutdown_h2(connection).await;
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    listen 127.0.0.1:18720;
                    location / {
                        proxy_pass http://backend.test/;
                        proxy_http_version 2;
                        proxy_request_buffering off;
                        proxy_read_timeout 5s;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/upload".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Content-Length".into(), "10".into()),
        ],
        body: Vec::new(),
        body_stream: Some(stream),
        body_chunked: false,
        body_limit: 10,
    };
    let mut connection = ProxyV2Connection::connect(
        Box::new(client_io),
        http2_test_key(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let producer = tokio::spawn(async move {
        sender.send(Bytes::from_static(b"part")).await;
        first_chunk_rx.await.unwrap();
        sender.send(Bytes::from_static(b"remain")).await;
    });
    let (response, _) = tokio::time::timeout(
        Duration::from_secs(2),
        timed_proxy_v2_exchange(&mut connection, &outgoing, proxy, context, 0, true),
    )
    .await
    .expect("HTTP/2 request body should stream before the producer finishes")
    .unwrap();
    producer.await.unwrap();
    assert_eq!(response.status, 200);
    assert_eq!(
        response
            .body
            .into_memory(MAX_BODY_BYTES as u64)
            .await
            .unwrap(),
        b"ok"
    );
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
