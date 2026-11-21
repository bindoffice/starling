use super::*;

struct DropSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for DropSignal {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn receiver_close_cancels_a_pending_channel_producer() {
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, dropped_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(run_channel_producer(sender, async move {
        let _dropped = DropSignal(Some(dropped_tx));
        let _ = started_tx.send(());
        std::future::pending::<io::Result<()>>().await
    }));

    started_rx.await.unwrap();
    drop(receiver);
    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("receiver close should cancel the producer")
        .expect("producer future should be dropped");
    task.await.unwrap();
}

#[tokio::test]
async fn producer_error_is_forwarded_to_the_response_stream() {
    let (sender, mut receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let task = tokio::spawn(run_channel_producer(sender, async {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "upstream failed",
        ))
    }));

    let error = receiver
        .recv()
        .await
        .expect("error item")
        .expect_err("producer error should be forwarded");
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    assert!(receiver.recv().await.is_none());
    task.await.unwrap();
}
