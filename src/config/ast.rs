use std::path::PathBuf;

use crate::error::SourceLocation;

#[derive(Debug, Clone)]
pub struct Directive {
    pub name: String,
    pub args: Vec<String>,
    pub children: Vec<Directive>,
    pub location: SourceLocation,
    pub has_block: bool,
}

impl Directive {
    pub fn is_block(&self) -> bool {
        self.has_block
    }

    pub fn first_arg(&self) -> Option<&str> {
        self.args.first().map(String::as_str)
    }

    pub fn path(&self) -> &PathBuf {
        &self.location.file
    }
}

#[derive(Debug, Clone)]
pub struct ConfigDocument {
    pub directives: Vec<Directive>,
    pub files: Vec<PathBuf>,
}
