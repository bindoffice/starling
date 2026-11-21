use super::*;
use bytes::Bytes;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum ResponseStreamItem {
    Data(Bytes),
    Trailers(Vec<Header>),
}

type TrailerFilter = Arc<dyn Fn(Vec<Header>) -> Vec<Header> + Send + Sync>;

pub struct ResponseStream {
    receiver: mpsc::Receiver<io::Result<ResponseStreamItem>>,
    length: Option<u64>,
    may_have_trailers: bool,
    trailer_filter: Option<TrailerFilter>,
    preserve_on_client_abort: bool,
}

impl ResponseStream {
    pub(crate) fn new(
        receiver: mpsc::Receiver<io::Result<ResponseStreamItem>>,
        length: Option<u64>,
        may_have_trailers: bool,
    ) -> Self {
        Self {
            receiver,
            length,
            may_have_trailers,
            trailer_filter: None,
            preserve_on_client_abort: false,
        }
    }

    pub(crate) fn length(&self) -> Option<u64> {
        self.length
    }

    pub(crate) fn may_have_trailers(&self) -> bool {
        self.may_have_trailers
    }

    pub(crate) fn preserve_on_client_abort(&self) -> bool {
        self.preserve_on_client_abort
    }

    pub(crate) fn set_preserve_on_client_abort(&mut self) {
        self.preserve_on_client_abort = true;
    }

    pub(crate) async fn drain(mut self) {
        while let Ok(Some(_)) = self.next().await {}
    }

    pub(crate) fn set_trailer_filter(
        &mut self,
        filter: impl Fn(Vec<Header>) -> Vec<Header> + Send + Sync + 'static,
    ) {
        self.trailer_filter = Some(Arc::new(filter));
    }

    pub(crate) async fn next(&mut self) -> io::Result<Option<ResponseStreamItem>> {
        loop {
            let item = self.receiver.recv().await.transpose()?;
            let Some(ResponseStreamItem::Trailers(mut trailers)) = item else {
                return Ok(item);
            };
            if let Some(filter) = &self.trailer_filter {
                trailers = filter(trailers);
            }
            if !trailers.is_empty() {
                return Ok(Some(ResponseStreamItem::Trailers(trailers)));
            }
        }
    }
}

impl std::fmt::Debug for ResponseStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResponseStream")
            .field("length", &self.length)
            .field("may_have_trailers", &self.may_have_trailers)
            .field("has_trailer_filter", &self.trailer_filter.is_some())
            .finish_non_exhaustive()
    }
}
