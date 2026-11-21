#[derive(Debug, Clone)]
pub struct HeaderValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone)]
pub struct AddHeader {
    pub name: String,
    pub value: String,
    pub always: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderInheritance {
    Off,
    On,
    Merge,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ServerTokens {
    Off,
    #[default]
    On,
    Build,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LingeringClose {
    Off,
    #[default]
    On,
    Always,
}

impl HeaderInheritance {
    pub fn merge(self, inherited: &mut Vec<AddHeader>, child: Vec<AddHeader>) {
        match self {
            Self::On => {
                if !child.is_empty() {
                    *inherited = child;
                }
            }
            Self::Off => {
                *inherited = child;
            }
            Self::Merge => {
                if !child.is_empty() {
                    let parent = std::mem::take(inherited);
                    *inherited = child;
                    inherited.extend(parent);
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrorPage {
    pub statuses: Vec<u16>,
    pub overwrite: ErrorPageOverwrite,
    pub uri: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorPageOverwrite {
    Preserve,
    Response,
    Status(u16),
}
