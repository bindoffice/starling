use super::*;

#[tokio::test]
async fn writes_final_chunk_and_terminal_block_in_one_batch() {
    let (mut reader, mut writer) = tokio::io::duplex(64 * 1024);
    let (sender, stream) = crate::http::RequestBodyStream::channel();
    let outgoing = ProxyRequestMessage {
        method: "POST".into(),
        path: "/".into(),
        headers: vec![
            ("Host".into(), "backend.test".into()),
            ("Transfer-Encoding".into(), "chunked".into()),
        ],
        body: Vec::new(),
        body_stream: Some(stream),
        body_chunked: true,
        body_limit: 0,
    };
    let task =
        tokio::spawn(async move { write_proxy_request(&mut writer, &outgoing, "1.1").await });

    sender.finish();
    sender.send(Bytes::from_static(b"final")).await;
    drop(sender);
    let mut received = Vec::new();
    read_until(&mut reader, &mut received, b"5\r\nfinal\r\n0\r\n\r\n").await;
    task.await.unwrap().unwrap();
}
