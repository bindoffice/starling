use super::super::*;
use super::common::{connect, outgoing, proxy_config};

#[tokio::test]
async fn buffered_http2_body_releases_flow_control_above_the_initial_window() {
    let (client_io, server_io) = tokio::io::duplex(256 * 1024);
    let total = 96 * 1024;
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/large-buffered");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Length", total)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; total]), true)
            .unwrap();
        shutdown_h2(connection).await;
    });

    let (prefix, config) = proxy_config(
        18752,
        "proxy_buffers 24 8k; \
         proxy_buffer_size 8k; \
         proxy_busy_buffers_size 184k; \
         proxy_temp_file_write_size 16k;",
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/large-buffered");
    let (response, _) = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true)
        .await
        .unwrap();
    let body = response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .unwrap();
    assert_eq!(body.len(), total);
    assert!(body.iter().all(|byte| *byte == b'a'));

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn buffered_http2_body_uses_continuous_read_progress_timeout() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let total = 12 * 1024;
    let chunk = 4 * 1024;
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/trickle-buffered");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Length", total)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        let work = async move {
            let mut sent = 0;
            while sent < total {
                tokio::time::sleep(Duration::from_millis(30)).await;
                sent += chunk;
                stream
                    .send_data(Bytes::from(vec![b'b'; chunk]), sent == total)
                    .unwrap();
            }
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

    let (prefix, config) = proxy_config(
        18753,
        "proxy_buffers 8 4k; \
         proxy_buffer_size 4k; \
         proxy_busy_buffers_size 28k; \
         proxy_read_timeout 80ms;",
    );
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/trickle-buffered");
    let (response, _) = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true)
        .await
        .unwrap();
    let body = response
        .body
        .into_memory(MAX_BODY_BYTES as u64)
        .await
        .unwrap();
    assert_eq!(body.len(), total);
    assert!(body.iter().all(|byte| *byte == b'b'));

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn maps_buffered_prefetch_timeout_to_proxy_timeout() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/timeout");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Length", 12 * 1024)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; 1024]), false)
            .unwrap();
        let _ = wait_for_release.await;
        drop(stream);
        shutdown_h2(connection).await;
    });

    let (prefix, config) = proxy_config(18746, "proxy_read_timeout 100ms;");
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/timeout");
    let result = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true).await;
    assert!(matches!(result, Err(ProxyFailure::Timeout)));

    drop(connection);
    release.send(()).unwrap();
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn rejects_content_length_truncation_before_threshold() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/truncated");
        let response = http::Response::builder()
            .status(200)
            .header("Content-Length", 12 * 1024)
            .body(())
            .unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; 1024]), true)
            .unwrap();
        shutdown_h2(connection).await;
    });

    let (prefix, config) = proxy_config(18747, "");
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/truncated");
    let result = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true).await;
    assert!(
        matches!(
            result,
            Err(ProxyFailure::Failure | ProxyFailure::InvalidHeader)
        ),
        "truncated HTTP/2 response should fail before response commit"
    );

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
