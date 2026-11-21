use super::*;

#[derive(Default)]
pub(in crate::server) struct LimitState {
    pub(super) requests: HashMap<(String, Vec<u8>), RequestBucket>,
    pub(super) connections: HashMap<(String, Vec<u8>), u64>,
}

#[derive(Clone, Copy)]
pub(super) struct RequestBucket {
    pub(super) last: Instant,
    pub(super) excess: u64,
}

pub(in crate::server) struct ConnectionLimitGuard {
    state: Arc<Mutex<LimitState>>,
    keys: Vec<(String, Vec<u8>)>,
}

impl ConnectionLimitGuard {
    pub(in crate::server) fn new(state: Arc<Mutex<LimitState>>) -> Self {
        Self {
            state,
            keys: Vec::new(),
        }
    }

    /// Returns `Ok(false)` when the limit is reached and `Err(())` when the
    /// shared state lock is unavailable.
    pub(in crate::server) fn acquire(
        &mut self,
        zone: &str,
        key: &[u8],
        connections: u64,
    ) -> Result<bool, ()> {
        let mut state = self.state.lock().map_err(|_| ())?;
        let state_key = (zone.to_string(), key.to_vec());
        let count = state.connections.get(&state_key).copied().unwrap_or(0);
        if count >= connections {
            return Ok(false);
        }
        state.connections.insert(state_key.clone(), count + 1);
        drop(state);
        self.keys.push(state_key);
        Ok(true)
    }

    pub(in crate::server) fn release_all(&mut self) {
        let Ok(mut state) = self.state.lock() else {
            self.keys.clear();
            return;
        };
        release_keys(&mut state, &mut self.keys);
    }
}

impl Drop for ConnectionLimitGuard {
    fn drop(&mut self) {
        self.release_all();
    }
}

fn release_keys(state: &mut LimitState, keys: &mut Vec<(String, Vec<u8>)>) {
    for key in keys.drain(..) {
        let Some(count) = state.connections.get_mut(&key) else {
            continue;
        };
        *count = count.saturating_sub(1);
        if *count == 0 {
            state.connections.remove(&key);
        }
    }
}
