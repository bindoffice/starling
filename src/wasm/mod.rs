use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use sha1::{Digest, Sha1};
use wasmtime::{ExternType, Module, ValType};

mod abi;
mod runtime;

pub(crate) use runtime::{call, validate_abi_version};

static MODULE_CACHE: OnceLock<Mutex<ModuleCache>> = OnceLock::new();

#[derive(Clone)]
pub struct WasmModule {
    module: Module,
    path: PathBuf,
    digest: String,
    source_len: u64,
}

impl std::fmt::Debug for WasmModule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WasmModule")
            .field("path", &self.path)
            .field("digest", &self.digest)
            .field("source_len", &self.source_len)
            .finish()
    }
}

impl WasmModule {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn source_len(&self) -> u64 {
        self.source_len
    }
}

#[derive(Debug, Clone)]
pub struct WasmRequest {
    pub method: String,
    pub target: String,
    pub uri: String,
    pub args: String,
    pub version: String,
    pub scheme: String,
    pub host: String,
    pub remote_addr: String,
    pub local_addr: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct WasmResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmErrorKind {
    Module,
    Abi,
    Trap,
    Timeout,
    MemoryLimit,
    Busy,
    Internal,
}

#[derive(Debug, Clone)]
pub struct WasmError {
    kind: WasmErrorKind,
    message: String,
}

impl WasmError {
    fn new(kind: WasmErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn kind(&self) -> WasmErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WasmError {}

#[derive(Debug, Clone, Copy)]
pub struct WasmLimits {
    pub timeout: Duration,
    pub memory_max: u64,
    pub fuel: Option<u64>,
    pub response_body_max: u64,
}

#[derive(Debug)]
pub struct WasmCallStats {
    pub elapsed: Duration,
    pub fuel_used: Option<u64>,
    pub memory_bytes: usize,
}

struct ModuleCache {
    entries: HashMap<PathBuf, CacheEntry>,
    total_source_bytes: u64,
    max_source_bytes: u64,
}

struct CacheEntry {
    module: Arc<WasmModule>,
    modified: Option<SystemTime>,
    source_len: u64,
    last_used: Instant,
}

impl ModuleCache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            total_source_bytes: 0,
            max_source_bytes: 64 * 1024 * 1024,
        }
    }

    fn set_limit(&mut self, limit: u64) {
        if limit == 0 {
            return;
        }
        self.max_source_bytes = limit;
        self.evict();
    }

    fn evict(&mut self) {
        while self.total_source_bytes > self.max_source_bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.total_source_bytes = self.total_source_bytes.saturating_sub(entry.source_len);
            }
        }
    }
}

pub fn load_module(path: &Path, cache_limit: u64) -> Result<Arc<WasmModule>, WasmError> {
    let canonical = fs::canonicalize(path).map_err(|error| {
        WasmError::new(
            WasmErrorKind::Module,
            format!("cannot open WASM module {}: {error}", path.display()),
        )
    })?;
    let metadata = fs::metadata(&canonical).map_err(|error| {
        WasmError::new(
            WasmErrorKind::Module,
            format!("cannot stat WASM module {}: {error}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(WasmError::new(
            WasmErrorKind::Module,
            format!("WASM module {} is not a regular file", path.display()),
        ));
    }
    let modified = metadata.modified().ok();
    let bytes = fs::read(&canonical).map_err(|error| {
        WasmError::new(
            WasmErrorKind::Module,
            format!("cannot read WASM module {}: {error}", path.display()),
        )
    })?;
    let source_len = bytes.len() as u64;
    let digest = format!("{:x}", Sha1::digest(&bytes));
    let cache = MODULE_CACHE.get_or_init(|| Mutex::new(ModuleCache::new()));
    {
        let mut cache = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cache.set_limit(cache_limit);
        if let Some(entry) = cache.entries.get_mut(&canonical)
            && entry.source_len == source_len
            && entry.modified == modified
            && entry.module.digest() == digest.as_str()
        {
            entry.last_used = Instant::now();
            return Ok(entry.module.clone());
        }
    }

    let engine = runtime::engine()?;
    let module = Module::new(engine, &bytes).map_err(|error| {
        WasmError::new(
            WasmErrorKind::Module,
            format!("cannot compile WASM module {}: {error}", path.display()),
        )
    })?;
    validate_abi_exports(&module, &canonical)?;
    let module = Arc::new(WasmModule {
        module,
        path: canonical.clone(),
        digest,
        source_len,
    });

    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(previous) = cache.entries.insert(
        canonical,
        CacheEntry {
            module: module.clone(),
            modified,
            source_len,
            last_used: Instant::now(),
        },
    ) {
        cache.total_source_bytes = cache
            .total_source_bytes
            .saturating_add(source_len)
            .saturating_sub(previous.source_len);
    } else {
        cache.total_source_bytes = cache.total_source_bytes.saturating_add(source_len);
    }
    cache.evict();
    Ok(module)
}

fn validate_abi_exports(module: &Module, path: &Path) -> Result<(), WasmError> {
    if let Some(import) = module.imports().next() {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} imports {}.{}, but host imports are not supported",
                path.display(),
                import.module(),
                import.name()
            ),
        ));
    }

    let memory = module.get_export("memory").ok_or_else(|| {
        WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} does not export linear memory as \"memory\"",
                path.display()
            ),
        )
    })?;
    let ExternType::Memory(memory) = memory else {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} export \"memory\" is not linear memory",
                path.display()
            ),
        ));
    };
    if memory.is_64() || memory.is_shared() {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} must export a non-shared 32-bit linear memory",
                path.display()
            ),
        ));
    }

    validate_function_export(module, path, "starling_abi_version", &[], &[ValType::I32])?;
    validate_function_export(
        module,
        path,
        "starling_alloc",
        &[ValType::I32],
        &[ValType::I32],
    )?;
    validate_function_export(
        module,
        path,
        "starling_handle",
        &[ValType::I32, ValType::I32],
        &[ValType::I64],
    )
}

fn validate_function_export(
    module: &Module,
    path: &Path,
    name: &str,
    expected_params: &[ValType],
    expected_results: &[ValType],
) -> Result<(), WasmError> {
    let Some(export) = module.get_export(name) else {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} does not export required function \"{name}\"",
                path.display()
            ),
        ));
    };
    let ExternType::Func(function) = export else {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} export \"{name}\" is not a function",
                path.display()
            ),
        ));
    };
    let params: Vec<_> = function.params().collect();
    let results: Vec<_> = function.results().collect();
    if !val_types_match(&params, expected_params) || !val_types_match(&results, expected_results) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!(
                "WASM module {} export \"{name}\" has signature {function}; expected {expected_params:?} -> {expected_results:?}",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn val_types_match(actual: &[ValType], expected: &[ValType]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| match expected {
                ValType::I32 => actual.is_i32(),
                ValType::I64 => actual.is_i64(),
                _ => false,
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_cache_reuses_an_unchanged_module() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/wasm/hello.wat");
        let first = load_module(&path, 64 * 1024 * 1024).unwrap();
        let second = load_module(&path, 64 * 1024 * 1024).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
    }
}
