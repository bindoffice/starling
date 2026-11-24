use super::*;

#[tokio::test]
async fn no_error_reset_does_not_discard_a_queued_response() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let _body = request.into_body();

        let response = http::Response::builder()
            .status(413)
            .header("content-length", "9")
            .body(())
            .unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::from_static(b"too large"), true)
            .unwrap();
        // The request body is still open, so this mirrors the response path
        // used when Nginx discards an unread request body.
        send.send_reset(h2::Reason::NO_ERROR);

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
    body.send_data(Bytes::from_static(b"request"), false)
        .unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("the queued response must be delivered before the reset")
        .unwrap();
    assert_eq!(response.status(), 413);

    drop(response);
    drop(body);
    drop(sender);
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}
