#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AutoindexFormat {
    #[default]
    Html,
    Json,
    Jsonp,
    Xml,
}
