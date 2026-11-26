use super::support::*;
use crate::server::run::reload;

const HELLO_MODULE: &str = include_str!("../../../examples/wasm/hello.wat");

fn wasm_config(module: &str, limits: &str) -> String {
    format!(
        r#"
            worker_processes 1;
            error_log logs/error.log error;
            events {{ worker_connections 64; }}
            http {{
                wasm_engine on;
                server {{
                    listen 127.0.0.1:18098;
                    location / {{
                        wasm_module {module};
                        {limits}
                    }}
                }}
            }}
        "#
    )
}

fn wasm_empty_status_module(status: u16) -> String {
    let mut response = Vec::new();
    response.extend_from_slice(b"STWLRS01");
    response.extend_from_slice(&1_u16.to_le_bytes());
    response.extend_from_slice(&status.to_le_bytes());
    response.extend_from_slice(&0_u16.to_le_bytes());
    response.extend_from_slice(&0_u32.to_le_bytes());
    let data = response
        .iter()
        .map(|byte| format!(r"\{byte:02x}"))
        .collect::<String>();
    let packed = ((4096_u64) << 32) | response.len() as u64;
    format!(
        r#"
            (module
              (memory (export "memory") 1)
              (func (export "starling_abi_version") (result i32) i32.const 1)
              (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
              (func (export "starling_handle") (param i32 i32) (result i64)
                i64.const {packed})
              (data (i32.const 4096) "{data}"))
        "#
    )
}

#[tokio::test]
async fn wasm_content_handler_returns_status_headers_and_body() {
    let source = wasm_config("wasm/hello.wat", "");
    let (prefix, config) = test_config(&source, &[("wasm/hello.wat", HELLO_MODULE)]);
    let response = request_body(
        &config,
        "GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
    assert!(
        response.contains("Content-Type: text/plain\r\n"),
        "{response}"
    );
    assert!(response.contains("X-Wasm: hello\r\n"), "{response}");
    assert!(response.ends_with("hello from wasm\n"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn wasm_bodyless_statuses_use_correct_content_length_framing() {
    for (status, reason) in [
        (204, "No Content"),
        (205, "Reset Content"),
        (304, "Not Modified"),
    ] {
        let module_path = format!("wasm/{status}.wat");
        let module = wasm_empty_status_module(status);
        let source = wasm_config(&module_path, "");
        let (prefix, config) = test_config(&source, &[(&module_path, &module)]);
        let response = request_body(
            &config,
            "GET /empty HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status} {reason}\r\n")),
            "{response}"
        );
        if status == 205 {
            assert!(response.contains("Content-Length: 0\r\n"), "{response}");
        } else {
            assert!(!response.contains("Content-Length:"), "{response}");
        }
        let (_, body) = split_http_response(response.as_bytes());
        assert!(body.is_empty(), "{response}");
        let _ = std::fs::remove_dir_all(prefix);
    }
}

#[tokio::test]
async fn wasm_request_body_limit_returns_413() {
    let source = wasm_config("wasm/hello.wat", "wasm_request_body_max 4;");
    let (prefix, config) = test_config(&source, &[("wasm/hello.wat", HELLO_MODULE)]);
    let response = request_body(
        &config,
        "POST /hello HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    let chunked = request_body(
        &config,
        "POST /hello HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    )
    .await;
    assert!(chunked.starts_with("HTTP/1.1 413"), "{chunked}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn wasm_timeout_returns_500_without_blocking_the_worker() {
    let timeout_module = r#"
        (module
          (memory (export "memory") 1)
          (func (export "starling_abi_version") (result i32) i32.const 1)
          (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
          (func (export "starling_handle") (param i32 i32) (result i64)
            (loop $spin br $spin)
            i64.const 0))
    "#;
    let source = wasm_config("wasm/timeout.wat", "wasm_timeout 20ms;");
    let (prefix, config) = test_config(&source, &[("wasm/timeout.wat", timeout_module)]);
    let response = request_body(
        &config,
        "GET /timeout HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn wasm_trap_and_memory_limit_return_predictable_500() {
    let trap_module = r#"
        (module
          (memory (export "memory") 1)
          (func (export "starling_abi_version") (result i32) i32.const 1)
          (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
          (func (export "starling_handle") (param i32 i32) (result i64) unreachable))
    "#;
    let source = wasm_config("wasm/trap.wat", "");
    let (prefix, config) = test_config(&source, &[("wasm/trap.wat", trap_module)]);
    let response = request_body(
        &config,
        "GET /trap HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);

    let memory_module = r#"
        (module
          (memory (export "memory") 2)
          (func (export "starling_abi_version") (result i32) i32.const 1)
          (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
          (func (export "starling_handle") (param i32 i32) (result i64)
            (drop (memory.grow (i32.const 1)))
            (drop (i32.load (i32.const 131072)))
            i64.const 0))
    "#;
    let source = wasm_config("wasm/memory.wat", "wasm_memory_max 128k;");
    let (prefix, config) = test_config(&source, &[("wasm/memory.wat", memory_module)]);
    let response = request_body(
        &config,
        "GET /memory HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[tokio::test]
async fn wasm_fuel_exhaustion_returns_predictable_500() {
    let fuel_module = r#"
        (module
          (memory (export "memory") 1)
          (func (export "starling_abi_version") (result i32) i32.const 1)
          (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
          (func (export "starling_handle") (param i32 i32) (result i64)
            (loop $spin br $spin)
            i64.const 0))
    "#;
    let source = wasm_config("wasm/fuel.wat", "wasm_fuel 1;");
    let (prefix, config) = test_config(&source, &[("wasm/fuel.wat", fuel_module)]);
    let response = request_body(
        &config,
        "GET /fuel HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    let _ = std::fs::remove_dir_all(prefix);
}

#[test]
fn wasm_configuration_requires_engine_and_rejects_conflicts() {
    let without_engine = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                server {
                    location / { wasm_module wasm/hello.wat; }
                }
            }
        "#,
        &[("wasm/hello.wat", HELLO_MODULE)],
    );
    assert!(
        without_engine.contains("requires \"wasm_engine on\""),
        "{without_engine}"
    );

    let proxy_conflict = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / {
                        proxy_pass http://127.0.0.1:18099;
                        wasm_module wasm/hello.wat;
                    }
                }
            }
        "#,
        &[("wasm/hello.wat", HELLO_MODULE)],
    );
    assert!(
        proxy_conflict.contains("conflicts with \"proxy_pass\""),
        "{proxy_conflict}"
    );

    let process_isolation = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / {
                        wasm_module wasm/hello.wat;
                        wasm_isolation process;
                    }
                }
            }
        "#,
        &[("wasm/hello.wat", HELLO_MODULE)],
    );
    assert!(
        process_isolation.contains("not implemented yet"),
        "{process_isolation}"
    );

    let invalid_abi = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / { wasm_module wasm/invalid.wat; }
                }
            }
        "#,
        &[("wasm/invalid.wat", "(module (func))")],
    );
    assert!(
        invalid_abi.contains("does not export linear memory"),
        "{invalid_abi}"
    );

    let unsupported_imports = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / { wasm_module wasm/imports.wat; }
                }
            }
        "#,
        &[(
            "wasm/imports.wat",
            r#"(module (import "env" "host" (func)) (memory (export "memory") 1))"#,
        )],
    );
    assert!(
        unsupported_imports.contains("host imports are not supported"),
        "{unsupported_imports}"
    );

    let invalid_version = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / { wasm_module wasm/version.wat; }
                }
            }
        "#,
        &[(
            "wasm/version.wat",
            r#"
                (module
                  (memory (export "memory") 1)
                  (func (export "starling_abi_version") (result i32) i32.const 2)
                  (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
                  (func (export "starling_handle") (param i32 i32) (result i64) i64.const 0))
            "#,
        )],
    );
    assert!(
        invalid_version.contains("unsupported WASM ABI version 2"),
        "{invalid_version}"
    );

    let scope_memory_limit = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location /large/ {
                        wasm_module wasm/two-pages.wat;
                        wasm_memory_max 1m;
                    }
                    location /small/ {
                        wasm_module wasm/two-pages.wat;
                        wasm_memory_max 64k;
                    }
                }
            }
        "#,
        &[(
            "wasm/two-pages.wat",
            r#"
                (module
                  (memory (export "memory") 2)
                  (func (export "starling_abi_version") (result i32) i32.const 1)
                  (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
                  (func (export "starling_handle") (param i32 i32) (result i64) i64.const 0))
            "#,
        )],
    );
    assert!(
        scope_memory_limit.contains("WASM linear memory limit exceeded"),
        "{scope_memory_limit}"
    );

    let startup_timeout = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / {
                        wasm_module wasm/startup-timeout.wat;
                        wasm_timeout 20ms;
                    }
                }
            }
        "#,
        &[(
            "wasm/startup-timeout.wat",
            r#"
                (module
                  (memory (export "memory") 1)
                  (func $spin (loop $again br $again))
                  (start $spin)
                  (func (export "starling_abi_version") (result i32) i32.const 1)
                  (func (export "starling_alloc") (param i32) (result i32) i32.const 1024)
                  (func (export "starling_handle") (param i32 i32) (result i64) i64.const 0))
            "#,
        )],
    );
    assert!(
        startup_timeout.contains("failed ABI validation"),
        "{startup_timeout}"
    );

    let conflicting_status_handler = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / {
                        wasm_module wasm/hello.wat;
                        stub_status;
                    }
                }
            }
        "#,
        &[("wasm/hello.wat", HELLO_MODULE)],
    );
    assert!(
        conflicting_status_handler.contains("conflicts with \"stub_status\""),
        "{conflicting_status_handler}"
    );

    let oversized_body_limit = test_config_error_with_files(
        r#"
            worker_processes 1;
            events { worker_connections 64; }
            http {
                wasm_engine on;
                server {
                    location / {
                        wasm_module wasm/hello.wat;
                        wasm_response_body_max 3g;
                    }
                }
            }
        "#,
        &[("wasm/hello.wat", HELLO_MODULE)],
    );
    assert!(
        oversized_body_limit.contains("between 1 byte and 2 GiB"),
        "{oversized_body_limit}"
    );
}

#[tokio::test]
async fn wasm_reload_swaps_the_compiled_module_snapshot() {
    let source = wasm_config("wasm/hello.wat", "");
    let (prefix, config) = test_config(&source, &[("wasm/hello.wat", HELLO_MODULE)]);
    let state = test_state(&config);
    let request = "GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    let first = request_body_with_state(&config, state.clone(), request).await;
    assert!(first.contains("X-Wasm: hello\r\n"), "{first}");

    let reloaded_module = HELLO_MODULE.replacen(r"\68\65\6c\6c\6f", r"\77\6f\72\6c\64", 1)
        + "\n;; force a module-cache miss\n";
    std::fs::write(prefix.join("wasm/hello.wat"), reloaded_module).unwrap();
    reload(&state, &config.config_path, &prefix).await;
    let reloaded = state.current_config();
    let second = request_body_with_state(&reloaded, state, request).await;
    assert!(second.contains("X-Wasm: world\r\n"), "{second}");
    assert!(second.ends_with("hello from wasm\n"), "{second}");
    let _ = std::fs::remove_dir_all(prefix);
}
