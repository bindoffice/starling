use std::time::Duration;

use tokio::time::{Instant, sleep_until};

pub(crate) struct ReadRate {
    rate: u64,
    bytes: u64,
    started: Option<Instant>,
}

impl ReadRate {
    pub(crate) fn new(rate: u64) -> Self {
        Self {
            rate,
            bytes: 0,
            started: None,
        }
    }

    pub(crate) async fn account(&mut self, bytes: u64) {
        if self.rate == 0 || bytes == 0 {
            return;
        }
        self.bytes = self.bytes.saturating_add(bytes);
        let started = *self.started.get_or_insert_with(Instant::now);
        let target = started + Duration::from_secs_f64(self.bytes as f64 / self.rate as f64);
        sleep_until(target).await;
    }
}
