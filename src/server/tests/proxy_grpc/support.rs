use super::*;

pub(super) async fn bind_grpc_listener(label: &str) -> Option<TcpListener> {
    match TcpListener::bind("127.0.0.1:0").await {
        Ok(listener) => Some(listener),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            eprintln!("skipping {label}: binding TCP sockets is not permitted");
            None
        }
        Err(error) => panic!("bind gRPC listener: {error}"),
    }
}

pub(super) async fn serve_grpc_response(
    listener: TcpListener,
    status: u16,
    body: &'static [u8],
    grpc_status: &'static str,
) {
    let _ = serve_grpc_response_with_body(listener, status, body, grpc_status).await;
}

pub(super) async fn serve_grpc_response_with_body(
    listener: TcpListener,
    status: u16,
    body: &'static [u8],
    grpc_status: &'static str,
) -> Vec<u8> {
    serve_grpc_reply(
        listener,
        GrpcTestReply::Response {
            status,
            body,
            grpc_status,
        },
    )
    .await
}

#[derive(Clone, Copy)]
pub(super) enum GrpcTestReply {
    Response {
        status: u16,
        body: &'static [u8],
        grpc_status: &'static str,
    },
    Delayed(Duration),
    NoResponse,
}

pub(super) async fn serve_grpc_reply(listener: TcpListener, reply: GrpcTestReply) -> Vec<u8> {
    let (stream, _) = listener.accept().await.unwrap();
    let mut connection = h2::server::handshake(stream).await.unwrap();
    let (request, mut respond) = connection.accept().await.unwrap().unwrap();
    let work = async move {
        let mut request_body = request.into_body();
        let mut received = Vec::new();
        while let Some(chunk) = request_body.data().await {
            received.extend_from_slice(&chunk.unwrap());
        }
        match reply {
            GrpcTestReply::Response {
                status,
                body,
                grpc_status,
            } => {
                let response = http::Response::builder()
                    .status(status)
                    .header("Content-Type", "application/grpc")
                    .body(())
                    .unwrap();
                let mut stream = respond.send_response(response, false).unwrap();
                stream.send_data(Bytes::from_static(body), false).unwrap();
                let mut trailers = http::HeaderMap::new();
                trailers.insert("grpc-status", http::HeaderValue::from_static(grpc_status));
                stream.send_trailers(trailers).unwrap();
            }
            GrpcTestReply::Delayed(delay) => {
                tokio::time::sleep(delay).await;
                drop(respond);
            }
            GrpcTestReply::NoResponse => drop(respond),
        }
        received
    };
    tokio::pin!(work);
    let received = loop {
        tokio::select! {
            received = &mut work => break received,
            accepted = connection.accept() => {
                if accepted.is_none() {
                    break work.await;
                }
            }
        }
    };
    shutdown_grpc_h2(connection).await;
    received
}

pub(super) async fn shutdown_grpc_h2(
    mut connection: h2::server::Connection<impl AsyncRead + AsyncWrite + Unpin, Bytes>,
) {
    connection.graceful_shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while connection.accept().await.is_some() {}
    })
    .await;
}
