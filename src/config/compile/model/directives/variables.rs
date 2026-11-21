use super::*;

#[derive(Debug, Clone)]
pub struct MapVariable {
    pub name: String,
    pub source: String,
    pub default: Option<String>,
    pub hostnames: bool,
    pub volatile: bool,
    pub exact: HashMap<String, String>,
    pub leading_wildcards: Vec<(String, String)>,
    pub trailing_wildcards: Vec<(String, String)>,
    pub regexes: Vec<(Regex, String)>,
}

#[derive(Debug, Clone)]
pub struct GeoVariable {
    pub name: String,
    pub source: String,
    pub default: Option<String>,
    pub networks: Vec<(IpNetwork, String)>,
    pub ranges: bool,
    pub range_entries: Vec<(IpRange, String)>,
}

#[derive(Debug, Clone)]
pub struct SplitClientsVariable {
    pub name: String,
    pub source: String,
    pub parts: Vec<SplitClientsPart>,
}

#[derive(Debug, Clone)]
pub struct SplitClientsPart {
    pub percent: u32,
    pub threshold: u32,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone)]
pub struct SetVariable {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct AuthRequestSet {
    pub name: String,
    pub value: String,
}
