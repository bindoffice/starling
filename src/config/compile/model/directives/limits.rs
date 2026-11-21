#[derive(Debug, Clone)]
pub struct LimitReqZone {
    pub name: String,
    pub key: String,
    /// Requests per second multiplied by 1000, matching Nginx's millirate.
    pub rate_millis: u64,
}

#[derive(Debug, Clone)]
pub struct LimitConnZone {
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct LimitReqLimit {
    pub zone: String,
    pub key: String,
    pub rate_millis: u64,
    pub burst: u64,
    pub delay: u64,
    pub nodelay: bool,
}

#[derive(Debug, Clone)]
pub struct LimitConnLimit {
    pub zone: String,
    pub key: String,
    pub connections: u64,
}
