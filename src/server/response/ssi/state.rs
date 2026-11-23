use std::collections::HashMap;

pub(super) const DEFAULT_TIME_FORMAT: &str = "%A, %d-%b-%Y %H:%M:%S %Z";
const DEFAULT_ERROR: &str = "[an error occurred while processing the directive]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Encoding {
    None,
    Url,
    Entity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Conditional {
    None,
    If,
    Else,
}

pub(super) struct State {
    pub(super) variables: HashMap<String, String>,
    pub(super) blocks: Vec<(String, Vec<u8>)>,
    pub(super) captures: Vec<String>,
    pub(super) named_captures: HashMap<String, String>,
    pub(super) output: bool,
    pub(super) output_chosen: bool,
    pub(super) conditional: Conditional,
    pub(super) block: Option<String>,
    pub(super) block_buffer: Vec<u8>,
    pub(super) encoding: Encoding,
    pub(super) errmsg: String,
    pub(super) timefmt: String,
}

impl State {
    pub(super) fn new() -> Self {
        Self {
            variables: HashMap::new(),
            blocks: Vec::new(),
            captures: Vec::new(),
            named_captures: HashMap::new(),
            output: true,
            output_chosen: false,
            conditional: Conditional::None,
            block: None,
            block_buffer: Vec::new(),
            encoding: Encoding::Entity,
            errmsg: DEFAULT_ERROR.into(),
            timefmt: DEFAULT_TIME_FORMAT.into(),
        }
    }
}
