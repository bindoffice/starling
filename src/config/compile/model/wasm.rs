use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;

use crate::wasm::WasmModule;

const DEFAULT_WASM_CACHE_SIZE: u64 = 64 * 1024 * 1024;
const DEFAULT_WASM_TIMEOUT: Duration = Duration::from_millis(100);
const DEFAULT_WASM_MEMORY_MAX: u64 = 64 * 1024 * 1024;
const DEFAULT_WASM_REQUEST_BODY_MAX: u64 = 1024 * 1024;
const DEFAULT_WASM_RESPONSE_BODY_MAX: u64 = 8 * 1024 * 1024;
const DEFAULT_WASM_MAX_INSTANCES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmPhase {
    Content,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmIsolation {
    InProcess,
    Process,
}

#[derive(Clone)]
pub struct WasmPlugin {
    pub path: PathBuf,
    pub module: Arc<WasmModule>,
}

impl std::fmt::Debug for WasmPlugin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WasmPlugin")
            .field("path", &self.path)
            .field("module", &self.module)
            .finish()
    }
}

#[derive(Clone)]
pub struct WasmConfig {
    pub engine: bool,
    pub module_cache_size: u64,
    pub plugin: Option<WasmPlugin>,
    pub phase: WasmPhase,
    pub timeout: Duration,
    pub memory_max: u64,
    pub fuel: Option<u64>,
    pub request_body_max: u64,
    pub response_body_max: u64,
    pub max_instances: usize,
    pub isolation: WasmIsolation,
    pub instance_limiter: Arc<Semaphore>,
}

impl std::fmt::Debug for WasmConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WasmConfig")
            .field("engine", &self.engine)
            .field("module_cache_size", &self.module_cache_size)
            .field("plugin", &self.plugin)
            .field("phase", &self.phase)
            .field("timeout", &self.timeout)
            .field("memory_max", &self.memory_max)
            .field("fuel", &self.fuel)
            .field("request_body_max", &self.request_body_max)
            .field("response_body_max", &self.response_body_max)
            .field("max_instances", &self.max_instances)
            .field("isolation", &self.isolation)
            .finish_non_exhaustive()
    }
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            engine: false,
            module_cache_size: DEFAULT_WASM_CACHE_SIZE,
            plugin: None,
            phase: WasmPhase::Content,
            timeout: DEFAULT_WASM_TIMEOUT,
            memory_max: DEFAULT_WASM_MEMORY_MAX,
            fuel: None,
            request_body_max: DEFAULT_WASM_REQUEST_BODY_MAX,
            response_body_max: DEFAULT_WASM_RESPONSE_BODY_MAX,
            max_instances: DEFAULT_WASM_MAX_INSTANCES,
            isolation: WasmIsolation::InProcess,
            instance_limiter: Arc::new(Semaphore::new(DEFAULT_WASM_MAX_INSTANCES)),
        }
    }
}

impl WasmConfig {
    pub fn has_plugin(&self) -> bool {
        self.plugin.is_some()
    }

    pub(in crate::config::compile) fn install_plugin(
        &mut self,
        path: PathBuf,
        module: Arc<WasmModule>,
    ) {
        self.plugin = Some(WasmPlugin { path, module });
        self.instance_limiter = Arc::new(Semaphore::new(self.max_instances));
    }

    pub(in crate::config::compile) fn set_max_instances(&mut self, max_instances: usize) {
        self.max_instances = max_instances;
        self.instance_limiter = Arc::new(Semaphore::new(max_instances));
    }
}
