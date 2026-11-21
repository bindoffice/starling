use std::net::IpAddr;

use super::variable_value;
use crate::server::variables::Variables;

pub(in crate::server) fn append_variable_bytes(
    name: &str,
    variables: &Variables,
    stack: &mut Vec<String>,
    output: &mut Vec<u8>,
) {
    if name == "binary_remote_addr" {
        output.extend_from_slice(&binary_address_bytes(&variables.remote_addr));
        return;
    }
    output.extend_from_slice(variable_value(name, variables, stack).as_bytes());
}

pub(in crate::server) fn binary_address_bytes(value: &str) -> Vec<u8> {
    match value.parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => address.octets().to_vec(),
        Ok(IpAddr::V6(address)) => address.octets().to_vec(),
        Err(_) => value.as_bytes().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_addresses_in_network_byte_order() {
        assert_eq!(binary_address_bytes("127.0.0.1"), [127, 0, 0, 1]);
        assert_eq!(
            binary_address_bytes("2001:db8::1"),
            [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(
            binary_address_bytes("unix:/run/starling.sock"),
            b"unix:/run/starling.sock"
        );
    }
}
