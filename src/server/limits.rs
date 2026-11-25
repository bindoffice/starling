use super::*;
use std::sync::atomic::AtomicBool;

type LimitKey = (String, Vec<u8>);
type GuardEntry = (LimitKey, Arc<ConnectionOwner>);

#[derive(Default)]
pub(in crate::server) struct LimitState {
    pub(super) requests: HashMap<LimitKey, RequestBucket>,
    pub(super) connections: HashMap<LimitKey, Vec<Arc<ConnectionOwner>>>,
}

#[derive(Clone, Copy)]
pub(super) struct RequestBucket {
    pub(super) last: Instant,
    pub(super) excess: u64,
}

#[derive(Default)]
pub(in crate::server) struct ConnectionLimitGuard {
    inner: Option<Arc<Mutex<ConnectionLimitGuardState>>>,
}

struct ConnectionLimitGuardState {
    state: Arc<Mutex<LimitState>>,
    socket: Option<ClientSocketControl>,
    entries: Vec<GuardEntry>,
}

pub(super) struct ConnectionOwner {
    released: AtomicBool,
    socket: Option<ClientSocketControl>,
}

impl ConnectionOwner {
    fn closed(&self) -> bool {
        self.socket
            .as_ref()
            .is_some_and(ClientSocketControl::close_detected)
    }
}

#[derive(Clone)]
pub(in crate::server) struct ConnectionLimitGuardHandle {
    inner: Arc<Mutex<ConnectionLimitGuardState>>,
}

impl std::fmt::Debug for ConnectionLimitGuardHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConnectionLimitGuardHandle")
    }
}

impl ConnectionLimitGuard {
    pub(in crate::server) fn new(
        state: Arc<Mutex<LimitState>>,
        socket: Option<ClientSocketControl>,
    ) -> Self {
        Self {
            inner: Some(Arc::new(Mutex::new(ConnectionLimitGuardState {
                state,
                socket,
                entries: Vec::new(),
            }))),
        }
    }

    pub(in crate::server) fn handle(&self) -> ConnectionLimitGuardHandle {
        ConnectionLimitGuardHandle {
            inner: self
                .inner
                .as_ref()
                .expect("connection limit guard is initialized")
                .clone(),
        }
    }

    pub(in crate::server) fn is_active(&self) -> bool {
        self.inner.is_some()
    }

    /// Returns `Ok(false)` when the limit is reached and `Err(())` when the
    /// shared state lock is unavailable.
    pub(in crate::server) fn acquire(
        &mut self,
        zone: &str,
        key: &[u8],
        connections: u64,
    ) -> Result<bool, ()> {
        let Some(inner) = self.inner.as_ref() else {
            return Err(());
        };
        let mut inner = inner.lock().map_err(|_| ())?;
        let mut state = inner.state.lock().map_err(|_| ())?;
        let state_key = (zone.to_string(), key.to_vec());
        let owners = state.connections.entry(state_key.clone()).or_default();
        owners.retain(|owner| {
            if owner.released.load(Ordering::Acquire) {
                return false;
            }
            if owner.closed() {
                owner.released.store(true, Ordering::Release);
                return false;
            }
            true
        });
        if owners.len() as u64 >= connections {
            return Ok(false);
        }
        let owner = Arc::new(ConnectionOwner {
            released: AtomicBool::new(false),
            socket: inner.socket.clone(),
        });
        owners.push(owner.clone());
        drop(state);
        inner.entries.push((state_key, owner));
        Ok(true)
    }

    pub(in crate::server) fn release_all(&self) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let Ok(mut inner) = inner.lock() else {
            return;
        };
        let state = inner.state.clone();
        let Ok(mut state) = state.lock() else {
            inner.entries.clear();
            return;
        };
        release_entries(&mut state, &mut inner.entries);
    }
}

impl Drop for ConnectionLimitGuard {
    fn drop(&mut self) {
        self.release_all();
    }
}

impl ConnectionLimitGuardHandle {
    pub(in crate::server) fn release_all(&self) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let state = inner.state.clone();
        let Ok(mut state) = state.lock() else {
            inner.entries.clear();
            return;
        };
        release_entries(&mut state, &mut inner.entries);
    }
}

fn release_entries(state: &mut LimitState, entries: &mut Vec<GuardEntry>) {
    for (key, owner) in entries.drain(..) {
        if owner.released.swap(true, Ordering::AcqRel) {
            continue;
        }
        let Some(owners) = state.connections.get_mut(&key) else {
            continue;
        };
        owners.retain(|candidate| !Arc::ptr_eq(candidate, &owner));
        if owners.is_empty() {
            state.connections.remove(&key);
        }
    }
}
