use super::*;
use crate::config::ProxyCacheLockConfig;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicUsize;
use tokio::sync::watch;
use tokio::time::Instant as TokioInstant;

static NEXT_LOCK_ID: AtomicU64 = AtomicU64::new(1);

pub(in crate::server) enum CacheLockOutcome {
    Acquired(CacheLockGuard),
    TimedOut,
}

pub(in crate::server) struct CacheLockGuard {
    request: LockRequest,
    id: u64,
}

impl Drop for CacheLockGuard {
    fn drop(&mut self) {
        let released = {
            let mut state = self
                .request
                .entry
                .state
                .lock()
                .expect("proxy cache lock state");
            if state
                .holder
                .as_ref()
                .is_some_and(|holder| holder.id == self.id)
            {
                state.holder = None;
                true
            } else {
                false
            }
        };
        if released {
            self.request
                .entry
                .changes
                .send_modify(|version| *version = version.wrapping_add(1));
        }
    }
}

#[derive(Default)]
pub(super) struct CacheLocks {
    inner: Arc<CacheLocksInner>,
}

#[derive(Default)]
struct CacheLocksInner {
    entries: StdMutex<HashMap<String, Arc<CacheLockEntry>>>,
}

struct CacheLockEntry {
    users: AtomicUsize,
    state: StdMutex<CacheLockState>,
    changes: watch::Sender<u64>,
}

#[derive(Default)]
struct CacheLockState {
    holder: Option<CacheLockHolder>,
}

struct CacheLockHolder {
    id: u64,
    started: TokioInstant,
}

struct LockRequest {
    inner: Arc<CacheLocksInner>,
    key: String,
    entry: Arc<CacheLockEntry>,
}

impl Drop for LockRequest {
    fn drop(&mut self) {
        self.inner.leave(&self.key, &self.entry);
    }
}

impl CacheLocks {
    pub(super) async fn acquire(
        &self,
        key: &str,
        config: &ProxyCacheLockConfig,
    ) -> CacheLockOutcome {
        let request = self.inner.join(key);
        let deadline = TokioInstant::now() + config.timeout;
        let mut changes = request.entry.changes.subscribe();

        loop {
            let holder = {
                let mut state = request.entry.state.lock().expect("proxy cache lock state");
                if state.holder.is_none() {
                    let id = NEXT_LOCK_ID.fetch_add(1, Ordering::Relaxed);
                    state.holder = Some(CacheLockHolder {
                        id,
                        started: TokioInstant::now(),
                    });
                    drop(state);
                    return CacheLockOutcome::Acquired(CacheLockGuard { request, id });
                }
                state
                    .holder
                    .as_ref()
                    .map(|holder| (holder.id, holder.started))
                    .expect("held cache lock")
            };

            let now = TokioInstant::now();
            if now >= deadline {
                return CacheLockOutcome::TimedOut;
            }

            let takeover_at = holder.1 + config.age;
            if now >= takeover_at {
                let mut state = request.entry.state.lock().expect("proxy cache lock state");
                let same_holder = state
                    .holder
                    .as_ref()
                    .is_some_and(|current| current.id == holder.0);
                if same_holder {
                    let id = NEXT_LOCK_ID.fetch_add(1, Ordering::Relaxed);
                    state.holder = Some(CacheLockHolder {
                        id,
                        started: TokioInstant::now(),
                    });
                    drop(state);
                    return CacheLockOutcome::Acquired(CacheLockGuard { request, id });
                }
                continue;
            }

            let wait_until = deadline.min(takeover_at);
            tokio::select! {
                changed = changes.changed() => {
                    let _ = changed;
                }
                _ = tokio::time::sleep_until(wait_until) => {}
            }
        }
    }

    pub(super) fn is_held(&self, key: &str) -> bool {
        let entries = self.inner.entries.lock().expect("proxy cache lock map");
        let Some(entry) = entries.get(key) else {
            return false;
        };
        entry
            .state
            .lock()
            .expect("proxy cache lock state")
            .holder
            .is_some()
    }
}

impl CacheLocksInner {
    fn join(self: &Arc<Self>, key: &str) -> LockRequest {
        let entry = {
            let mut entries = self.entries.lock().expect("proxy cache lock map");
            let entry = entries
                .entry(key.to_string())
                .or_insert_with(|| {
                    Arc::new(CacheLockEntry {
                        users: AtomicUsize::new(0),
                        state: StdMutex::new(CacheLockState::default()),
                        changes: watch::channel(0).0,
                    })
                })
                .clone();
            entry.users.fetch_add(1, Ordering::AcqRel);
            entry
        };
        LockRequest {
            inner: self.clone(),
            key: key.to_string(),
            entry,
        }
    }

    fn leave(&self, key: &str, entry: &Arc<CacheLockEntry>) {
        let mut entries = self.entries.lock().expect("proxy cache lock map");
        if entry.users.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        if entries
            .get(key)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            entries.remove(key);
        }
    }
}

#[cfg(test)]
mod tests;
