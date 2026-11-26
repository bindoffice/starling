use super::*;

#[derive(Default)]
pub(super) struct WasmSeen {
    engine: bool,
    module_cache: bool,
    module: bool,
    phase: bool,
    timeout: bool,
    memory_max: bool,
    fuel: bool,
    request_body_max: bool,
    response_body_max: bool,
    max_instances: bool,
    isolation: bool,
}

pub(super) fn try_apply_wasm_http_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    seen: &mut WasmSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "wasm_engine" => {
            reject_duplicate(directive, &mut seen.engine)?;
            context.wasm.engine = parse_flag(directive, "wasm_engine")?;
        }
        "wasm_module_cache" => {
            reject_duplicate(directive, &mut seen.module_cache)?;
            validate_no_block(directive)?;
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            if size == 0 {
                return Err(invalid_value(directive, "must be greater than zero"));
            }
            context.wasm.module_cache_size = size;
        }
        "wasm_module"
        | "wasm_phase"
        | "wasm_timeout"
        | "wasm_memory_max"
        | "wasm_fuel"
        | "wasm_request_body_max"
        | "wasm_response_body_max"
        | "wasm_max_instances"
        | "wasm_isolation" => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "\"{}\" directive is only allowed in server or location context",
                    directive.name
                ),
            ));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(super) fn try_apply_wasm_scope_directive(
    directive: &Directive,
    context: &mut EffectiveConfig,
    prefix: &Path,
    seen: &mut WasmSeen,
) -> ConfigResult<bool> {
    match directive.name.as_str() {
        "wasm_module" => {
            reject_duplicate(directive, &mut seen.module)?;
            validate_no_block(directive)?;
            let path = resolve_path(prefix, one_arg(directive)?);
            match crate::wasm::load_module(&path, context.wasm.module_cache_size) {
                Ok(module) => context.wasm.install_plugin(path, module),
                Err(error) => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        error.to_string(),
                    ));
                }
            }
        }
        "wasm_phase" => {
            reject_duplicate(directive, &mut seen.phase)?;
            validate_no_block(directive)?;
            context.wasm.phase = match one_arg(directive)? {
                "content" => WasmPhase::Content,
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!(
                            "unsupported value \"{value}\" in \"wasm_phase\" directive; only \"content\" is implemented"
                        ),
                    ));
                }
            };
        }
        "wasm_timeout" => {
            reject_duplicate(directive, &mut seen.timeout)?;
            validate_no_block(directive)?;
            let timeout = parse_duration(one_arg(directive)?, &directive.location)?;
            if timeout.is_zero() {
                return Err(invalid_value(directive, "must be greater than zero"));
            }
            context.wasm.timeout = timeout;
        }
        "wasm_memory_max" => {
            reject_duplicate(directive, &mut seen.memory_max)?;
            validate_no_block(directive)?;
            let memory = parse_size(one_arg(directive)?, &directive.location)?;
            if memory == 0 || memory > 4 * 1024 * 1024 * 1024 {
                return Err(invalid_value(directive, "must be between 1 byte and 4 GiB"));
            }
            context.wasm.memory_max = memory;
        }
        "wasm_fuel" => {
            reject_duplicate(directive, &mut seen.fuel)?;
            validate_no_block(directive)?;
            let value = one_arg(directive)?.parse::<u64>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"wasm_fuel\" directive",
                )
            })?;
            context.wasm.fuel = (value != 0).then_some(value);
        }
        "wasm_request_body_max" => {
            reject_duplicate(directive, &mut seen.request_body_max)?;
            validate_no_block(directive)?;
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            if size == 0 || size > i32::MAX as u64 {
                return Err(invalid_value(directive, "must be between 1 byte and 2 GiB"));
            }
            context.wasm.request_body_max = size;
        }
        "wasm_response_body_max" => {
            reject_duplicate(directive, &mut seen.response_body_max)?;
            validate_no_block(directive)?;
            let size = parse_size(one_arg(directive)?, &directive.location)?;
            if size == 0 || size > i32::MAX as u64 {
                return Err(invalid_value(directive, "must be between 1 byte and 2 GiB"));
            }
            context.wasm.response_body_max = size;
        }
        "wasm_max_instances" => {
            reject_duplicate(directive, &mut seen.max_instances)?;
            validate_no_block(directive)?;
            let value = one_arg(directive)?.parse::<usize>().map_err(|_| {
                ConfigError::at(
                    directive.location.clone(),
                    "invalid value in \"wasm_max_instances\" directive",
                )
            })?;
            if !(1..=65_536).contains(&value) {
                return Err(invalid_value(directive, "must be between 1 and 65536"));
            }
            context.wasm.set_max_instances(value);
        }
        "wasm_isolation" => {
            reject_duplicate(directive, &mut seen.isolation)?;
            validate_no_block(directive)?;
            context.wasm.isolation = match one_arg(directive)? {
                "in-process" => WasmIsolation::InProcess,
                "process" => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"wasm_isolation process\" is not implemented yet",
                    ));
                }
                value => {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        format!("invalid value \"{value}\" in \"wasm_isolation\" directive"),
                    ));
                }
            };
        }
        "wasm_engine" | "wasm_module_cache" => {
            return Err(ConfigError::at(
                directive.location.clone(),
                format!(
                    "\"{}\" directive is only allowed in http context",
                    directive.name
                ),
            ));
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub(in crate::config::compile) fn validate_wasm_config(
    context: &EffectiveConfig,
    location: &SourceLocation,
) -> ConfigResult<()> {
    let Some(plugin) = &context.wasm.plugin else {
        return Ok(());
    };
    if !context.wasm.engine {
        return Err(ConfigError::at(
            location.clone(),
            "\"wasm_module\" requires \"wasm_engine on\" in http context",
        ));
    }
    if context.wasm.phase != WasmPhase::Content {
        return Err(ConfigError::at(
            location.clone(),
            "only \"wasm_phase content\" is implemented",
        ));
    }
    if context.wasm.isolation != WasmIsolation::InProcess {
        return Err(ConfigError::at(
            location.clone(),
            "only \"wasm_isolation in-process\" is implemented",
        ));
    }
    let conflict = if context.proxy_pass.is_some() {
        Some("proxy_pass")
    } else if context.grpc.pass.is_some() {
        Some("grpc_pass")
    } else if context.fastcgi_pass.is_some() {
        Some("fastcgi_pass")
    } else if context.uwsgi_pass.is_some() {
        Some("uwsgi_pass")
    } else if context.scgi_pass.is_some() {
        Some("scgi_pass")
    } else if context.memcached_pass.is_some() {
        Some("memcached_pass")
    } else if context.tunnel.pass.is_some() {
        Some("tunnel_pass")
    } else if context.stub_status {
        Some("stub_status")
    } else if !context.dav.methods.is_empty() {
        Some("dav_methods")
    } else if context.empty_gif {
        Some("empty_gif")
    } else if context.flv {
        Some("flv")
    } else {
        None
    };
    if let Some(conflict) = conflict {
        return Err(ConfigError::at(
            location.clone(),
            format!(
                "\"wasm_module\" for {} conflicts with \"{conflict}\"",
                plugin.path.display()
            ),
        ));
    }
    crate::wasm::validate_abi_version(
        plugin.module.as_ref(),
        context.wasm.timeout,
        context.wasm.memory_max,
    )
    .map_err(|error| {
        ConfigError::at(
            location.clone(),
            format!(
                "WASM module {} failed ABI validation: {error}",
                plugin.path.display()
            ),
        )
    })?;
    Ok(())
}

fn reject_duplicate(directive: &Directive, seen: &mut bool) -> ConfigResult<()> {
    if *seen {
        return Err(ConfigError::at(
            directive.location.clone(),
            format!("\"{}\" directive is duplicate", directive.name),
        ));
    }
    *seen = true;
    Ok(())
}

fn invalid_value(directive: &Directive, message: &str) -> ConfigError {
    ConfigError::at(
        directive.location.clone(),
        format!(
            "invalid value in \"{}\" directive: {message}",
            directive.name
        ),
    )
}
