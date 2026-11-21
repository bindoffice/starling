#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DavMethod {
    Put,
    Delete,
    MkCol,
    Copy,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DavConfig {
    pub methods: Vec<DavMethod>,
    pub create_full_put_path: bool,
    pub min_delete_depth: usize,
    pub access: u32,
}

impl Default for DavConfig {
    fn default() -> Self {
        Self {
            methods: Vec::new(),
            create_full_put_path: false,
            min_delete_depth: 0,
            access: 0o600,
        }
    }
}

impl DavConfig {
    pub fn allows(&self, method: DavMethod) -> bool {
        self.methods.contains(&method)
    }
}
