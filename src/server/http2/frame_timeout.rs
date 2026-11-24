use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const FRAME_DATA: u8 = 0x0;
const FRAME_HEADERS: u8 = 0x1;
const FRAME_CONTINUATION: u8 = 0x9;

pub(super) struct ExpiredFrame {
    pub(super) stream_id: u32,
    pub(super) headers: bool,
}

#[derive(Clone)]
pub(super) struct FrameTimeoutTracker {
    state: Arc<Mutex<State>>,
    changed: mpsc::UnboundedSender<()>,
}

pub(super) struct FrameTimeoutEvents {
    changed: mpsc::UnboundedReceiver<()>,
}

struct State {
    header_timeout: Duration,
    body_timeout: Duration,
    pending: Option<PendingFrame>,
    discard_remaining: usize,
}

struct PendingFrame {
    stream_id: u32,
    kind: u8,
    remaining: usize,
    deadline: Instant,
}

impl FrameTimeoutTracker {
    pub(super) fn new(
        header_timeout: Duration,
        body_timeout: Duration,
    ) -> (Self, FrameTimeoutEvents) {
        let (changed, events) = mpsc::unbounded_channel();
        (
            Self {
                state: Arc::new(Mutex::new(State {
                    header_timeout,
                    body_timeout,
                    pending: None,
                    discard_remaining: 0,
                })),
                changed,
            },
            FrameTimeoutEvents { changed: events },
        )
    }

    pub(super) fn frame_incomplete(
        &self,
        stream_id: u32,
        kind: u8,
        remaining: usize,
        now: Instant,
    ) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let timeout = if kind == FRAME_DATA {
            state.body_timeout
        } else {
            state.header_timeout
        };
        state.pending = (!timeout.is_zero() && remaining != 0).then(|| PendingFrame {
            stream_id,
            kind,
            remaining,
            deadline: now + timeout,
        });
        let _ = self.changed.send(());
    }

    pub(super) fn frame_complete(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        if state.pending.take().is_some() {
            let _ = self.changed.send(());
        }
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.pending.as_ref().map(|frame| frame.deadline))
    }

    pub(super) fn take_expired(&self, now: Instant) -> Option<ExpiredFrame> {
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        let frame = state.pending.as_ref()?;
        if frame.deadline > now {
            return None;
        }
        let stream_id = frame.stream_id;
        let headers = matches!(frame.kind, FRAME_HEADERS | FRAME_CONTINUATION);
        let remaining = frame.remaining;
        state.pending = None;
        state.discard_remaining = remaining;
        let _ = self.changed.send(());
        Some(ExpiredFrame { stream_id, headers })
    }

    pub(super) fn discard_remaining(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.discard_remaining)
            .unwrap_or(0)
    }

    pub(super) fn has_pending(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.pending.is_some() || state.discard_remaining != 0)
            .unwrap_or(false)
    }

    pub(super) fn consume_discard(&self, bytes: usize) {
        if let Ok(mut state) = self.state.lock() {
            state.discard_remaining = state.discard_remaining.saturating_sub(bytes);
        }
    }
}

impl FrameTimeoutEvents {
    pub(super) async fn changed(&mut self) -> Option<()> {
        self.changed.recv().await
    }
}
