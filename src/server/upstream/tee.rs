use super::*;
use std::future::Future;
use tokio::sync::{mpsc, oneshot, watch};

const TEE_SINK_QUEUE_CAPACITY: usize = 8;

pub(in crate::server) trait TeeSink: Send + 'static {
    fn write<'a>(&'a mut self, data: &'a [u8]) -> impl Future<Output = io::Result<()>> + Send + 'a;
    fn abort(&mut self) -> impl Future<Output = ()> + Send;
    fn finish(&mut self) -> impl Future<Output = io::Result<()>> + Send;
}

enum SinkCommand {
    Write(Bytes),
    Finish(oneshot::Sender<io::Result<()>>),
}

pub(in crate::server) fn tee_response_stream<S, F>(
    mut source: ResponseStream,
    prepare: F,
) -> ResponseStream
where
    S: TeeSink,
    F: Future<Output = io::Result<S>> + Send + 'static,
{
    let length = source.length();
    let may_have_trailers = source.may_have_trailers();
    let (sender, receiver) = mpsc::channel(STREAM_CHANNEL_CAPACITY);
    let (sink_sender, sink_receiver) = mpsc::channel(TEE_SINK_QUEUE_CAPACITY);
    let (cancel_sender, cancel_receiver) = watch::channel(false);
    tokio::spawn(run_sink(prepare, sink_receiver, cancel_receiver));
    tokio::spawn(async move {
        let mut sink_sender = Some(sink_sender);
        loop {
            let item = tokio::select! {
                biased;
                _ = sender.closed() => {
                    let _ = cancel_sender.send(true);
                    return;
                }
                item = source.next() => item,
            };
            match item {
                Ok(Some(ResponseStreamItem::Data(data))) => {
                    if sender
                        .send(Ok(ResponseStreamItem::Data(data.clone())))
                        .await
                        .is_err()
                    {
                        let _ = cancel_sender.send(true);
                        return;
                    }
                    let Some(current) = sink_sender.as_ref() else {
                        continue;
                    };
                    let sent = tokio::select! {
                        biased;
                        _ = sender.closed() => {
                            let _ = cancel_sender.send(true);
                            return;
                        }
                        result = current.send(SinkCommand::Write(data)) => result,
                    };
                    if sent.is_err() {
                        sink_sender = None;
                    }
                }
                Ok(Some(ResponseStreamItem::Trailers(trailers))) => {
                    if sender
                        .send(Ok(ResponseStreamItem::Trailers(trailers)))
                        .await
                        .is_err()
                    {
                        let _ = cancel_sender.send(true);
                        return;
                    }
                }
                Ok(None) => {
                    if let Some(sink_sender) = sink_sender.take() {
                        let (finish_sender, finish_receiver) = oneshot::channel();
                        let sent = tokio::select! {
                            biased;
                            _ = sender.closed() => {
                                let _ = cancel_sender.send(true);
                                return;
                            }
                            result = sink_sender.send(SinkCommand::Finish(finish_sender)) => result,
                        };
                        if sent.is_ok() {
                            tokio::select! {
                                biased;
                                _ = sender.closed() => {
                                    let _ = cancel_sender.send(true);
                                    return;
                                }
                                _ = finish_receiver => {}
                            }
                        }
                    }
                    return;
                }
                Err(error) => {
                    let _ = cancel_sender.send(true);
                    let _ = sender.send(Err(error)).await;
                    return;
                }
            }
        }
    });
    let mut output = ResponseStream::new(receiver, length, may_have_trailers);
    // A cache/store tee must finish its durable side even if the client goes
    // away; otherwise a disconnected client can discard a nearly complete
    // cache fill or stored file.
    output.set_preserve_on_client_abort();
    output
}

async fn run_sink<S, F>(
    prepare: F,
    mut receiver: mpsc::Receiver<SinkCommand>,
    mut cancel: watch::Receiver<bool>,
) where
    S: TeeSink,
    F: Future<Output = io::Result<S>> + Send + 'static,
{
    tokio::pin!(prepare);
    let prepared = loop {
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return;
                }
            }
            result = &mut prepare => break result,
        }
    };
    let mut sink = match prepared {
        Ok(sink) => Some(sink),
        Err(_) => {
            drain_without_sink(receiver, cancel).await;
            return;
        }
    };
    loop {
        let command = tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    abort_sink(&mut sink).await;
                    return;
                }
                continue;
            }
            command = receiver.recv() => command,
        };
        let Some(command) = command else {
            abort_sink(&mut sink).await;
            return;
        };
        match command {
            SinkCommand::Write(data) => {
                if sink.is_none() {
                    continue;
                }
                let Some(result) = write_or_cancel(&mut sink, &data, &mut cancel).await else {
                    abort_sink(&mut sink).await;
                    return;
                };
                if result.is_err() {
                    abort_sink(&mut sink).await;
                    sink = None;
                }
            }
            SinkCommand::Finish(reply) => {
                let Some(result) = finish_or_cancel(&mut sink, &mut cancel).await else {
                    abort_sink(&mut sink).await;
                    return;
                };
                if result.is_err() {
                    abort_sink(&mut sink).await;
                }
                let _ = reply.send(result);
                return;
            }
        }
    }
}

async fn write_or_cancel<S: TeeSink>(
    sink: &mut Option<S>,
    data: &[u8],
    cancel: &mut watch::Receiver<bool>,
) -> Option<io::Result<()>> {
    loop {
        let current = sink.as_mut().expect("tee sink");
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return None;
                }
            }
            result = current.write(data) => return Some(result),
        }
    }
}

async fn finish_or_cancel<S: TeeSink>(
    sink: &mut Option<S>,
    cancel: &mut watch::Receiver<bool>,
) -> Option<io::Result<()>> {
    let Some(current) = sink.as_mut() else {
        return Some(Ok(()));
    };
    loop {
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return None;
                }
            }
            result = current.finish() => return Some(result),
        }
    }
}

async fn drain_without_sink(
    mut receiver: mpsc::Receiver<SinkCommand>,
    mut cancel: watch::Receiver<bool>,
) {
    loop {
        let command = tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return;
                }
                continue;
            }
            command = receiver.recv() => command,
        };
        match command {
            Some(SinkCommand::Write(_)) => {}
            Some(SinkCommand::Finish(reply)) => {
                let _ = reply.send(Ok(()));
                return;
            }
            None => return,
        }
    }
}

async fn abort_sink<S: TeeSink>(sink: &mut Option<S>) {
    if let Some(sink) = sink.as_mut() {
        sink.abort().await;
    }
}

#[cfg(test)]
mod tests;
