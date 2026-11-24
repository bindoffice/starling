use super::*;

pub(in crate::server) fn resolve_location<'a>(
    locations: &'a [LocationConfig],
    uri: &str,
) -> Option<&'a LocationConfig> {
    let mut current = locations;
    let mut selected = None;
    while let Some(location) = match_location(current, uri) {
        selected = Some(location);
        if location.nested.is_empty() {
            break;
        }
        current = &location.nested;
    }
    selected
}

pub(in crate::server) fn find_named_location<'a>(
    locations: &'a [LocationConfig],
    name: &str,
) -> Option<&'a LocationConfig> {
    for location in locations {
        if matches!(&location.matcher, LocationMatcher::Named(value) if value == name) {
            return Some(location);
        }
        if let Some(found) = find_named_location(&location.nested, name) {
            return Some(found);
        }
    }
    None
}

pub(in crate::server) fn access_allowed(rules: &[AccessRule], remote_addr: &str) -> bool {
    let ip = remote_addr
        .parse::<std::net::IpAddr>()
        .ok()
        .or_else(|| remote_addr.rsplit_once(':')?.0.parse().ok());
    for rule in rules {
        match rule {
            AccessRule::AllowAll => return true,
            AccessRule::DenyAll => return false,
            AccessRule::Allow(network) => {
                if ip.is_some_and(|ip| network.contains(ip)) {
                    return true;
                }
            }
            AccessRule::Deny(network) => {
                if ip.is_some_and(|ip| network.contains(ip)) {
                    return false;
                }
            }
        }
    }
    true
}

pub(in crate::server) fn limit_except_applies(limit_except: &LimitExcept, method: &str) -> bool {
    if method.eq_ignore_ascii_case("HEAD")
        && limit_except
            .methods
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case("GET"))
    {
        return false;
    }
    !limit_except
        .methods
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(method))
}

pub(in crate::server) fn apply_limit_except(context: &mut Arc<EffectiveConfig>, request: &Request) {
    if !context
        .limit_except
        .as_ref()
        .is_some_and(|limit_except| limit_except_applies(limit_except, &request.method))
    {
        return;
    }
    let limit_except = context
        .limit_except
        .clone()
        .expect("limit_except checked above");
    let context = Arc::make_mut(context);
    if let Some(proxy_pass) = &limit_except.proxy_pass {
        context.proxy_pass = Some(proxy_pass.clone());
        context.grpc.pass = None;
        context.fastcgi_pass = None;
        context.uwsgi_pass = None;
        context.scgi_pass = None;
        context.memcached_pass = None;
        context.tunnel.pass = None;
    }
    context.auth_basic = limit_except.auth_basic.clone();
    context.auth_basic_user_file = limit_except.auth_basic_user_file.clone();
    context.access_log = limit_except.access_log.clone();
}
