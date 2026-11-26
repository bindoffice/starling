use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use super::certificate::CertificateIdentity;
use super::request;
use super::response::refresh_delay;
use super::state::StaplingState;
use super::url::ResponderUrl;
use crate::server::resolver::Resolver;

pub(super) async fn run(
    state: Weak<StaplingState>,
    identity: Arc<CertificateIdentity>,
    responder: ResponderUrl,
    trusted_certificate: Option<PathBuf>,
    resolver: Resolver,
    timeout: Duration,
) {
    loop {
        if state.upgrade().is_none() {
            return;
        }
        let delay = match request::fetch(
            identity.clone(),
            responder.clone(),
            trusted_certificate.clone(),
            resolver.clone(),
            timeout,
        )
        .await
        {
            Ok(response) => {
                let now = SystemTime::now();
                if let Some(state) = state.upgrade() {
                    state.install_ocsp(response.der);
                } else {
                    return;
                }
                refresh_delay(now, response.next_update)
            }
            Err(_) => Duration::from_secs(300),
        };
        tokio::time::sleep(delay).await;
    }
}
