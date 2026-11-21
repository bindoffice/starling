#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserIdEnable {
    #[default]
    Off,
    Log,
    V1,
    On,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum UserIdExpires {
    #[default]
    Off,
    Max,
    Seconds(u64),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UserIdFlags {
    pub secure: bool,
    pub http_only: bool,
    pub same_site_strict: bool,
    pub same_site_lax: bool,
    pub same_site_none: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdConfig {
    pub enable: UserIdEnable,
    pub service: Option<u32>,
    pub name: String,
    pub domain: Option<String>,
    pub path: String,
    pub expires: UserIdExpires,
    pub flags: UserIdFlags,
    pub p3p: Option<String>,
    pub mark: Option<u8>,
}

impl Default for UserIdConfig {
    fn default() -> Self {
        Self {
            enable: UserIdEnable::Off,
            service: None,
            name: "uid".into(),
            domain: None,
            path: "/".into(),
            expires: UserIdExpires::Off,
            flags: UserIdFlags::default(),
            p3p: None,
            mark: None,
        }
    }
}
