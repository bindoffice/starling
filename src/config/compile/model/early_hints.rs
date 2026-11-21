#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EarlyHintsConfig {
    pub predicates: Vec<String>,
}

impl EarlyHintsConfig {
    pub fn is_configured(&self) -> bool {
        !self.predicates.is_empty()
    }
}
