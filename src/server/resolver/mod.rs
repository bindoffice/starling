use std::collections::HashMap;
use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::config::ResolverConfig;

mod dns;

#[derive(Clone)]
pub(in crate::server) struct Resolver {
    config: ResolverConfig,
    timeout: Duration,
    cache: Arc<Mutex<HashMap<CacheKey, CacheEntry>>>,
    reverse_cache: Arc<Mutex<HashMap<IpAddr, ReverseCacheEntry>>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct CacheKey {
    name: String,
    ipv6: bool,
}

#[derive(Clone, Debug)]
struct CacheEntry {
    addresses: Vec<IpAddr>,
    expires: Instant,
}

#[derive(Clone, Debug)]
struct ReverseCacheEntry {
    name: String,
    expires: Instant,
}

impl Resolver {
    pub(in crate::server) fn new(config: ResolverConfig, timeout: Duration) -> Self {
        Self {
            config,
            timeout,
            cache: Arc::new(Mutex::new(HashMap::new())),
            reverse_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(in crate::server) async fn resolve(&self, host: &str) -> io::Result<Vec<IpAddr>> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        if let Ok(address) = host.parse::<IpAddr>() {
            return Ok(vec![address]);
        }
        if host.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty DNS name",
            ));
        }
        if !self.config.is_configured() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no resolver defined",
            ));
        }

        let key = CacheKey {
            name: host.clone(),
            ipv6: self.config.ipv6,
        };
        if let Some(addresses) = self.cached(&key).await {
            return Ok(addresses);
        }

        let mut addresses = Vec::new();
        let mut ttl = u32::MAX;
        let mut last_error = None;
        for (query_type, enabled) in [(dns::TYPE_A, true), (dns::TYPE_AAAA, self.config.ipv6)] {
            if !enabled {
                continue;
            }
            match self.query(&host, query_type).await {
                Ok(response) => {
                    addresses.extend(response.addresses);
                    if let Some(response_ttl) = response.ttl {
                        ttl = ttl.min(response_ttl);
                    }
                }
                Err(error) => last_error = Some(error),
            }
        }
        addresses.sort_unstable();
        addresses.dedup();
        if addresses.is_empty() {
            return Err(last_error.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, format!("no address for {host}"))
            }));
        }

        let lifetime = self
            .config
            .valid
            .unwrap_or_else(|| Duration::from_secs(ttl.min(86_400) as u64));
        if !lifetime.is_zero() {
            self.cache.lock().await.insert(
                key,
                CacheEntry {
                    addresses: addresses.clone(),
                    expires: Instant::now() + lifetime,
                },
            );
        }
        Ok(addresses)
    }

    pub(in crate::server) async fn reverse_lookup(&self, address: IpAddr) -> io::Result<String> {
        if !self.config.is_configured() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "no resolver defined",
            ));
        }
        if let Some(name) = self.cached_reverse(address).await {
            return Ok(name);
        }

        let reverse_name = reverse_name(address);
        let lookup = self.query(&reverse_name, dns::TYPE_PTR).await?;
        let mut last_error = None;
        for name in lookup.names {
            if !valid_client_hostname(&name) {
                last_error = Some(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "resolver returned an invalid host name",
                ));
                continue;
            }
            match self.resolve(&name).await {
                Ok(addresses) if addresses.contains(&address) => {
                    let ttl = lookup.ttl.unwrap_or(86_400);
                    let lifetime = self
                        .config
                        .valid
                        .unwrap_or_else(|| Duration::from_secs(u64::from(ttl.min(86_400))));
                    if !lifetime.is_zero() {
                        self.reverse_cache.lock().await.insert(
                            address,
                            ReverseCacheEntry {
                                name: name.clone(),
                                expires: Instant::now() + lifetime,
                            },
                        );
                    }
                    return Ok(name);
                }
                Ok(_) => {
                    last_error = Some(io::Error::new(
                        io::ErrorKind::NotFound,
                        "PTR host does not resolve to the client address",
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "DNS response has no PTR record")
        }))
    }

    async fn cached(&self, key: &CacheKey) -> Option<Vec<IpAddr>> {
        let mut cache = self.cache.lock().await;
        let entry = cache.get(key)?;
        if entry.expires > Instant::now() {
            return Some(entry.addresses.clone());
        }
        cache.remove(key);
        None
    }

    async fn cached_reverse(&self, address: IpAddr) -> Option<String> {
        let mut cache = self.reverse_cache.lock().await;
        let entry = cache.get(&address)?;
        if entry.expires > Instant::now() {
            return Some(entry.name.clone());
        }
        cache.remove(&address);
        None
    }

    async fn query(&self, host: &str, query_type: u16) -> io::Result<dns::Lookup> {
        let mut last_error = None;
        for server in &self.config.servers {
            match dns::query(*server, host, query_type, self.timeout).await {
                Ok(response) => {
                    if query_type == dns::TYPE_PTR
                        || !response.addresses.is_empty()
                        || !response.names.is_empty()
                    {
                        return Ok(response);
                    }
                    last_error = Some(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("no address for {host}"),
                    ));
                }
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no DNS name server available")
        }))
    }
}

fn reverse_name(address: IpAddr) -> String {
    match address {
        IpAddr::V4(address) => {
            let octets = address.octets();
            format!(
                "{}.{}.{}.{}.in-addr.arpa",
                octets[3], octets[2], octets[1], octets[0]
            )
        }
        IpAddr::V6(address) => {
            let mut labels = Vec::with_capacity(32);
            for byte in address.octets().iter().rev() {
                labels.push(format!("{:x}", byte & 0x0f));
                labels.push(format!("{:x}", byte >> 4));
            }
            format!("{}.ip6.arpa", labels.join("."))
        }
    }
}

fn valid_client_hostname(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.'))
}
