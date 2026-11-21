use super::*;
use crate::http::RequestHead;

#[tokio::test]
async fn http1_body_task_treats_zero_limit_as_unlimited() {
    let (mut client, server) = tokio::io::duplex(64 * 1024);
    let (read_half, write_half) = tokio::io::split(server);
    let head = RequestHead {
        method: "POST".into(),
        target: "/".into(),
        version: "HTTP/1.1".into(),
        headers: Vec::new(),
        invalid_headers: Vec::new(),
        keep_alive: false,
        content_length: Some(4),
        chunked: false,
        expect_continue: false,
    };
    let task = Http1BodyTask::spawn(BufReader::new(read_half), &head, Duration::from_secs(1), 0)
        .expect("content-length body task");
    let stream = task.stream();

    client.write_all(b"part").await.unwrap();
    let data = stream.next().await.unwrap().expect("request body data");
    assert_eq!(&data[..], b"part");
    assert!(stream.next().await.unwrap().is_none());
    let (_, result) = task.finish().await.unwrap();
    result.unwrap();
    drop(write_half);
}

#[tokio::test]
async fn h2_body_task_treats_zero_limit_as_unlimited() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn(request.into_body(), Duration::from_secs(1), 0);
        let stream = task.stream();

        let first = next_unlimited_h2_chunk(&stream, &mut connection).await;
        assert_eq!(&first[..], b"part");
        let rest = next_unlimited_h2_chunk(&stream, &mut connection).await;
        assert_eq!(&rest[..], b"remain");
        assert!(stream.next().await.unwrap().is_none());
        task.finish().await.unwrap();

        let response = http::Response::builder().status(200).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::from_static(b"ok"), true).unwrap();
        connection.graceful_shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            while connection.accept().await.is_some() {}
        })
        .await;
    });

    let (mut sender, driver) = h2::client::handshake(client_io).await.unwrap();
    let driver = tokio::spawn(driver);
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/upload")
        .body(())
        .unwrap();
    let (response, mut body) = sender.send_request(request, false).unwrap();
    body.send_data(Bytes::from_static(b"part"), false).unwrap();
    body.send_data(Bytes::from_static(b"remain"), true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("HTTP/2 response should complete")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

async fn next_unlimited_h2_chunk(
    stream: &crate::http::RequestBodyStream,
    connection: &mut h2::server::Connection<tokio::io::DuplexStream, Bytes>,
) -> Bytes {
    let next = stream.next();
    tokio::pin!(next);
    loop {
        tokio::select! {
            data = &mut next => return data.unwrap().expect("HTTP/2 request body data"),
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "HTTP/2 connection closed before request body data");
            }
        }
    }
}
