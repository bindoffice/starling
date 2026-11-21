use super::*;
use crate::http::RequestBodyStream;

#[tokio::test]
async fn truncated_file_backed_request_body_aborts_http2_upstream_with_500() {
    let (client_io, server_io) = tokio::io::duplex(16 * 1024);
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, _respond) = connection.accept().await.unwrap().unwrap();
        let work = async move {
            let mut body = request.into_body();
            let mut received = Vec::new();
            while let Some(chunk) = body.data().await {
                match chunk {
                    Ok(chunk) => received.extend_from_slice(&chunk),
                    Err(_) => break,
                }
            }
            received
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                received = &mut work => break received,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        break work.await;
                    }
                }
            }
        }
    });

    let (prefix, config) = test_config(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                client_body_buffer_size 4;
                client_body_temp_path body_temp;
                server {
                    listen 127.0.0.1:18598;
                    location / {
                        proxy_pass http://backend.test/;
                        proxy_http_version 2;
                    }
                }
            }
        "#,
        &[],
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let (sender, stream) = RequestBodyStream::channel();
    sender.send(Bytes::from_static(b"part")).await;
    sender.send(Bytes::from_static(b"more")).await;
    drop(sender);
    let spooled = crate::http::spool_request_body(
        Some(&stream),
        true,
        4,
        &context.client_body.temp_path,
        context.client_body.in_file_only,
        0,
    )
    .await
    .unwrap();
    let file = spooled.file.expect("file-backed request body");
    std::fs::write(file.path(), b"part").unwrap();
    let body_stream = RequestBodyStream::from_file(file.clone());
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/call".into(),
        headers: vec![("Host".into(), "backend.test".into())],
        body: Vec::new(),
        body_stream: Some(body_stream),
        body_chunked: false,
        body_limit: 0,
    };
    let mut connection = ProxyV2Connection::connect(
        Box::new(client_io),
        http2_test_key(),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    let failure =
        match timed_proxy_v2_exchange(&mut connection, &outgoing, proxy, context, 0, false).await {
            Ok(_) => panic!("truncated file-backed request body must fail"),
            Err(failure) => failure,
        };
    assert!(matches!(failure, ProxyFailure::RequestBody(500)));
    let received = backend.await.unwrap();
    assert_eq!(received, b"part");

    drop(outgoing);
    drop(file);
    assert!(
        !contains_generated_temp_file(&prefix.join("body_temp")),
        "truncated file-backed HTTP/2 request body should be removed"
    );
    let _ = std::fs::remove_dir_all(prefix);
}
