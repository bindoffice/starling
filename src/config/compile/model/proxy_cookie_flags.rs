use super::*;

#[derive(Debug, Clone)]
pub struct ProxyCookieFlagsRule {
    pub cookie: String,
    pub regex: Option<Regex>,
    pub flags: Vec<String>,
}
