#[derive(Debug, Clone, Default)]
pub struct SecureLinkConfig {
    pub variable: Option<String>,
    pub md5: Option<String>,
    pub secret: Option<String>,
}
