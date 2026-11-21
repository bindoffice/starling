use super::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug, Default)]
pub(in crate::server) struct ClientAbortState {
    preserve: Arc<AtomicBool>,
    upstream_active: Arc<AtomicBool>,
}

impl ClientAbortState {
    pub(in crate::server) fn mark_preserve(&self) {
        self.preserve.store(true, Ordering::Release);
    }

    pub(in crate::server) fn preserve(&self) -> bool {
        self.preserve.load(Ordering::Acquire)
    }

    pub(in crate::server) fn mark_upstream_active(&self) {
        self.upstream_active.store(true, Ordering::Release);
    }

    pub(in crate::server) fn upstream_active(&self) -> bool {
        self.upstream_active.load(Ordering::Acquire)
    }
}

/// Keep an upstream response producer alive after the downstream connection
/// has failed when the response is cacheable/store-backed or the active
/// upstream module was configured with `*_ignore_client_abort on`.
pub(in crate::server) fn drain_after_client_abort(
    response: &mut Response,
) -> Option<tokio::task::JoinHandle<()>> {
    if !response.preserve_on_client_abort() {
        return None;
    }
    let body = std::mem::replace(&mut response.body, ResponseBody::Empty);
    let body_temp_file = response.body_temp_file.take();
    Some(tokio::spawn(async move {
        // The response body may still reference this backing file.
        let _body_temp_file = body_temp_file;
        body.drain_stream().await;
    }))
}
