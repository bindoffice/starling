use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use tokio_rustls::rustls::sign::CertifiedKey;

use super::certificate::CertificateIdentity;
use super::refresh;
use super::url::ResponderUrl;
use crate::server::resolver::Resolver;

pub(in crate::server) struct StaplingState {
    base: Arc<CertifiedKey>,
    current: RwLock<Arc<CertifiedKey>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl std::fmt::Debug for StaplingState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StaplingState")
            .field("has_ocsp", &self.certificate().ocsp.is_some())
            .finish_non_exhaustive()
    }
}

impl StaplingState {
    pub(super) fn new(base: Arc<CertifiedKey>, ocsp: Option<Vec<u8>>) -> Arc<Self> {
        let current = certificate_with_ocsp(&base, ocsp);
        Arc::new(Self {
            base,
            current: RwLock::new(current),
            task: Mutex::new(None),
        })
    }

    pub(in crate::server) fn certificate(&self) -> Arc<CertifiedKey> {
        self.current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(super) fn install_ocsp(&self, ocsp: Vec<u8>) {
        let certificate = certificate_with_ocsp(&self.base, Some(ocsp));
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = certificate;
    }

    pub(super) fn start(
        state: &Arc<Self>,
        identity: Arc<CertificateIdentity>,
        responder: ResponderUrl,
        trusted_certificate: Option<PathBuf>,
        resolver: Resolver,
        timeout: std::time::Duration,
    ) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(state);
        let task = runtime.spawn(refresh::run(
            weak,
            identity,
            responder,
            trusted_certificate,
            resolver,
            timeout,
        ));
        *state
            .task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(task);
    }
}

impl Drop for StaplingState {
    fn drop(&mut self) {
        if let Some(task) = self
            .task
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            task.abort();
        }
    }
}

fn certificate_with_ocsp(
    certificate: &Arc<CertifiedKey>,
    ocsp: Option<Vec<u8>>,
) -> Arc<CertifiedKey> {
    let Some(ocsp) = ocsp else {
        return certificate.clone();
    };
    let mut stapled = (**certificate).clone();
    stapled.ocsp = Some(ocsp);
    Arc::new(stapled)
}
