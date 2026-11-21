#[derive(Debug, Clone)]
pub enum ProxyRedirectConfig {
    Off,
    Rules(Vec<ProxyRedirectRule>),
}

#[derive(Debug, Clone)]
pub enum ProxyRedirectRule {
    Default,
    Pattern {
        pattern: String,
        replacement: String,
    },
}

#[derive(Debug, Clone)]
pub struct ProxyCookieRule {
    pub pattern: String,
    pub replacement: String,
}

#[derive(Debug, Clone)]
pub struct ProxyPassConfig {
    pub raw: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub uri: Option<String>,
    pub upstream: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FastCgiPassConfig {
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct UwsgiPassConfig {
    pub raw: String,
    pub tls: bool,
}

#[derive(Debug, Clone)]
pub struct ScgiPassConfig {
    pub raw: String,
}

#[derive(Debug, Clone)]
pub struct FastCgiParam {
    pub name: String,
    pub value: String,
    pub if_not_empty: bool,
}
