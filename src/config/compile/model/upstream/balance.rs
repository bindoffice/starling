use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::atomic::Ordering;

use super::types::{LeastTimeConfig, UpstreamConfig, UpstreamMethod, UpstreamServer};

impl UpstreamConfig {
    pub fn select(&self) -> Option<&UpstreamServer> {
        self.select_with(None, None)
    }

    pub fn select_with(
        &self,
        peer: Option<IpAddr>,
        hash_key: Option<&str>,
    ) -> Option<&UpstreamServer> {
        self.select_candidates_with(peer, hash_key)
            .into_iter()
            .next()
    }

    pub fn select_candidates(&self) -> Vec<&UpstreamServer> {
        self.select_candidates_with(None, None)
    }

    pub fn select_candidates_with(
        &self,
        peer: Option<IpAddr>,
        hash_key: Option<&str>,
    ) -> Vec<&UpstreamServer> {
        let primary_peers = self
            .servers
            .iter()
            .filter(|server| !server.backup && !server.down)
            .count();
        let primary_health = primary_peers != 1;
        let primary: Vec<_> = self
            .servers
            .iter()
            .filter(|server| {
                !server.backup
                    && server.has_capacity()
                    && (!primary_health || server.health_available())
            })
            .collect();
        let backup: Vec<_> = self
            .servers
            .iter()
            .filter(|server| server.backup && server.is_available())
            .collect();
        if primary.is_empty() && backup.is_empty() {
            return Vec::new();
        }
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        let mut candidates = self.order_servers(&primary, start, peer, hash_key);
        candidates.extend(self.order_servers(&backup, start, peer, hash_key));
        candidates
    }

    fn order_servers<'a>(
        &self,
        servers: &[&'a UpstreamServer],
        start: usize,
        peer: Option<IpAddr>,
        hash_key: Option<&str>,
    ) -> Vec<&'a UpstreamServer> {
        match self.method {
            UpstreamMethod::RoundRobin => weighted_unique(servers, start),
            UpstreamMethod::LeastConn => least_connections_order(servers, start),
            UpstreamMethod::LeastTime(config) => least_time_order(servers, start, config),
            UpstreamMethod::IpHash => ip_hash_order(servers, start, peer),
            UpstreamMethod::Random { two } => random_order(servers, start, two),
            UpstreamMethod::Hash { consistent: true } => {
                consistent_hash_order(servers, start, hash_key)
            }
            UpstreamMethod::Hash { consistent: false } => hash_order(servers, start, hash_key),
        }
    }
}

fn least_connections_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
) -> Vec<&'a UpstreamServer> {
    let mut ordered = weighted_unique(servers, start);
    ordered.sort_by(|left, right| {
        let left_weight = left.weight.max(1) as u128;
        let right_weight = right.weight.max(1) as u128;
        let left_load = left.active_connections() as u128 * right_weight;
        let right_load = right.active_connections() as u128 * left_weight;
        left_load.cmp(&right_load)
    });
    ordered
}

fn least_time_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
    config: LeastTimeConfig,
) -> Vec<&'a UpstreamServer> {
    let mut ordered = weighted_unique(servers, start);
    ordered.sort_by(|left, right| {
        let left_eta = left.least_time_eta(config) as u128;
        let right_eta = right.least_time_eta(config) as u128;
        let left_load = left_eta * right.weight.max(1) as u128;
        let right_load = right_eta * left.weight.max(1) as u128;
        left_load.cmp(&right_load)
    });
    ordered
}

fn ip_hash_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
    peer: Option<IpAddr>,
) -> Vec<&'a UpstreamServer> {
    if servers.len() < 2 {
        return weighted_unique(servers, start);
    }
    let Some(peer) = peer else {
        return weighted_unique(servers, start);
    };
    let mut hash = 89_usize;
    let bytes = match peer {
        IpAddr::V4(address) => address.octets()[..3].to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    };
    for byte in bytes {
        hash = (hash * 113 + byte as usize) % 6271;
    }
    prioritize_selected(servers, weighted_server(servers, hash))
}

fn random_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
    two: bool,
) -> Vec<&'a UpstreamServer> {
    if !two {
        return prioritize_selected(
            servers,
            weighted_server(servers, pseudo_random(start) as usize),
        );
    }

    let first = weighted_server(servers, pseudo_random(start) as usize);
    let mut second = weighted_server(
        servers,
        pseudo_random(start ^ 0xa5a5_a5a5_a5a5_a5a5) as usize,
    );
    for attempt in 1..=servers.len().max(1) * 4 {
        if second
            .zip(first)
            .is_none_or(|(second, first)| second.address != first.address)
        {
            break;
        }
        second = weighted_server(servers, pseudo_random(start.wrapping_add(attempt)) as usize);
    }
    let selected = match (first, second) {
        (Some(first), Some(second)) if first.address != second.address => {
            let first_load = first.active_connections() as u128 * second.weight.max(1) as u128;
            let second_load = second.active_connections() as u128 * first.weight.max(1) as u128;
            if second_load < first_load {
                second
            } else {
                first
            }
        }
        (Some(first), _) => first,
        (None, second) => match second {
            Some(second) => second,
            None => return Vec::new(),
        },
    };
    prioritize_selected(servers, Some(selected))
}

fn pseudo_random(value: usize) -> u64 {
    let mut value = (value as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    value
}

fn hash_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
    key: Option<&str>,
) -> Vec<&'a UpstreamServer> {
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return weighted_unique(servers, start);
    };
    if servers.len() < 2 {
        return weighted_unique(servers, start);
    }
    let total_weight: usize = servers.iter().map(|server| server.weight.max(1)).sum();
    if total_weight == 0 {
        return Vec::new();
    }
    let mut hash = 0_u32;
    let mut rehash = 0_usize;
    let mut seen = HashSet::new();
    let mut ordered = Vec::with_capacity(servers.len());
    let attempts = servers.len().saturating_mul(20).saturating_add(64);
    while ordered.len() < servers.len() && rehash < attempts {
        let mut hasher = crc32fast::Hasher::new();
        if rehash > 0 {
            hasher.update(rehash.to_string().as_bytes());
        }
        hasher.update(key.as_bytes());
        let delta = (hasher.finalize() >> 16) & 0x7fff;
        hash = hash.wrapping_add(delta);
        if let Some(server) = weighted_server(servers, hash as usize % total_weight)
            && seen.insert(server.address.clone())
        {
            ordered.push(server);
        }
        rehash += 1;
    }
    for server in weighted_unique(servers, start) {
        if seen.insert(server.address.clone()) {
            ordered.push(server);
        }
    }
    ordered
}

fn consistent_hash_order<'a>(
    servers: &[&'a UpstreamServer],
    start: usize,
    key: Option<&str>,
) -> Vec<&'a UpstreamServer> {
    let Some(key) = key.filter(|key| !key.is_empty()) else {
        return weighted_unique(servers, start);
    };
    let mut points = Vec::new();
    for server in servers {
        let (host, port) = split_upstream_address(&server.address);
        let mut previous = 0_u32;
        for _ in 0..server.weight.max(1) * 160 {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(host.as_bytes());
            hasher.update(&[0]);
            hasher.update(port.unwrap_or_default().as_bytes());
            hasher.update(&previous.to_le_bytes());
            let hash = hasher.finalize();
            points.push((hash, *server));
            previous = hash;
        }
    }
    points.sort_by_key(|(hash, _)| *hash);
    points.dedup_by_key(|(hash, _)| *hash);
    let target = crc32fast::hash(key.as_bytes());
    let index = points.partition_point(|(hash, _)| *hash < target);
    let selected = if points.is_empty() {
        None
    } else {
        Some(points[index % points.len()].1)
    };
    prioritize_selected(servers, selected)
}

fn prioritize_selected<'a>(
    servers: &[&'a UpstreamServer],
    selected: Option<&'a UpstreamServer>,
) -> Vec<&'a UpstreamServer> {
    let mut ordered = weighted_unique(servers, 0);
    if let Some(selected) = selected
        && let Some(index) = ordered
            .iter()
            .position(|server| server.address == selected.address)
    {
        ordered.rotate_left(index);
    }
    ordered
}

fn weighted_server<'a>(servers: &[&'a UpstreamServer], slot: usize) -> Option<&'a UpstreamServer> {
    let total_weight: usize = servers.iter().map(|server| server.weight.max(1)).sum();
    if total_weight == 0 {
        return None;
    }
    let mut slot = slot % total_weight;
    for server in servers {
        let weight = server.weight.max(1);
        if slot < weight {
            return Some(server);
        }
        slot -= weight;
    }
    None
}

fn split_upstream_address(address: &str) -> (&str, Option<&str>) {
    if let Some(address) = address.strip_prefix("unix:") {
        return (address, None);
    }
    if let Some((host, port)) = address.rsplit_once(':')
        && port.bytes().all(|byte| byte.is_ascii_digit())
    {
        return (host.trim_matches(['[', ']']), Some(port));
    }
    (address, None)
}

fn weighted_unique<'a>(servers: &[&'a UpstreamServer], start: usize) -> Vec<&'a UpstreamServer> {
    let mut slots = Vec::new();
    for server in servers {
        for _ in 0..server.weight.max(1) {
            slots.push(*server);
        }
    }
    if slots.is_empty() {
        return Vec::new();
    }
    let start = start % slots.len();
    slots.rotate_left(start);
    let mut seen = HashSet::new();
    slots
        .into_iter()
        .filter(|server| seen.insert(server.address.clone()))
        .collect()
}
