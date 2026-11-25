use super::*;
use std::sync::atomic::AtomicBool;
use tokio::sync::Notify;

#[derive(Clone, Debug, Default)]
pub(in crate::server) struct ClientAbortState {
    inner: Arc<ClientAbortInner>,
}

#[derive(Debug, Default)]
struct ClientAbortInner {
    preserve: AtomicBool,
    upstream_active: AtomicBool,
    upstream_active_notify: Notify,
    connection_limit: std::sync::Mutex<Option<ConnectionLimitGuardHandle>>,
    socket: std::sync::Mutex<Option<ClientSocketControl>>,
}

impl ClientAbortState {
    pub(in crate::server) fn with_socket(socket: Option<ClientSocketControl>) -> Self {
        let state = Self::default();
        if let (Some(socket), Ok(mut current)) = (socket, state.inner.socket.lock()) {
            *current = Some(socket);
        }
        state
    }

    pub(in crate::server) fn mark_preserve(&self) {
        self.inner.preserve.store(true, Ordering::Release);
    }

    pub(in crate::server) fn preserve(&self) -> bool {
        self.inner.preserve.load(Ordering::Acquire)
    }

    pub(in crate::server) fn mark_upstream_active(&self) {
        self.inner.upstream_active.store(true, Ordering::Release);
        self.inner.upstream_active_notify.notify_one();
    }

    pub(in crate::server) fn upstream_active(&self) -> bool {
        self.inner.upstream_active.load(Ordering::Acquire)
    }

    pub(in crate::server) async fn wait_for_upstream_active(&self) {
        loop {
            let notified = self.inner.upstream_active_notify.notified();
            if self.upstream_active() {
                return;
            }
            notified.await;
        }
    }

    pub(in crate::server) fn socket(&self) -> Option<ClientSocketControl> {
        self.inner
            .socket
            .lock()
            .ok()
            .and_then(|socket| socket.clone())
    }

    pub(in crate::server) fn set_connection_limit(&self, guard: ConnectionLimitGuardHandle) {
        if let Ok(mut connection_limit) = self.inner.connection_limit.lock() {
            *connection_limit = Some(guard);
        }
    }

    pub(in crate::server) fn release_connection_limit(&self) {
        let guard = self
            .inner
            .connection_limit
            .lock()
            .ok()
            .and_then(|mut guard| guard.take());
        if let Some(guard) = guard {
            guard.release_all();
        }
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
