use super::super::*;
use super::common::{
    DEFAULT_BUFFER_SIZE, collect_stream, connect, has_trailer, next_data, outgoing, proxy_config,
};

#[tokio::test]
async fn preserves_trailers_on_prefetched_streaming_response() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let total = 12 * 1024;
    let first = 9 * 1024;
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/trailers");
        let response = http::Response::builder().status(200).body(()).unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from(vec![b'a'; first]), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from(vec![b'b'; total - first]), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("x-stream-trailer", http::HeaderValue::from_static("yes"));
            stream.send_trailers(trailers).unwrap();
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

    let (prefix, config) = proxy_config(18744, "");
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/trailers");
    let (response, _) = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true)
        .await
        .unwrap();
    let ResponseBody::Stream(mut body) = response.body else {
        panic!("large response with trailers should be streamed");
    };
    let mut received = next_data(&mut body).await;
    assert_eq!(received.len(), DEFAULT_BUFFER_SIZE);

    release.send(()).unwrap();
    let (rest, trailers) = collect_stream(&mut body).await;
    received.extend_from_slice(&rest);
    assert_eq!(received.len(), total);
    assert!(has_trailer(&trailers, "x-stream-trailer", "yes"));

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn preserves_trailers_on_small_complete_buffered_response() {
    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let (release, wait_for_release) = tokio::sync::oneshot::channel();
    let backend = tokio::spawn(async move {
        let mut connection = h2::server::handshake(server_io).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/small-trailers");
        let response = http::Response::builder().status(200).body(()).unwrap();
        let mut stream = respond.send_response(response, false).unwrap();
        stream
            .send_data(Bytes::from_static(b"first"), false)
            .unwrap();
        let work = async move {
            wait_for_release.await.unwrap();
            stream
                .send_data(Bytes::from_static(b"second"), false)
                .unwrap();
            let mut trailers = http::HeaderMap::new();
            trailers.insert("x-buffered-trailer", http::HeaderValue::from_static("yes"));
            stream.send_trailers(trailers).unwrap();
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

    let (prefix, config) = proxy_config(18745, "");
    let context = &config.http.as_ref().unwrap().servers[0].locations[0].context;
    let proxy = context.proxy_pass.as_ref().unwrap();
    let mut connection = connect(client_io).await;
    let request = outgoing("/small-trailers");
    let response = {
        let exchange = timed_proxy_v2_exchange(&mut connection, &request, proxy, context, 0, true);
        tokio::pin!(exchange);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), &mut exchange)
                .await
                .is_err(),
            "small response should wait for trailers before completion"
        );

        release.send(()).unwrap();
        exchange.await.unwrap().0
    };
    assert_eq!(
        response.body.into_memory(1024).await.unwrap(),
        b"firstsecond"
    );
    assert!(has_trailer(&response.trailers, "x-buffered-trailer", "yes"));

    drop(connection);
    backend.await.unwrap();
    let _ = std::fs::remove_dir_all(prefix);
}
