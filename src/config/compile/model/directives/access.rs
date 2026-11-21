use super::*;

#[derive(Debug, Clone)]
pub struct LimitExcept {
    pub methods: Vec<String>,
    pub access_rules: Vec<AccessRule>,
    pub proxy_pass: Option<ProxyPassConfig>,
    pub auth_basic: Option<String>,
    pub auth_basic_user_file: Option<String>,
    pub access_log: AccessLogConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessRule {
    AllowAll,
    DenyAll,
    Allow(IpNetwork),
    Deny(IpNetwork),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpNetwork {
    pub address: IpAddr,
    pub prefix: u8,
}

impl IpNetwork {
    pub fn contains(&self, candidate: IpAddr) -> bool {
        match (self.address, candidate) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u32::MAX << (32 - self.prefix)
                };
                (u32::from(network) & mask) == (u32::from(candidate) & mask)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                let mask = if self.prefix == 0 {
                    0
                } else {
                    u128::MAX << (128 - self.prefix)
                };
                (u128::from(network) & mask) == (u128::from(candidate) & mask)
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealIpSource {
    Network(IpNetwork),
    Unix,
}

impl RealIpSource {
    pub fn contains(&self, candidate: IpAddr) -> bool {
        match self {
            Self::Network(network) => network.contains(candidate),
            Self::Unix => false,
        }
    }

    pub fn is_unix(&self) -> bool {
        matches!(self, Self::Unix)
    }

    pub fn network(&self) -> Option<&IpNetwork> {
        match self {
            Self::Network(network) => Some(network),
            Self::Unix => None,
        }
    }
}
