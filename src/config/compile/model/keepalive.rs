#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepaliveDisableConfig {
    pub msie6: bool,
    pub safari: bool,
}

impl Default for KeepaliveDisableConfig {
    fn default() -> Self {
        Self {
            msie6: true,
            safari: false,
        }
    }
}
