use super::*;

pub(in crate::server) struct ProxyV2Connection {
    pub(super) sender: h2::client::SendRequest<Bytes>,
    driver: ProxyV2DriverGuard,
    pub(super) key: UpstreamPoolKey,
    pub(super) created: Instant,
    pub(super) idle_since: Instant,
    pub(super) requests: u64,
}

struct ProxyV2Driver {
    task: tokio::task::JoinHandle<()>,
}

impl Drop for ProxyV2Driver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Clone)]
pub(super) struct ProxyV2DriverGuard(Arc<ProxyV2Driver>);

impl ProxyV2DriverGuard {
    fn is_finished(&self) -> bool {
        self.0.task.is_finished()
    }
}

impl ProxyV2Connection {
    pub(in crate::server) async fn connect(
        io: BoxedUpstreamIo,
        key: UpstreamPoolKey,
        timeout: Duration,
    ) -> Result<Self, ProxyFailure> {
        let (sender, connection) =
            match tokio::time::timeout(timeout, h2::client::handshake(io)).await {
                Ok(Ok(parts)) => parts,
                Ok(Err(_)) => return Err(ProxyFailure::Failure),
                Err(_) => return Err(ProxyFailure::Timeout),
            };
        let task = tokio::spawn(async move {
            let _ = connection.await;
        });
        let now = Instant::now();
        Ok(Self {
            sender,
            driver: ProxyV2DriverGuard(Arc::new(ProxyV2Driver { task })),
            key,
            created: now,
            idle_since: now,
            requests: 0,
        })
    }

    pub(in crate::server) fn key(&self) -> &UpstreamPoolKey {
        &self.key
    }

    pub(in crate::server) fn created(&self) -> Instant {
        self.created
    }

    pub(in crate::server) fn idle_since(&self) -> Instant {
        self.idle_since
    }

    pub(in crate::server) fn requests(&self) -> u64 {
        self.requests
    }

    pub(in crate::server) fn mark_idle(&mut self, idle_since: Instant) {
        self.idle_since = idle_since;
    }

    pub(in crate::server) fn idle_is_open(&mut self) -> bool {
        !self.driver.is_finished()
    }

    pub(super) fn driver_guard(&self) -> ProxyV2DriverGuard {
        self.driver.clone()
    }
}
