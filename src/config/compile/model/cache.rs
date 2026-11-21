use super::*;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ProxyCacheConfig {
    #[default]
    Off,
    Zone(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyCacheMethods(u8);

impl ProxyCacheMethods {
    const GET: u8 = 1 << 0;
    const HEAD: u8 = 1 << 1;
    const POST: u8 = 1 << 2;

    pub fn allows(&self, method: &str) -> bool {
        let flag = if method.eq_ignore_ascii_case("GET") {
            Self::GET
        } else if method.eq_ignore_ascii_case("HEAD") {
            Self::HEAD
        } else if method.eq_ignore_ascii_case("POST") {
            Self::POST
        } else {
            return false;
        };
        self.0 & flag != 0
    }

    pub(in crate::config::compile) fn insert(&mut self, method: &str) -> bool {
        let flag = if method.eq_ignore_ascii_case("GET") {
            Self::GET
        } else if method.eq_ignore_ascii_case("HEAD") {
            Self::HEAD
        } else if method.eq_ignore_ascii_case("POST") {
            Self::POST
        } else {
            return false;
        };
        self.0 |= flag;
        true
    }
}

impl Default for ProxyCacheMethods {
    fn default() -> Self {
        Self(Self::GET | Self::HEAD)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCacheValid {
    pub statuses: Vec<u16>,
    pub valid: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCacheLockConfig {
    pub enabled: bool,
    pub timeout: Duration,
    pub age: Duration,
}

impl Default for ProxyCacheLockConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout: Duration::from_secs(5),
            age: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayCacheConfig {
    pub cache: ProxyCacheConfig,
    pub key: String,
    pub valid: Vec<ProxyCacheValid>,
    pub min_uses: usize,
    pub max_range_offset: Option<u64>,
    pub methods: ProxyCacheMethods,
    pub bypass: Vec<String>,
    pub no_cache: Vec<String>,
    pub lock: ProxyCacheLockConfig,
    pub revalidate: bool,
    pub background_update: bool,
    pub use_stale: ProxyCacheUseStaleConfig,
}

impl Default for GatewayCacheConfig {
    fn default() -> Self {
        Self {
            cache: ProxyCacheConfig::Off,
            key: String::new(),
            valid: Vec::new(),
            min_uses: 1,
            max_range_offset: None,
            methods: ProxyCacheMethods::default(),
            bypass: Vec::new(),
            no_cache: Vec::new(),
            lock: ProxyCacheLockConfig::default(),
            revalidate: false,
            background_update: false,
            use_stale: ProxyCacheUseStaleConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProxyCacheUseStaleConfig {
    pub(in crate::config::compile) flags: u32,
}

pub(in crate::config::compile) const CACHE_STALE_ERROR: u32 = 1 << 0;
pub(in crate::config::compile) const CACHE_STALE_TIMEOUT: u32 = 1 << 1;
pub(in crate::config::compile) const CACHE_STALE_INVALID_HEADER: u32 = 1 << 2;
pub(in crate::config::compile) const CACHE_STALE_UPDATING: u32 = 1 << 3;
pub(in crate::config::compile) const CACHE_STALE_HTTP_403: u32 = 1 << 4;
pub(in crate::config::compile) const CACHE_STALE_HTTP_404: u32 = 1 << 5;
pub(in crate::config::compile) const CACHE_STALE_HTTP_429: u32 = 1 << 6;
pub(in crate::config::compile) const CACHE_STALE_HTTP_500: u32 = 1 << 7;
pub(in crate::config::compile) const CACHE_STALE_HTTP_502: u32 = 1 << 8;
pub(in crate::config::compile) const CACHE_STALE_HTTP_503: u32 = 1 << 9;
pub(in crate::config::compile) const CACHE_STALE_HTTP_504: u32 = 1 << 10;
pub(in crate::config::compile) const CACHE_STALE_OFF: u32 = 1 << 31;

impl ProxyCacheUseStaleConfig {
    pub fn error(&self) -> bool {
        self.enabled() && self.flags & CACHE_STALE_ERROR != 0
    }

    pub fn timeout(&self) -> bool {
        self.enabled() && self.flags & CACHE_STALE_TIMEOUT != 0
    }

    pub fn invalid_header(&self) -> bool {
        self.enabled() && self.flags & CACHE_STALE_INVALID_HEADER != 0
    }

    pub fn updating(&self) -> bool {
        self.enabled() && self.flags & CACHE_STALE_UPDATING != 0
    }

    pub fn status(&self, status: u16) -> bool {
        if !self.enabled() {
            return false;
        }
        let flag = match status {
            403 => CACHE_STALE_HTTP_403,
            404 => CACHE_STALE_HTTP_404,
            429 => CACHE_STALE_HTTP_429,
            500 => CACHE_STALE_HTTP_500,
            502 => CACHE_STALE_HTTP_502,
            503 => CACHE_STALE_HTTP_503,
            504 => CACHE_STALE_HTTP_504,
            _ => return false,
        };
        self.flags & flag != 0
    }

    fn enabled(&self) -> bool {
        self.flags & CACHE_STALE_OFF == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyCachePathConfig {
    pub path: PathBuf,
    pub levels: [usize; 3],
    pub keys_zone: String,
    pub keys_zone_size: usize,
    pub inactive: Duration,
    pub max_size: Option<u64>,
    pub use_temp_path: bool,
    pub loader_files: usize,
    pub loader_sleep: Duration,
    pub loader_threshold: Duration,
    pub manager_files: usize,
    pub manager_sleep: Duration,
    pub manager_threshold: Duration,
    pub min_free: Option<u64>,
}
