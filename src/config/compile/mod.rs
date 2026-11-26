use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::config::{Regex, RegexBuilder};

use crate::error::{ConfigError, ConfigResult, SourceLocation};

use super::ast::{ConfigDocument, Directive};

mod access_log;
mod addition;
mod auto_https;
mod browser;
mod common;
mod compat;
mod core;
mod directives;
mod expires;
mod helpers;
mod http;
mod memcached;
mod mirror;
mod parsing;
mod referer;
mod secure_link;
mod server;
mod settings;
mod ssi;
mod stream;
mod stream_pass;
mod stream_server;
mod stream_server_options;
mod stream_tls_options;
mod syslog;
mod temp_path;
mod upstream;
mod uri;
mod userid;
mod variables;

use access_log::*;
use addition::*;
use auto_https::*;
use browser::*;
use common::*;
use compat::collect_compat_warnings;
use core::*;
use directives::*;
use expires::*;
use helpers::*;
use http::*;
use memcached::*;
use mirror::*;
use parsing::*;
use referer::*;
use secure_link::*;
use server::*;
use settings::*;
use ssi::*;
use stream::*;
use syslog::*;
use temp_path::*;
use upstream::*;
use variables::*;

mod model;
pub(crate) use helpers::parse_size_value;
pub use model::*;
use uri::normalize_location_path;
pub use uri::{match_location, normalize_uri, normalize_uri_with_merge, split_uri};
use userid::*;

pub fn compile(
    document: ConfigDocument,
    prefix: PathBuf,
    config_path: PathBuf,
) -> ConfigResult<RuntimeConfig> {
    let warnings = collect_compat_warnings(&document.directives);
    let mut core = CoreConfig {
        daemon: true,
        master_process: true,
        worker_processes: WorkerProcesses::Count(1),
        worker_shutdown_timeout: None,
        pid: prefix.join("logs/nginx.pid"),
        error_logs: Vec::new(),
        working_directory: None,
        worker_connections: 512,
    };
    let mut http: Option<HttpConfig> = None;
    let mut stream: Option<StreamConfig> = None;
    let mut auto_https = AutoHttpsConfig::new(&prefix);
    let mut seen_events = false;

    for directive in &document.directives {
        match directive.name.as_str() {
            "daemon" => core.daemon = parse_flag(directive, "daemon")?,
            "master_process" => core.master_process = parse_flag(directive, "master_process")?,
            "worker_processes" => {
                let value = one_arg(directive)?;
                core.worker_processes = if value == "auto" {
                    WorkerProcesses::Auto
                } else {
                    WorkerProcesses::Count(parse_usize(
                        value,
                        &directive.location,
                        "worker_processes",
                    )?)
                };
            }
            "pid" => core.pid = resolve_path(&prefix, one_arg(directive)?),
            "error_log" => core.error_logs.push(parse_log_target(&prefix, directive)?),
            "working_directory" => {
                core.working_directory = Some(resolve_path(&prefix, one_arg(directive)?));
            }
            "worker_shutdown_timeout" => {
                validate_no_block(directive)?;
                if core.worker_shutdown_timeout.is_some() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"worker_shutdown_timeout\" directive is duplicate",
                    ));
                }
                core.worker_shutdown_timeout =
                    Some(parse_duration(one_arg(directive)?, &directive.location)?);
            }
            _ if apply_auto_https_directive(directive, &mut auto_https, &prefix)? => {}
            "events" => {
                require_block(directive)?;
                if seen_events {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"events\" directive is duplicate",
                    ));
                }
                seen_events = true;
                compile_events(directive, &mut core)?;
            }
            "http" => {
                require_block(directive)?;
                if http.is_some() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"http\" directive is duplicate",
                    ));
                }
                http = Some(compile_http(directive, &prefix)?);
            }
            "stream" => {
                require_block(directive)?;
                if stream.is_some() {
                    return Err(ConfigError::at(
                        directive.location.clone(),
                        "\"stream\" directive is duplicate",
                    ));
                }
                stream = Some(compile_stream(directive, &prefix)?);
            }
            "user"
            | "worker_priority"
            | "worker_cpu_affinity"
            | "worker_rlimit_nofile"
            | "worker_rlimit_core"
            | "timer_resolution"
            | "debug_points"
            | "env"
            | "lock_file" => {
                validate_no_block(directive)?;
            }
            "load_module" => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    "load_module is not supported by the Rust runtime",
                ));
            }
            "mail" => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("\"{}\" is not implemented yet", directive.name),
                ));
            }
            other => {
                return Err(ConfigError::at(
                    directive.location.clone(),
                    format!("unknown directive \"{other}\""),
                ));
            }
        }
    }

    if !seen_events {
        return Err(ConfigError::new("no \"events\" section in configuration"));
    }
    if core.error_logs.is_empty() {
        core.error_logs.push(LogTarget {
            destination: LogDestination::File(prefix.join("logs/error.log")),
            level: LogLevel::Error,
        });
    }
    if auto_https.enabled && auto_https.http_port == auto_https.https_port {
        return Err(ConfigError::new(
            "auto_https http_port and https_port must be different",
        ));
    }
    if auto_https.enabled
        && auto_https.challenge == AutoHttpsChallenge::Dns01
        && auto_https.dns_hook.is_none()
    {
        return Err(ConfigError::new(
            "auto_https challenge=dns-01 requires a dns_hook",
        ));
    }
    validate_access_log_buffers(http.as_ref(), stream.as_ref())?;

    Ok(RuntimeConfig {
        prefix,
        config_path,
        files: document.files,
        warnings,
        core,
        auto_https,
        http,
        stream,
    })
}

#[cfg(test)]
mod tests;
