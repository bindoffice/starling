use super::*;

#[derive(Clone, Copy)]
pub(super) enum UpstreamContext {
    Http,
    Stream,
}

pub(super) fn compile_upstream(
    directive: &Directive,
    context: UpstreamContext,
) -> ConfigResult<UpstreamConfig> {
    let name = one_arg(directive)?.to_string();
    let mut servers = Vec::new();
    let mut method = UpstreamMethod::RoundRobin;
    let mut hash_key = None;
    let mut keepalive = None;
    let mut keepalive_requests = None;
    let mut keepalive_time = None;
    let mut keepalive_timeout = None;
    for child in &directive.children {
        match child.name.as_str() {
            "server" => {
                validate_no_block(child)?;
                if child.args.is_empty() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"server\" directive",
                    ));
                }
                let mut server = UpstreamServer {
                    address: child.args[0].clone(),
                    weight: 1,
                    max_conns: 0,
                    max_fails: 1,
                    fail_timeout: Duration::from_secs(10),
                    backup: false,
                    down: false,
                    active: Arc::new(AtomicUsize::new(0)),
                    health: Arc::new(Mutex::new(UpstreamHealth {
                        failures: 0,
                        checked: None,
                    })),
                    timing: Arc::new(Mutex::new(UpstreamTiming::default())),
                };
                for option in &child.args[1..] {
                    if option == "backup" {
                        server.backup = true;
                    } else if option == "down" {
                        server.down = true;
                    } else if let Some(value) = option.strip_prefix("weight=") {
                        server.weight = parse_usize(value, &child.location, "weight")?;
                        if server.weight == 0 {
                            return Err(ConfigError::at(child.location.clone(), "invalid weight"));
                        }
                    } else if let Some(value) = option.strip_prefix("max_conns=") {
                        server.max_conns = parse_usize(value, &child.location, "max_conns")?;
                    } else if let Some(value) = option.strip_prefix("max_fails=") {
                        server.max_fails = parse_usize(value, &child.location, "max_fails")?;
                    } else if let Some(value) = option.strip_prefix("fail_timeout=") {
                        server.fail_timeout = parse_duration(value, &child.location)?;
                    } else {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{option}\""),
                        ));
                    }
                }
                servers.push(server);
            }
            "least_conn" => {
                validate_upstream_method_args(child, 0, 0)?;
                set_upstream_method(&mut method, UpstreamMethod::LeastConn, child)?;
            }
            "ip_hash" => {
                validate_upstream_method_args(child, 0, 0)?;
                set_upstream_method(&mut method, UpstreamMethod::IpHash, child)?;
            }
            "random" => {
                validate_upstream_method_args(child, 0, 2)?;
                let two = match child.args.as_slice() {
                    [] => false,
                    [parameter] if parameter == "two" => true,
                    [parameter, method] if parameter == "two" && method == "least_conn" => true,
                    [parameter, ..] => {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{parameter}\""),
                        ));
                    }
                };
                set_upstream_method(&mut method, UpstreamMethod::Random { two }, child)?;
            }
            "hash" => {
                validate_upstream_method_args(child, 1, 2)?;
                let consistent = match child.args.get(1).map(String::as_str) {
                    None => false,
                    Some("consistent") => true,
                    Some(parameter) => {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{parameter}\""),
                        ));
                    }
                };
                hash_key = Some(child.args[0].clone());
                set_upstream_method(&mut method, UpstreamMethod::Hash { consistent }, child)?;
            }
            "least_time" => {
                validate_upstream_method_args(child, 1, 2)?;
                let mode = match (context, child.args[0].as_str()) {
                    (UpstreamContext::Http, "header") => LeastTimeMode::Header,
                    (_, "last_byte") => LeastTimeMode::LastByte,
                    (UpstreamContext::Stream, "connect") => LeastTimeMode::Connect,
                    (UpstreamContext::Stream, "first_byte") => LeastTimeMode::FirstByte,
                    (_, parameter) => {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{parameter}\""),
                        ));
                    }
                };
                let inflight = match child.args.get(1).map(String::as_str) {
                    None => false,
                    Some("inflight") => true,
                    Some(parameter) => {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{parameter}\""),
                        ));
                    }
                };
                set_upstream_method(
                    &mut method,
                    UpstreamMethod::LeastTime(LeastTimeConfig { mode, inflight }),
                    child,
                )?;
            }
            "keepalive" => {
                validate_no_block(child)?;
                if keepalive.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate \"keepalive\" directive",
                    ));
                }
                if !(1..=2).contains(&child.args.len()) {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "invalid number of arguments in \"keepalive\" directive",
                    ));
                }
                let max_cached = parse_usize(child.args[0].as_str(), &child.location, "keepalive")?;
                let local = match child.args.get(1).map(String::as_str) {
                    None => false,
                    Some("local") => true,
                    Some(parameter) => {
                        return Err(ConfigError::at(
                            child.location.clone(),
                            format!("invalid parameter \"{parameter}\""),
                        ));
                    }
                };
                keepalive = Some(UpstreamKeepaliveConfig {
                    max_cached,
                    local,
                    ..UpstreamKeepaliveConfig::default()
                });
            }
            "keepalive_requests" => {
                validate_no_block(child)?;
                if keepalive_requests.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate \"keepalive_requests\" directive",
                    ));
                }
                keepalive_requests = Some(one_arg(child)?.parse::<u64>().map_err(|_| {
                    ConfigError::at(
                        child.location.clone(),
                        "invalid value in \"keepalive_requests\" directive",
                    )
                })?);
            }
            "keepalive_time" => {
                validate_no_block(child)?;
                if keepalive_time.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate \"keepalive_time\" directive",
                    ));
                }
                keepalive_time = Some(parse_duration(one_arg(child)?, &child.location)?);
            }
            "keepalive_timeout" => {
                validate_no_block(child)?;
                if keepalive_timeout.is_some() {
                    return Err(ConfigError::at(
                        child.location.clone(),
                        "duplicate \"keepalive_timeout\" directive",
                    ));
                }
                keepalive_timeout = Some(parse_duration(one_arg(child)?, &child.location)?);
            }
            "queue" | "resolver" | "resolver_timeout" | "zone" => {
                validate_no_block(child)?;
            }
            other => {
                return Err(ConfigError::at(
                    child.location.clone(),
                    format!("unknown directive \"{other}\" in upstream context"),
                ));
            }
        }
    }
    if servers.is_empty() {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("no servers are inside upstream \"{name}\""),
        ));
    }
    let mut keepalive = keepalive.unwrap_or_default();
    if let Some(requests) = keepalive_requests {
        keepalive.requests = requests;
    }
    if let Some(time) = keepalive_time {
        keepalive.time = time;
    }
    if let Some(timeout) = keepalive_timeout {
        keepalive.timeout = timeout;
    }
    Ok(UpstreamConfig {
        name,
        servers,
        method,
        hash_key,
        keepalive: Some(keepalive),
        next: Arc::new(AtomicUsize::new(0)),
    })
}

pub(super) fn validate_upstream_method_args(
    directive: &Directive,
    minimum: usize,
    maximum: usize,
) -> ConfigResult<()> {
    validate_no_block(directive)?;
    if !(minimum..=maximum).contains(&directive.args.len()) {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!(
                "invalid number of arguments in \"{}\" directive",
                directive.name
            ),
        ));
    }
    Ok(())
}

pub(super) fn set_upstream_method(
    current: &mut UpstreamMethod,
    method: UpstreamMethod,
    directive: &Directive,
) -> ConfigResult<()> {
    if *current != UpstreamMethod::RoundRobin {
        return Err(ConfigError::at(
            directive.location.clone(),
            "load balancing method redefined",
        ));
    }
    *current = method;
    Ok(())
}
