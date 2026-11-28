use super::*;

pub(in crate::server) fn resolve_fastcgi_targets(
    fastcgi: &FastCgiPassConfig,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> Vec<ResolvedUpstreamTarget> {
    resolve_gateway_targets(&fastcgi.raw, http, variables, peer)
}

pub(in crate::server) fn resolve_gateway_targets(
    raw: &str,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> Vec<ResolvedUpstreamTarget> {
    let target = eval_variables(raw, variables);
    if let Some(upstream) = http.upstreams.get(&target) {
        let hash_key = upstream
            .hash_key
            .as_deref()
            .map(|key| eval_variables(key, variables));
        return resolve_upstream_config(upstream, peer, hash_key.as_deref());
    }
    if target.starts_with("unix:") {
        return vec![ResolvedUpstreamTarget {
            host: target,
            port: 0,
            peer: None,
            check_health: false,
            record_timing: false,
        }];
    }
    parse_upstream_address(&target)
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

pub(in crate::server) fn resolve_uwsgi_targets(
    pass: &UwsgiPassConfig,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> (Vec<ResolvedUpstreamTarget>, bool) {
    let evaluated = eval_variables(&pass.raw, variables);
    let (target, scheme_tls) = strip_uwsgi_scheme(&evaluated);
    (
        resolve_gateway_targets(target, http, variables, peer),
        pass.tls || scheme_tls,
    )
}

pub(in crate::server) fn strip_uwsgi_scheme(raw: &str) -> (&str, bool) {
    if raw
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("suwsgi://"))
    {
        (&raw[9..], true)
    } else if raw
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("uwsgi://"))
    {
        (&raw[8..], false)
    } else {
        (raw, false)
    }
}

pub(in crate::server) fn upstream_target_display(target: &ResolvedUpstreamTarget) -> String {
    if target.host.starts_with("unix:") {
        target.host.clone()
    } else {
        format!("{}:{}", target.host, target.port)
    }
}

pub(in crate::server) fn gateway_keepalive_context(
    raw: &str,
    http: &HttpConfig,
) -> Option<UpstreamKeepaliveContext> {
    let upstream = http.upstreams.get(raw)?;
    let config = upstream.keepalive.clone()?;
    (config.max_cached != 0).then(|| UpstreamKeepaliveContext {
        name: upstream.name.clone(),
        config,
    })
}
