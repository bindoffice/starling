use std::future::Future;
use std::io;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::{BodyWindow, WindowItem};
use crate::http::{ResponseBody, ResponseStream, ResponseStreamItem};

const STREAM_CHANNEL_CAPACITY: usize = 1;

pub(in crate::server::upstream) fn spawn_windowed_body<F>(
    window: Arc<BodyWindow>,
    length: Option<u64>,
    may_have_trailers: bool,
    producer: F,
) -> ResponseBody
where
    F: Future<Output = io::Result<()>> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let reader = window.clone();
    let producer_task = tokio::spawn(producer);
    let producer = AbortOnDrop::new(tokio::spawn(async move {
        let mut producer_task = AbortOnDrop::new(producer_task);
        match producer_task.join().await {
            Some(Ok(Ok(()))) => window.finish().await,
            Some(Ok(Err(error))) => window.fail(error).await,
            Some(Err(error)) => {
                window
                    .fail(io::Error::other(format!(
                        "upstream body producer task failed: {error}"
                    )))
                    .await;
            }
            None => {
                window
                    .fail(io::Error::other("upstream body producer task disappeared"))
                    .await;
            }
        }
    }));
    tokio::spawn(async move {
        if drive_window(reader, sender, producer).await.is_err() {
            // The producer observes the closed window and terminates on its next push.
        }
    });
    ResponseBody::Stream(ResponseStream::new(receiver, length, may_have_trailers))
}

async fn drive_window(
    window: Arc<BodyWindow>,
    sender: mpsc::Sender<io::Result<ResponseStreamItem>>,
    producer: AbortOnDrop<()>,
) -> io::Result<()> {
    loop {
        let item = match tokio::select! {
            biased;
            _ = sender.closed() => {
                producer.abort();
                window.abort().await;
                return Ok(());
            }
            item = window.pop() => item,
        } {
            Ok(Some(item)) => item,
            Ok(None) => {
                producer.disarm();
                return Ok(());
            }
            Err(error) => {
                producer.abort();
                window.abort().await;
                let _ = sender.send(Err(error)).await;
                return Ok(());
            }
        };
        let item = match item {
            WindowItem::Data(data) => ResponseStreamItem::Data(data),
            WindowItem::Trailers(trailers) => ResponseStreamItem::Trailers(trailers),
        };
        if sender.send(Ok(item)).await.is_err() {
            producer.abort();
            window.abort().await;
            return Ok(());
        }
    }
}

struct AbortOnDrop<T> {
    task: Option<JoinHandle<T>>,
}

impl<T> AbortOnDrop<T> {
    fn new(task: JoinHandle<T>) -> Self {
        Self { task: Some(task) }
    }

    fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    async fn join(&mut self) -> Option<Result<T, tokio::task::JoinError>> {
        let task = self.task.as_mut()?;
        let result = task.await;
        self.task = None;
        Some(result)
    }

    fn disarm(mut self) {
        self.task = None;
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
