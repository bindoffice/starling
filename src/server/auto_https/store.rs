use super::*;
use crate::config::{AutoHttpsChallenge, AutoHttpsConfig};
use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

pub(in crate::server) const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

#[derive(Debug, Clone)]
pub(in crate::server) struct AcmeOrder {
    pub(in crate::server) domains: Vec<String>,
    pub(in crate::server) paths: CertificatePaths,
    pub(in crate::server) staging: bool,
    pub(in crate::server) directory_url: String,
    pub(in crate::server) ca_file: Option<PathBuf>,
    pub(in crate::server) email: Option<String>,
    pub(in crate::server) challenge: AutoHttpsChallenge,
    pub(in crate::server) dns_hook: Option<PathBuf>,
    pub(in crate::server) dns_propagation_delay: Duration,
}

pub(in crate::server) struct AutoHttpsState {
    config: RwLock<AutoHttpsConfig>,
    challenges: RwLock<HashMap<String, String>>,
    orders: RwLock<Vec<AcmeOrder>>,
    in_flight: Mutex<std::collections::HashSet<String>>,
}

impl AutoHttpsState {
    pub(in crate::server) fn new(config: AutoHttpsConfig) -> Self {
        Self {
            config: RwLock::new(config),
            challenges: RwLock::new(HashMap::new()),
            orders: RwLock::new(Vec::new()),
            in_flight: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub(in crate::server) fn update_config(&self, config: AutoHttpsConfig) {
        *self
            .config
            .write()
            .expect("auto HTTPS config lock poisoned") = config;
    }

    pub(in crate::server) fn config(&self) -> AutoHttpsConfig {
        self.config
            .read()
            .expect("auto HTTPS config lock poisoned")
            .clone()
    }

    pub(in crate::server) fn set_orders(&self, orders: Vec<AcmeOrder>) {
        *self.orders.write().expect("auto HTTPS order lock poisoned") = orders;
    }

    pub(in crate::server) fn orders(&self) -> Vec<AcmeOrder> {
        self.orders
            .read()
            .expect("auto HTTPS order lock poisoned")
            .clone()
    }

    pub(in crate::server) fn set_challenge(&self, token: &str, key_authorization: String) {
        self.challenges
            .write()
            .expect("auto HTTPS challenge lock poisoned")
            .insert(token.to_string(), key_authorization);
    }

    pub(in crate::server) fn remove_challenge(&self, token: &str) {
        self.challenges
            .write()
            .expect("auto HTTPS challenge lock poisoned")
            .remove(token);
    }

    pub(in crate::server) fn challenge_response(&self, token: &str) -> Option<String> {
        self.challenges
            .read()
            .expect("auto HTTPS challenge lock poisoned")
            .get(token)
            .cloned()
    }

    pub(in crate::server) fn begin_order(&self, key: &str) -> bool {
        self.in_flight
            .lock()
            .expect("auto HTTPS in-flight lock poisoned")
            .insert(key.to_string())
    }

    pub(in crate::server) fn finish_order(&self, key: &str) {
        self.in_flight
            .lock()
            .expect("auto HTTPS in-flight lock poisoned")
            .remove(key);
    }
}
