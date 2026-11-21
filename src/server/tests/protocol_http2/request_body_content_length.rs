use super::*;
use tokio::io::{AsyncRead, AsyncWrite};

#[tokio::test]
async fn h2_body_task_rejects_short_content_length() {
    run_content_length_mismatch(b"part", 5, true).await;
}

#[tokio::test]
async fn h2_body_task_rejects_excess_content_length() {
    run_content_length_mismatch(b"partmore", 5, false).await;
}

async fn run_content_length_mismatch(body: &'static [u8], expected: u64, expect_prefix: bool) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        let task = H2BodyTask::spawn_with_connection_fault_and_length(
            request.into_body(),
            Duration::from_secs(1),
            64 * 1024,
            None,
            Some(expected),
        );
        let stream = task.stream();
        let first = next_body_result(&stream, &mut connection).await;
        if expect_prefix {
            let first = first.expect("accepted body prefix").expect("body prefix");
            assert_eq!(&first[..], b"part");
            let error = next_body_result(&stream, &mut connection)
                .await
                .expect_err("body should fail after the accepted prefix");
            assert_eq!(crate::http::request_body_error_status(&error), Some(400));
        } else {
            let error = first.expect_err("extra body data should be rejected");
            assert_eq!(crate::http::request_body_error_status(&error), Some(400));
        }
        assert!(matches!(stream.next().await, Ok(None)));
        let task_error = task.finish().await.expect_err("body task should fail");
        assert_eq!(
            crate::http::request_body_error_status(&task_error),
            Some(400)
        );

        let response = http::Response::builder().status(400).body(()).unwrap();
        let mut send = respond.send_response(response, false).unwrap();
        send.send_data(Bytes::new(), true).unwrap();
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
    let (response, mut request_body) = sender.send_request(request, false).unwrap();
    request_body
        .send_data(Bytes::from_static(body), true)
        .unwrap();

    let response = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .expect("content-length mismatch response should complete")
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    server.await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(1), driver).await;
}

async fn next_body_result<S>(
    stream: &crate::http::RequestBodyStream,
    connection: &mut h2::server::Connection<S, Bytes>,
) -> io::Result<Option<Bytes>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let next = stream.next();
    tokio::pin!(next);
    loop {
        tokio::select! {
            item = &mut next => return item,
            accepted = connection.accept() => {
                assert!(accepted.is_some(), "connection closed before request body ended");
            }
        }
    }
}
