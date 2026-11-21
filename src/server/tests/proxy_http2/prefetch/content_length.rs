use super::super::*;
use super::common::{
    DEFAULT_BUFFER_SIZE, collect_stream, connect, next_data, outgoing, proxy_config,
};

#[tokio::test]
async fn streams_default_buffered_content_length_after_threshold() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let total = 12 * 1024;
    let first = 9 * 1024;
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/large");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", total)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; first]), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from(vec![b'b'; total - first]), true)
                .unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_h2(connection).await;
    });

    let (prefix, config) = proxy_config(18740, "");
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/large");
    let (response, _) = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true)
        .await
        .unwrap();
    let ResponseBody::Stream(mut body) = response.body else {
        panic!("large default-buffered response should be streamed");
    };
    let first_chunk = next_data(&mut body).await;
    assert_eq!(
        first_chunk.len(),
        DEFAULT_BUFFER_SIZE,
        "streamed data must preserve fixed proxy_buffers geometry"
    );
    assert!(first_chunk.iter().all(|byte| *byte == b'a'));

    release.send(()).unwrap();
    let (rest, trailers) = collect_stream(&mut body).await;
    let mut body = first_chunk;
    body.extend_from_slice(&rest);
    assert_eq!(body.len(), total);
    assert!(body[first..].iter().all(|byte| *byte == b'b'));
    assert!(trailers.is_empty());

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn keeps_small_default_buffered_response_until_end() {
    assert_buffering_behavior("", 18741, 10, 5, false).await;
}

#[tokio::test]
async fn explicit_busy_buffers_size_streams_after_threshold() {
    assert_buffering_behavior(
        "proxy_busy_buffers_size 8k;",
        18742,
        40 * 1024,
        9 * 1024,
        true,
    )
    .await;
}

#[tokio::test]
async fn explicit_max_temp_file_size_streams_after_threshold() {
    assert_buffering_behavior(
        "proxy_max_temp_file_size 1m;",
        18743,
        40 * 1024,
        9 * 1024,
        true,
    )
    .await;
}

async fn assert_buffering_behavior(
    directives: &str,
    port: u16,
    total: usize,
    first: usize,
    expect_streaming: bool,
) {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/buffered");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Length", total)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; first]), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from(vec![b'b'; total - first]), true)
                .unwrap();
        };
        tokio::pin!(work);
        loop {
            tokio::select! {
                () = &mut work => break,
                accepted = connection.accept() => {
                    if accepted.is_none() {
                        work.await;
                        break;
                    }
                }
            }
        }
        shutdown_h2(connection).await;
    });

    let (prefix, config) = proxy_config(port, directives);
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/buffered");
    if expect_streaming {
        let (response, _) =
            timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true)
                .await
                .unwrap();
        let ResponseBody::Stream(mut body) = response.body else {
            panic!("large response should be streamed");
        };
        let first_chunk = next_data(&mut body).await;
        assert_eq!(first_chunk.len(), DEFAULT_BUFFER_SIZE);
        release.send(()).unwrap();
        let (rest, _) = collect_stream(&mut body).await;
        assert_eq!(first_chunk.len() + rest.len(), total);
        drop(connection);
        backend.await.unwrap();
        let _ = std::fs::remove_dir_all(prefix);
        return;
    }

    let response = {
        let exchange = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true);
        tokio::pin!(exchange);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut exchange)
                .await
                .is_err(),
            "response reached the client before the complete buffered body"
        );

        release.send(()).unwrap();
        exchange.await.unwrap().0
    };
    assert!(!matches!(response.body, ResponseBody::Stream(_)));
    let body = response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .unwrap();
    assert_eq!(body.len(), total);
    assert!(body[..first].iter().all(|byte| *byte == b'a'));
    assert!(body[first..].iter().all(|byte| *byte == b'b'));

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
