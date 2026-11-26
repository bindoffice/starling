use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const FD_RESERVE: u64 = 256;
const FDS_PER_ACTIVE_REQUEST: u64 = 2;
const FDS_PER_ACTIVE_BODY_REQUEST: u64 = 3;
const MAX_REQUEST_BUDGET: usize = 16_384;

pub(in crate::server) struct Http2Budget {
    requests: Arc<Semaphore>,
    bodies: Arc<Semaphore>,
}

impl Http2Budget {
    pub(in crate::server) fn new() -> Self {
        Self::with_limit(open_file_limit().unwrap_or(1024))
    }

    fn with_limit(limit: u64) -> Self {
        Self {
            requests: Arc::new(Semaphore::new(request_limit(limit))),
            bodies: Arc::new(Semaphore::new(body_request_limit(limit))),
        }
    }

    pub(in crate::server) async fn acquire_request(&self) -> OwnedSemaphorePermit {
        self.requests
            .clone()
            .acquire_owned()
            .await
            .expect("HTTP/2 request budget semaphore is never closed")
    }

    pub(in crate::server) async fn acquire_body(&self) -> OwnedSemaphorePermit {
        self.bodies
            .clone()
            .acquire_owned()
            .await
            .expect("HTTP/2 body budget semaphore is never closed")
    }
}

fn request_limit(open_file_limit: u64) -> usize {
    budget_limit(open_file_limit, FDS_PER_ACTIVE_REQUEST)
}

fn body_request_limit(open_file_limit: u64) -> usize {
    budget_limit(open_file_limit, FDS_PER_ACTIVE_BODY_REQUEST)
}

fn budget_limit(open_file_limit: u64, fds_per_request: u64) -> usize {
    open_file_limit
        .saturating_sub(FD_RESERVE)
        .checked_div(fds_per_request)
        .unwrap_or(0)
        .clamp(1, MAX_REQUEST_BUDGET as u64) as usize
}

#[cfg(unix)]
fn open_file_limit() -> Option<u64> {
    Some(
        rustix::process::getrlimit(rustix::process::Resource::Nofile)
            .current
            .unwrap_or(u64::MAX),
    )
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
        assert_eq!(request_limit(1024), 384);
        assert_eq!(request_limit(4096), 1920);
        assert_eq!(request_limit(256), 1);
        assert_eq!(request_limit(u64::MAX), MAX_REQUEST_BUDGET);
        assert_eq!(body_request_limit(1024), 256);
        assert_eq!(body_request_limit(4096), 1280);
        assert_eq!(body_request_limit(256), 1);
        assert_eq!(body_request_limit(u64::MAX), MAX_REQUEST_BUDGET);
    }

    #[tokio::test]
    async fn limits_concurrent_requests_and_bodies_independently() {
        let budget = Http2Budget::with_limit(264);
        let request = budget.acquire_request().await;
        let second_request = budget.acquire_request().await;
        assert_eq!(budget.requests.available_permits(), 2);

        let body = budget.acquire_body().await;
        let second_body = budget.acquire_body().await;
        assert_eq!(budget.bodies.available_permits(), 0);

        drop(request);
        drop(body);
        assert_eq!(budget.requests.available_permits(), 3);
        assert_eq!(budget.bodies.available_permits(), 1);
        drop(second_request);
        drop(second_body);
    }
}
