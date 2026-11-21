use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct CancelSinkState {
    write_started: tokio::sync::Notify,
    aborted: tokio::sync::Notify,
}

struct CancelSink {
    state: Arc<CancelSinkState>,
}

impl TeeSink for CancelSink {
    async fn write(&mut self, _data: &[u8]) -> io::Result<()> {
        self.state.write_started.notify_one();
        std::future::pending::<io::Result<()>>().await
    }

    async fn abort(&mut self) {
        self.state.aborted.notify_one();
    }

    async fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn downstream_disconnect_aborts_a_blocked_sink() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(CancelSinkState::default());
    let sink_state = state.clone();
    let output = tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
        Ok(CancelSink { state: sink_state })
    });

    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), state.write_started.notified())
        .await
        .expect("the sink should start writing");
    drop(output);
    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"second"))))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(1), state.aborted.notified())
        .await
        .expect("downstream disconnect should abort the blocked sink");
}

#[derive(Default)]
struct FailingSinkState {
    aborted: AtomicUsize,
}

struct FailingSink {
    state: Arc<FailingSinkState>,
}

impl TeeSink for FailingSink {
    async fn write(&mut self, _data: &[u8]) -> io::Result<()> {
        Err(io::Error::other("simulated sink failure"))
    }

    async fn abort(&mut self) {
        self.state.aborted.fetch_add(1, Ordering::SeqCst);
    }

    async fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn sink_failure_does_not_interrupt_downstream_stream() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(FailingSinkState::default());
    let sink_state = state.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(FailingSink { state: sink_state })
        });

    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"first"))))
        .await
        .unwrap();
    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(b"second"))))
        .await
        .unwrap();
    drop(source_sender);

    assert_eq!(collect_stream(&mut output).await, b"firstsecond");
    assert_eq!(state.aborted.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct PartialFailureSinkState {
    written: Mutex<Vec<u8>>,
    aborted: AtomicUsize,
}

struct PartialFailureSink {
    state: Arc<PartialFailureSinkState>,
    limit: usize,
}

impl TeeSink for PartialFailureSink {
    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        let mut written = self.state.written.lock().expect("partial sink state");
        if written.len().saturating_add(data.len()) > self.limit {
            return Err(io::Error::other("simulated disk full"));
        }
        written.extend_from_slice(data);
        Ok(())
    }

    async fn abort(&mut self) {
        self.state.aborted.fetch_add(1, Ordering::SeqCst);
    }

    async fn finish(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn partial_sink_failure_keeps_the_downstream_body_complete() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(PartialFailureSinkState::default());
    let sink_state = state.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(PartialFailureSink {
                state: sink_state,
                limit: 4,
            })
        });

    for chunk in [b"one".as_slice(), b"two", b"three"] {
        source_sender
            .send(Ok(ResponseStreamItem::Data(Bytes::copy_from_slice(chunk))))
            .await
            .unwrap();
    }
    drop(source_sender);

    assert_eq!(collect_stream(&mut output).await, b"onetwothree");
    assert_eq!(
        state.written.lock().expect("partial sink state").as_slice(),
        b"one"
    );
    assert_eq!(state.aborted.load(Ordering::SeqCst), 1);
}

#[derive(Default)]
struct OrderedSinkState {
    events: Mutex<Vec<String>>,
}

struct OrderedSink {
    state: Arc<OrderedSinkState>,
}

impl TeeSink for OrderedSink {
    async fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.state
            .events
            .lock()
            .expect("ordered sink state")
            .push(format!("write:{}", String::from_utf8_lossy(data)));
        Ok(())
    }

    async fn abort(&mut self) {
        self.state
            .events
            .lock()
            .expect("ordered sink state")
            .push("abort".into());
    }

    async fn finish(&mut self) -> io::Result<()> {
        self.state
            .events
            .lock()
            .expect("ordered sink state")
            .push("finish".into());
        Ok(())
    }
}

#[tokio::test]
async fn successful_sink_writes_every_chunk_before_finish() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(OrderedSinkState::default());
    let sink_state = state.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(OrderedSink { state: sink_state })
        });

    for chunk in [b"one".as_slice(), b"two", b"three"] {
        source_sender
            .send(Ok(ResponseStreamItem::Data(Bytes::copy_from_slice(chunk))))
            .await
            .unwrap();
    }
    drop(source_sender);

    assert_eq!(collect_stream(&mut output).await, b"onetwothree");
    assert_eq!(
        state.events.lock().expect("ordered sink state").as_slice(),
        [
            "write:one".to_string(),
            "write:two".to_string(),
            "write:three".to_string(),
            "finish".to_string(),
        ]
    );
}

#[derive(Default)]
struct BlockingPrepareState {
    started: tokio::sync::Notify,
    cancelled: tokio::sync::Notify,
}

struct PrepareDropGuard {
    state: Arc<BlockingPrepareState>,
}

impl Drop for PrepareDropGuard {
    fn drop(&mut self) {
        self.state.cancelled.notify_one();
    }
}

#[tokio::test]
async fn downstream_disconnect_cancels_a_blocked_prepare() {
    let (_source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(BlockingPrepareState::default());
    let prepare_state = state.clone();
    let output = tee_response_stream(
        ResponseStream::new(source_receiver, None, false),
        async move {
            let _guard = PrepareDropGuard {
                state: prepare_state.clone(),
            };
            prepare_state.started.notify_one();
            std::future::pending::<io::Result<CancelSink>>().await
        },
    );

    tokio::time::timeout(Duration::from_secs(1), state.started.notified())
        .await
        .expect("prepare should start");
    drop(output);

    tokio::time::timeout(Duration::from_secs(1), state.cancelled.notified())
        .await
        .expect("downstream disconnect should cancel blocked preparation");
}

#[derive(Default)]
struct BlockingFinishState {
    finish_started: tokio::sync::Notify,
    aborted: tokio::sync::Notify,
}

struct BlockingFinishSink {
    state: Arc<BlockingFinishState>,
}

impl TeeSink for BlockingFinishSink {
    async fn write(&mut self, _data: &[u8]) -> io::Result<()> {
        Ok(())
    }

    async fn abort(&mut self) {
        self.state.aborted.notify_one();
    }

    async fn finish(&mut self) -> io::Result<()> {
        self.state.finish_started.notify_one();
        std::future::pending::<io::Result<()>>().await
    }
}

#[tokio::test]
async fn downstream_disconnect_aborts_a_blocked_finish() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let state = Arc::new(BlockingFinishState::default());
    let sink_state = state.clone();
    let output = tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
        Ok(BlockingFinishSink { state: sink_state })
    });

    drop(source_sender);
    tokio::time::timeout(Duration::from_secs(1), state.finish_started.notified())
        .await
        .expect("finish should start");
    drop(output);

    tokio::time::timeout(Duration::from_secs(1), state.aborted.notified())
        .await
        .expect("downstream disconnect should abort blocked finish");
}

struct FinishErrorSink {
    finished: Arc<AtomicUsize>,
}

impl TeeSink for FinishErrorSink {
    async fn write(&mut self, _data: &[u8]) -> io::Result<()> {
        Ok(())
    }

    async fn abort(&mut self) {}

    async fn finish(&mut self) -> io::Result<()> {
        self.finished.fetch_add(1, Ordering::SeqCst);
        Err(io::Error::other("simulated finish failure"))
    }
}

#[tokio::test]
async fn finish_failure_does_not_interrupt_completed_downstream_body() {
    let (source_sender, source_receiver) = mpsc::channel(8);
    let finished = Arc::new(AtomicUsize::new(0));
    let sink_finished = finished.clone();
    let mut output =
        tee_response_stream(ResponseStream::new(source_receiver, None, false), async {
            Ok(FinishErrorSink {
                finished: sink_finished,
            })
        });

    source_sender
        .send(Ok(ResponseStreamItem::Data(Bytes::from_static(
            b"complete",
        ))))
        .await
        .unwrap();
    drop(source_sender);

    assert_eq!(collect_stream(&mut output).await, b"complete");
    assert_eq!(finished.load(Ordering::SeqCst), 1);
}
