#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreConfig {
    Off,
    On,
    Path(String),
}

pub type ProxyStoreConfig = StoreConfig;
