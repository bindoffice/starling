use crate::config::Regex;

#[derive(Debug, Clone, Default)]
pub struct ValidReferers {
    pub no_referer: bool,
    pub blocked_referer: bool,
    pub server_names: bool,
    pub hosts: Vec<RefererHostRule>,
    pub regexes: Vec<Regex>,
}

impl ValidReferers {
    pub fn is_configured(&self) -> bool {
        self.no_referer
            || self.blocked_referer
            || self.server_names
            || !self.hosts.is_empty()
            || !self.regexes.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct RefererHostRule {
    pub host: RefererHost,
    pub uri: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RefererHost {
    Exact(String),
    LeadingWildcard(String),
    TrailingWildcard(String),
}

impl RefererHost {
    pub fn matches(&self, host: &str) -> bool {
        match self {
            Self::Exact(name) => name.eq_ignore_ascii_case(host),
            Self::LeadingWildcard(suffix) => {
                let bare = suffix.strip_prefix('.').unwrap_or(suffix);
                host.eq_ignore_ascii_case(bare)
                    || (host.len() > suffix.len()
                        && host
                            .get(host.len() - suffix.len()..)
                            .is_some_and(|tail| tail.eq_ignore_ascii_case(suffix)))
            }
            Self::TrailingWildcard(prefix) => {
                let bare = prefix.strip_suffix('.').unwrap_or(prefix);
                host.eq_ignore_ascii_case(bare)
                    || (host.len() > prefix.len()
                        && host
                            .get(..prefix.len())
                            .is_some_and(|head| head.eq_ignore_ascii_case(prefix)))
            }
        }
    }
}
