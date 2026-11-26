use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use wasmtime::{Config, Engine, Instance, Memory, ResourceLimiter, Store, TypedFunc};

use super::abi::{ABI_VERSION, MAX_WASM_RESPONSE_OVERHEAD, decode_response, encode_request};
use super::{
    WasmCallStats, WasmError, WasmErrorKind, WasmLimits, WasmModule, WasmRequest, WasmResponse,
};

const MAX_WASM_TABLE_ELEMENTS: usize = 100_000;
const EPOCH_TICK: Duration = Duration::from_millis(10);

static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
static EPOCH_TICKER: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn validate_abi_version(
    module: &WasmModule,
    timeout: Duration,
    memory_max: u64,
) -> Result<(), WasmError> {
    let engine = engine()?;
    ensure_epoch_ticker(engine)?;
    let memory_max = usize::try_from(memory_max).map_err(|_| {
        WasmError::new(WasmErrorKind::MemoryLimit, "WASM memory limit is too large")
    })?;
    let mut store = Store::new(
        engine,
        StoreData {
            limiter: GuestLimiter::new(memory_max),
        },
    );
    store.limiter(|data| &mut data.limiter);
    store.set_fuel(u64::MAX).map_err(|error| {
        WasmError::new(
            WasmErrorKind::Internal,
            format!("cannot configure WASM fuel: {error}"),
        )
    })?;
    store.set_epoch_deadline(epoch_deadline_ticks(timeout));
    let instance = Instance::new(&mut store, &module.module, &[])
        .map_err(|error| classify_runtime_error(&store, "instantiate WASM module", error))?;
    let abi_version: TypedFunc<(), i32> = instance
        .get_typed_func(&mut store, "starling_abi_version")
        .map_err(|error| abi_error("starling_abi_version", error))?;
    let version = abi_version
        .call(&mut store, ())
        .map_err(|error| classify_runtime_error(&store, "call starling_abi_version", error))?;
    if version != ABI_VERSION {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            format!("unsupported WASM ABI version {version}; Starling requires {ABI_VERSION}"),
        ));
    }
    Ok(())
}

pub(crate) async fn call(
    module: Arc<WasmModule>,
    request: WasmRequest,
    limits: WasmLimits,
) -> Result<(WasmResponse, WasmCallStats), WasmError> {
    let started = Instant::now();
    let engine = engine()?;
    ensure_epoch_ticker(engine)?;
    let request_bytes = encode_request(&request)?;
    let timeout = limits.timeout.max(Duration::from_millis(1));
    let fuel = limits.fuel;
    let memory_max = usize::try_from(limits.memory_max).map_err(|_| {
        WasmError::new(WasmErrorKind::MemoryLimit, "WASM memory limit is too large")
    })?;
    let response_limit = limits
        .response_body_max
        .saturating_add(MAX_WASM_RESPONSE_OVERHEAD as u64);

    let call = async move {
        let mut store = Store::new(
            engine,
            StoreData {
                limiter: GuestLimiter::new(memory_max),
            },
        );
        store.limiter(|data| &mut data.limiter);
        store.set_fuel(fuel.unwrap_or(u64::MAX)).map_err(|error| {
            WasmError::new(
                WasmErrorKind::Internal,
                format!("cannot configure WASM fuel: {error}"),
            )
        })?;
        store.set_epoch_deadline(epoch_deadline_ticks(timeout));

        let instance = Instance::new_async(&mut store, &module.module, &[])
            .await
            .map_err(|error| classify_runtime_error(&store, "instantiate WASM module", error))?;
        let memory = instance
            .get_export(&mut store, "memory")
            .and_then(|export| export.into_memory())
            .ok_or_else(|| {
                WasmError::new(
                    WasmErrorKind::Abi,
                    "WASM module does not export linear memory as \"memory\"",
                )
            })?;
        let abi_version: TypedFunc<(), i32> = instance
            .get_typed_func(&mut store, "starling_abi_version")
            .map_err(|error| abi_error("starling_abi_version", error))?;
        let version = abi_version
            .call_async(&mut store, ())
            .await
            .map_err(|error| classify_runtime_error(&store, "call starling_abi_version", error))?;
        if version != ABI_VERSION {
            return Err(WasmError::new(
                WasmErrorKind::Abi,
                format!("unsupported WASM ABI version {version}; Starling requires {ABI_VERSION}"),
            ));
        }
        let alloc: TypedFunc<i32, i32> = instance
            .get_typed_func(&mut store, "starling_alloc")
            .map_err(|error| abi_error("starling_alloc", error))?;
        let handler: TypedFunc<(i32, i32), i64> = instance
            .get_typed_func(&mut store, "starling_handle")
            .map_err(|error| abi_error("starling_handle", error))?;
        let request_len = i32::try_from(request_bytes.len()).map_err(|_| {
            WasmError::new(
                WasmErrorKind::Abi,
                "WASM request exceeds the 32-bit ABI limit",
            )
        })?;
        let request_ptr = alloc
            .call_async(&mut store, request_len)
            .await
            .map_err(|error| {
                classify_runtime_error(&store, "allocate WASM request buffer", error)
            })?;
        validate_guest_range(&store, &memory, request_ptr, request_len)?;
        memory
            .write(&mut store, request_ptr as usize, &request_bytes)
            .map_err(|error| {
                WasmError::new(
                    WasmErrorKind::Abi,
                    format!("cannot write WASM request buffer: {error}"),
                )
            })?;
        let packed = handler
            .call_async(&mut store, (request_ptr, request_len))
            .await
            .map_err(|error| classify_runtime_error(&store, "call WASM handler", error))?;
        let packed = packed as u64;
        let response_ptr = (packed >> 32) as u32;
        let response_len = packed as u32;
        if response_len == 0 {
            return Err(WasmError::new(
                WasmErrorKind::Abi,
                "WASM handler returned an empty response",
            ));
        }
        if u64::from(response_len) > response_limit {
            return Err(WasmError::new(
                WasmErrorKind::Abi,
                format!("WASM response exceeds configured limit of {response_limit} bytes"),
            ));
        }
        validate_guest_range(&store, &memory, response_ptr as i32, response_len as i32)?;
        let mut response_bytes = vec![0_u8; response_len as usize];
        memory
            .read(&store, response_ptr as usize, &mut response_bytes)
            .map_err(|error| {
                WasmError::new(
                    WasmErrorKind::Abi,
                    format!("cannot read WASM response: {error}"),
                )
            })?;
        let response = decode_response(&response_bytes, limits.response_body_max)?;
        let fuel_used = fuel.and_then(|initial| {
            store
                .get_fuel()
                .ok()
                .map(|remaining| initial.saturating_sub(remaining))
        });
        let elapsed = started.elapsed();
        Ok((
            response,
            WasmCallStats {
                elapsed,
                fuel_used,
                memory_bytes: memory.data_size(&store),
            },
        ))
    };

    match tokio::time::timeout(timeout, call).await {
        Ok(result) => result,
        Err(_) => Err(WasmError::new(
            WasmErrorKind::Timeout,
            format!("WASM execution exceeded {}ms", timeout.as_millis()),
        )),
    }
}

pub(super) fn engine() -> Result<&'static Engine, WasmError> {
    match ENGINE.get_or_init(|| {
        let mut config = Config::new();
        config.epoch_interruption(true).consume_fuel(true);
        Engine::new(&config).map_err(|error| error.to_string())
    }) {
        Ok(engine) => Ok(engine),
        Err(error) => Err(WasmError::new(
            WasmErrorKind::Internal,
            format!("cannot initialize Wasmtime: {error}"),
        )),
    }
}

fn ensure_epoch_ticker(engine: &Engine) -> Result<(), WasmError> {
    match EPOCH_TICKER.get_or_init(|| {
        let engine = engine.clone();
        std::thread::Builder::new()
            .name("starling-wasm-epoch".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }) {
        Ok(()) => Ok(()),
        Err(error) => Err(WasmError::new(
            WasmErrorKind::Internal,
            format!("cannot start WASM epoch ticker: {error}"),
        )),
    }
}

fn epoch_deadline_ticks(timeout: Duration) -> u64 {
    timeout
        .as_nanos()
        .div_ceil(EPOCH_TICK.as_nanos())
        .max(1)
        .try_into()
        .unwrap_or(u64::MAX)
}

struct StoreData {
    limiter: GuestLimiter,
}

struct GuestLimiter {
    max_memory: usize,
    memory_limit_hit: bool,
}

impl GuestLimiter {
    fn new(max_memory: usize) -> Self {
        Self {
            max_memory,
            memory_limit_hit: false,
        }
    }
}

impl ResourceLimiter for GuestLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_memory || maximum.is_some_and(|maximum| desired > maximum) {
            self.memory_limit_hit = true;
            return Ok(false);
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= MAX_WASM_TABLE_ELEMENTS && maximum.is_none_or(|maximum| desired <= maximum))
    }

    fn instances(&self) -> usize {
        1
    }

    fn tables(&self) -> usize {
        4
    }

    fn memories(&self) -> usize {
        1
    }
}

fn classify_runtime_error(
    store: &Store<StoreData>,
    action: &str,
    error: wasmtime::Error,
) -> WasmError {
    if store.data().limiter.memory_limit_hit {
        return WasmError::new(
            WasmErrorKind::MemoryLimit,
            format!("{action}: WASM linear memory limit exceeded"),
        );
    }
    WasmError::new(WasmErrorKind::Trap, format!("{action}: {error}"))
}

fn abi_error(action: &str, error: wasmtime::Error) -> WasmError {
    WasmError::new(
        WasmErrorKind::Abi,
        format!("WASM module has an incompatible {action} export: {error}"),
    )
}

fn validate_guest_range(
    store: &Store<StoreData>,
    memory: &Memory,
    pointer: i32,
    length: i32,
) -> Result<(), WasmError> {
    if pointer < 0 || length < 0 {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            "WASM module returned a negative buffer pointer or length",
        ));
    }
    let start = pointer as usize;
    let end = start
        .checked_add(length as usize)
        .ok_or_else(|| WasmError::new(WasmErrorKind::Abi, "WASM buffer range overflowed"))?;
    if end > memory.data_size(store) {
        return Err(WasmError::new(
            WasmErrorKind::Abi,
            "WASM module returned an out-of-bounds buffer",
        ));
    }
    Ok(())
}
