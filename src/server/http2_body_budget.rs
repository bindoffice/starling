use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const FD_RESERVE: u64 = 256;
const FDS_PER_ACTIVE_BODY_REQUEST: u64 = 3;
const MAX_BODY_REQUEST_BUDGET: usize = 16_384;

pub(in crate::server) struct Http2BodyBudget {
    permits: Arc<Semaphore>,
}

impl Http2BodyBudget {
    pub(in crate::server) fn new() -> Self {
        Self::with_limit(open_file_limit().unwrap_or(1024))
    }

    fn with_limit(limit: u64) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(body_request_limit(limit))),
        }
    }

    pub(in crate::server) async fn acquire(&self) -> OwnedSemaphorePermit {
        self.permits
            .clone()
            .acquire_owned()
            .await
            .expect("request body budget semaphore is never closed")
    }
}

fn body_request_limit(open_file_limit: u64) -> usize {
    open_file_limit
        .saturating_sub(FD_RESERVE)
        .checked_div(FDS_PER_ACTIVE_BODY_REQUEST)
        .unwrap_or(0)
        .clamp(1, MAX_BODY_REQUEST_BUDGET as u64) as usize
}

#[cfg(unix)]
fn open_file_limit() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit only writes to the provided rlimit structure.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return None;
    }
    Some(limit.rlim_cur)
}

#[cfg(not(unix))]
fn open_file_limit() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_descriptors_for_clients_and_runtime_files() {
        assert_eq!(body_request_limit(1024), 256);
        assert_eq!(body_request_limit(4096), 1280);
        assert_eq!(body_request_limit(256), 1);
        assert_eq!(body_request_limit(u64::MAX), MAX_BODY_REQUEST_BUDGET);
    }

    #[tokio::test]
    async fn limits_concurrent_body_requests() {
        let budget = Http2BodyBudget::with_limit(262);
        let first = budget.acquire().await;
        assert_eq!(budget.permits.available_permits(), 1);
        let second = budget.acquire().await;
        assert_eq!(budget.permits.available_permits(), 0);
        drop(first);
        assert_eq!(budget.permits.available_permits(), 1);
        drop(second);
    }
}
