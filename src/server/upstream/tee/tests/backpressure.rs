use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct SlowSinkState {
    writes_started: AtomicUsize,
    first_write_started: tokio::sync::Notify,
    release_first_write: tokio::sync::Notify,
    written: Mutex<Vec<u8>>,
}

struct SlowSink {
    state: Arc<SlowSinkState>,
}

impl TeeSink for SlowSink {
    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let index = self.state.writes_started.fetch_add(1, Ordering::SeqCst);
        if index == 0 {
            self.state.first_write_started.notify_one();
            self.state.release_first_write.notified().await;
        }
        self.state
            .written
            .lock()
            .expect("slow sink state")
            .extend_from_slice(data);
        Ok(())
    }

    async fn abort(&mut self) {}

    async fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn slow_sink_does_not_block_queued_downstream_chunks() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(SlowSinkState::default());
    let sink_state = state.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(SlowSink { state: sink_state })
        });

    for chunk in [b"one".as_slice(), b"two", b"three"] {
        source_sender
            .send(Ok(ResponseStreamItem::Data(Bytes::copy_from_slice(chunk))))
            .await
            .unwrap();
    }

    tokio::time::timeout(Duration::from_secs(1), state.first_write_started.notified())
        .await
        .expect("the sink should start writing");
    for expected in [b"one".as_slice(), b"two", b"three"] {
        let item = tokio::time::timeout(Duration::from_secs(1), output.next())
            .await
            .expect("queued downstream data should not wait for the slow sink")
            .unwrap()
            .unwrap();
        let ResponseStreamItem::Data(data) = item else {
            panic!("unexpected response stream item");
        };
        assert_eq!(&data[..], expected);
    }

    drop(source_sender);
    state.release_first_write.notify_one();
    collect_stream(&mut output).await;
    assert_eq!(
        state.written.lock().expect("slow sink state").as_slice(),
        b"onetwothree"
    );
}

#[tokio::test]
async fn tee_stream_is_preserved_when_the_client_aborts() {
    let (_source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(SlowSinkState::default());
    let sink_state = state.clone();
    let output = tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
        Ok(SlowSink { state: sink_state })
    });

    assert!(output.preserve_on_client_abort());
}

#[tokio::test]
async fn sink_queue_exhaustion_applies_bounded_backpressure() {
    let (source_sender, source_receiver) = mpsc::channel(1);
    let state = Arc::new(SlowSinkState::default());
    let sink_state = state.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(SlowSink { state: sink_state })
        });

    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"one"))))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), state.first_write_started.notified())
        .await
        .expect("the sink should start writing");

    let mut accepted = 1;
    loop {
        let sent = tokio::time::timeout(
            Duration::from_millis(50),
            source_sender.send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"two")))),
        )
        .await;
        match sent {
            Ok(Ok(())) => accepted += 1,
            Ok(Err(_)) => panic!("source closed while testing backpressure"),
            Err(_) => break,
        }
        assert!(
            accepted < 64,
            "tee queues must apply backpressure instead of growing without bound"
        );
    }

    state.release_first_write.notify_one();
    drop(source_sender);
    let body = collect_stream(&mut output).await;
    let written = state.written.lock().expect("slow sink state");
    assert_eq!(body.len(), written.len());
}
