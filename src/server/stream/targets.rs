use super::*;

pub(in crate::server) fn resolve_stream_target(
    config: &RuntimeConfig,
    upstream_name: Option<&str>,
    direct_target: &str,
    peer: Option<IpAddr>,
    hash_key: &str,
) -> Option<(ResolvedUpstreamTarget, Option<UpstreamPeerGuard>)> {
    resolve_stream_targets(config, upstream_name, direct_target, peer, hash_key)
        .into_iter()
        .find_map(|target| target.try_acquire().ok().map(|guard| (target, guard)))
}

pub(in crate::server) fn resolve_stream_targets(
    config: &RuntimeConfig,
    upstream_name: Option<&str>,
    direct_target: &str,
    peer: Option<IpAddr>,
    hash_key: &str,
) -> Vec<ResolvedUpstreamTarget> {
    if let Some(upstream_name) = upstream_name {
        let upstream = config
            .stream
            .as_ref()
            .and_then(|stream| stream.upstreams.get(upstream_name));
        return upstream
            .map(|upstream| resolve_upstream_config(upstream, peer, Some(hash_key)))
            .unwrap_or_default();
    }

    parse_upstream_address(direct_target)
        .map(|(host, port)| ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
            record_timing: false,
        })
        .into_iter()
        .collect()
}
