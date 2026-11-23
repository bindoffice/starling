use super::*;
use crate::config::{StreamConfig, StreamServer, parse_stream_pass_target};

const MAX_STREAM_PASSES: usize = 10;

pub(in crate::server) async fn pass_stream(
    inbound: BoxedStream,
    addresses: &ProxyProtocolAddresses,
    server: &StreamServer,
    state: &SharedState,
    log: &mut StreamLogContext,
    has_preread_data: bool,
    passes: &mut usize,
) -> bool {
    if log.protocol == "UDP" {
        log.status = 500;
        log.logger
            .log(LogLevel::Error, "cannot pass udp connection");
        return false;
    }
    if has_preread_data {
        log.status = 500;
        log.logger
            .log(LogLevel::Error, "cannot pass connection with preread data");
        return false;
    }
    let Some(pass) = &server.pass else {
        log.status = 500;
        return false;
    };
    let target = if let Some(address) = &pass.address {
        address.clone()
    } else {
        let value = eval_stream_log_variables(&pass.target, log);
        let prefix = state.current_config().prefix.clone();
        match parse_stream_pass_target(&value, &prefix) {
            Ok(address) => address,
            Err(err) => {
                log.status = 500;
                log.logger
                    .log(LogLevel::Error, format!("{err} in pass \"{value}\""));
                return false;
            }
        }
    };

    *passes += 1;
    if *passes > MAX_STREAM_PASSES {
        log.status = 500;
        log.logger.log(LogLevel::Error, "stream pass cycle");
        return false;
    }

    let route = {
        let config = state.current_config();
        config
            .stream
            .as_ref()
            .and_then(|stream| stream_pass_route(stream, &target))
    };
    let Some(route) = route else {
        log.status = 200;
        log.logger
            .log(LogLevel::Error, format!("port not found for \"{target}\""));
        return false;
    };

    handle_stream_accepted_with_passes(
        inbound,
        addresses.transport_remote.clone(),
        target,
        route,
        state.clone(),
        passes,
    )
    .await;
    true
}

fn stream_pass_route(stream: &StreamConfig, target: &ListenAddress) -> Option<StreamRoute> {
    for server in &stream.servers {
        for listen in &server.listens {
            if !listen.udp && pass_address_matches(&listen.address, target) {
                return Some(StreamRoute {
                    address: listen.address.clone(),
                    ssl: listen.ssl,
                    udp: false,
                    proxy_protocol: listen.proxy_protocol,
                    socket_options: listen.socket_options.clone(),
                });
            }
        }
    }
    None
}

fn pass_address_matches(listener: &ListenAddress, target: &ListenAddress) -> bool {
    match (listener, target) {
        (ListenAddress::Inet(listener), ListenAddress::Inet(target)) => {
            listener.port() == target.port()
                && listener.ip().is_ipv4() == target.ip().is_ipv4()
                && (listener.ip().is_unspecified() || listener == target)
        }
        (ListenAddress::Unix(listener), ListenAddress::Unix(target)) => listener == target,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_wildcard_listeners_by_family_and_port() {
        let wildcard = ListenAddress::Inet("0.0.0.0:8080".parse().unwrap());
        let target = ListenAddress::Inet("127.0.0.1:8080".parse().unwrap());
        assert!(pass_address_matches(&wildcard, &target));
        assert!(!pass_address_matches(
            &wildcard,
            &ListenAddress::Inet("127.0.0.1:8081".parse().unwrap())
        ));

        let exact = ListenAddress::Inet("127.0.0.1:8080".parse().unwrap());
        assert!(pass_address_matches(&exact, &target));
        assert!(!pass_address_matches(
            &exact,
            &ListenAddress::Inet("0.0.0.0:8080".parse().unwrap())
        ));
    }
}
