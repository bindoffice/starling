use super::*;

pub(in crate::server) fn stream_geo_value(
    geo: &GeoVariable,
    log: &StreamLogContext,
    stack: &mut Vec<String>,
    captures: &StreamCaptures,
) -> String {
    if stack.iter().any(|name| name == &geo.name) {
        return String::new();
    }
    stack.push(geo.name.clone());

    let source = eval_stream_variables_with_stack(&geo.source, log, stack, captures);
    let address =
        source
            .split(',')
            .next()
            .and_then(parse_geo_address)
            .map(|address| match address {
                IpAddr::V6(address) => address
                    .to_ipv4_mapped()
                    .map(IpAddr::V4)
                    .unwrap_or(IpAddr::V6(address)),
                address => address,
            });
    let value = if geo.ranges {
        address
            .and_then(|address| match address {
                IpAddr::V4(address) => {
                    let address = u32::from(address);
                    geo.range_entries
                        .iter()
                        .find(|(range, _)| (range.start..=range.end).contains(&address))
                        .map(|(_, value)| value)
                }
                IpAddr::V6(_) => None,
            })
            .or(geo.default.as_ref())
    } else {
        address
            .and_then(|address| {
                geo.networks
                    .iter()
                    .find(|(network, _)| network.contains(address))
                    .map(|(_, value)| value)
            })
            .or(geo.default.as_ref())
    };
    let output = value.cloned().unwrap_or_default();

    stack.pop();
    output
}
