use super::*;

pub(in crate::server) struct GrpcTargetSelection {
    pub(in crate::server) targets: Vec<ResolvedUpstreamTarget>,
    pub(in crate::server) tls: bool,
    pub(in crate::server) authority: String,
    pub(in crate::server) upstream: Option<String>,
}

pub(in crate::server) fn resolve_grpc_targets(
    pass: &GrpcPassConfig,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> GrpcTargetSelection {
    if pass.dynamic {
        return resolve_dynamic_grpc_target(pass, http, variables, peer);
    }
    if let Some(name) = &pass.upstream
        && let Some(upstream) = http.upstreams.get(name)
    {
        return selection_from_upstream(
            upstream,
            pass.tls,
            pass.authority.clone(),
            name,
            variables,
            peer,
        );
    }
    GrpcTargetSelection {
        targets: vec![ResolvedUpstreamTarget {
            host: pass.host.clone(),
            port: pass.port,
            peer: None,
            check_health: false,
            record_timing: false,
        }],
        tls: pass.tls,
        authority: pass.authority.clone(),
        upstream: None,
    }
}

fn resolve_dynamic_grpc_target(
    pass: &GrpcPassConfig,
    http: &HttpConfig,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> GrpcTargetSelection {
    let value = eval_variables(&pass.raw, variables);
    let grpcs_prefix = value
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grpcs://"));
    let grpc_prefix = value
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("grpc://"));
    let (target, tls) = if grpcs_prefix {
        (&value[8..], true)
    } else if grpc_prefix {
        (&value[7..], false)
    } else {
        (value.as_str(), false)
    };
    if !grpcs_prefix
        && !grpc_prefix
        && let Some(upstream) = http.upstreams.get(target)
    {
        return selection_from_upstream(upstream, tls, target.to_string(), target, variables, peer);
    }
    let Some((host, port, authority)) = parse_grpc_endpoint(target, tls) else {
        return empty_selection();
    };
    GrpcTargetSelection {
        targets: vec![ResolvedUpstreamTarget {
            host,
            port,
            peer: None,
            check_health: false,
            record_timing: false,
        }],
        tls,
        authority,
        upstream: None,
    }
}

fn selection_from_upstream(
    upstream: &UpstreamConfig,
    tls: bool,
    authority: String,
    name: &str,
    variables: &Variables,
    peer: Option<IpAddr>,
) -> GrpcTargetSelection {
    let hash_key = upstream
        .hash_key
        .as_deref()
        .map(|key| eval_variables(key, variables));
    GrpcTargetSelection {
        targets: resolve_upstream_config(upstream, peer, hash_key.as_deref()),
        tls,
        authority,
        upstream: Some(name.to_string()),
    }
}

fn parse_grpc_endpoint(target: &str, tls: bool) -> Option<(String, u16, String)> {
    if let Some(path) = target.strip_prefix("unix:") {
        return (!path.is_empty()).then(|| (format!("unix:{path}"), 0, "localhost".to_string()));
    }
    let authority = target.split_once('/').map_or(target, |(value, _)| value);
    if authority.is_empty() {
        return None;
    }
    let default_port = if tls { 443 } else { 80 };
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        if rest.is_empty() {
            return Some((format!("[{host}]"), default_port, format!("[{host}]")));
        }
        let port = rest.strip_prefix(':')?.parse::<u16>().ok()?;
        return Some((format!("[{host}]"), port, format!("[{host}]:{port}")));
    }
    if let Some((host, port)) = authority.rsplit_once(':')
        && !host.contains(':')
    {
        let port = port.parse::<u16>().ok()?;
        return Some((host.to_string(), port, format!("{host}:{port}")));
    }
    if authority.contains(':') {
        return None;
    }
    Some((authority.to_string(), default_port, authority.to_string()))
}

fn empty_selection() -> GrpcTargetSelection {
    GrpcTargetSelection {
        targets: Vec::new(),
        tls: false,
        authority: String::new(),
        upstream: None,
    }
}
