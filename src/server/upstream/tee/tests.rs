use super::*;

mod backpressure;
mod failure_paths;

pub(super) async fn collect_stream(stream: &mut ResponseStream) -> Vec<u8> {
    let mut body = Vec::new();
    while let Some(item) = stream.next().await.unwrap() {
        if let ResponseStreamItem::Data(data) = item {
            body.extend_from_slice(&data);
        }
    }
    body
}
