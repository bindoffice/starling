#[derive(Debug, Clone)]
pub struct BrowserConfig {
    pub modern: Vec<ModernBrowserRule>,
    pub modern_unlisted: bool,
    pub ancient: Vec<String>,
    pub netscape4: bool,
    pub modern_value: String,
    pub ancient_value: String,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            modern: Vec::new(),
            modern_unlisted: false,
            ancient: Vec::new(),
            netscape4: false,
            modern_value: "1".into(),
            ancient_value: "1".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BrowserKind {
    Opera,
    Msie,
    Gecko,
    Safari,
    Konqueror,
}

#[derive(Debug, Clone)]
pub struct ModernBrowserRule {
    pub browser: BrowserKind,
    pub version: u32,
}
