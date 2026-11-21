#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DisableSymlinksMode {
    #[default]
    Off,
    On,
    IfNotOwner,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DisableSymlinksConfig {
    pub mode: DisableSymlinksMode,
    pub from: Option<String>,
}
