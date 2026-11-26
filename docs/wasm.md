# WASM content handler

Starling can run WebAssembly modules as a location-scoped content handler. The
host remains a native Starling process; WASM modules cannot access Starling
files, sockets, environment variables, or TLS private keys through WASI.

The current implementation supports:

- A buffered request/response content handler on core WASM modules.
- In-process Wasmtime execution with one store and one module instance per call.
- Process-wide compiled-module caching keyed by canonical path, size, mtime,
  and source digest.
- Epoch-based wall-clock interruption plus an optional fuel limit.
- Linear-memory, request-body, response-body, header, and instance-concurrency
  limits.
- Atomic configuration reloads. In-flight requests keep the old module and
  configuration snapshot until they finish.

Request filters, response filters, streaming bodies, component-model ABI, and
the process-isolated backend are not implemented yet. Configuration directives
for those unimplemented modes fail during `starling -t` instead of silently
falling back.

In-process execution is not a hard multi-tenant security boundary: it relies on
Wasmtime itself isolating guest code. Use only reviewed modules in this mode
until the process-isolated backend is available.

## Configuration

WASM must be enabled explicitly in `http` context:

```nginx
http {
    wasm_engine on;
    wasm_module_cache 64m;

    server {
        listen 8080;

        location /greeting {
            wasm_module /srv/starling/wasm/greeting.wasm;
            wasm_phase content;
            wasm_timeout 100ms;
            wasm_memory_max 64m;
            wasm_fuel 10000000;          # optional; omit or set 0 for no fuel cap
            wasm_request_body_max 1m;
            wasm_response_body_max 8m;
            wasm_max_instances 32;
            wasm_isolation in-process;
        }
    }
}
```

`wasm_module`, `wasm_phase`, and the resource directives are inherited by
nested locations. They can be overridden in a server or location scope.
`wasm_engine` and `wasm_module_cache` are valid only in `http` scope.

Default values are:

| Directive | Default |
| --- | --- |
| `wasm_engine` | `off` |
| `wasm_module_cache` | `64m` |
| `wasm_phase` | `content` |
| `wasm_timeout` | `100ms` |
| `wasm_memory_max` | `64m` |
| `wasm_fuel` | unlimited |
| `wasm_request_body_max` | `1m` |
| `wasm_response_body_max` | `8m` |
| `wasm_max_instances` | `32` |
| `wasm_isolation` | `in-process` |

`wasm_module` conflicts with `proxy_pass`, `grpc_pass`, FastCGI, uWSGI, SCGI,
Memcached, tunnel, `stub_status`, WebDAV, `empty_gif`, and `flv` content
handlers. A `.wasm` file or text-format `.wat` module is compiled and its ABI
exports, memory shape, absence of host imports, and `starling_abi_version`
value are validated while loading the configuration. Syntax errors and
incompatible modules reject `starling -t` or a reload instead of failing only
at request time. The runtime repeats the version check before each call.

## Runtime limits

- `wasm_timeout` interrupts CPU-bound guest code with Wasmtime epochs. It also
  bounds the wait for an available instance slot.
- `wasm_memory_max` rejects linear-memory growth beyond the configured size.
- `wasm_fuel` is an optional deterministic instruction budget. It is not a
  replacement for the wall-clock timeout.
- `wasm_request_body_max` larger requests fail with `413`.
  `client_max_body_size` remains an independent upper bound, and the stricter
  value wins. Values above the 32-bit ABI limit (2 GiB) are rejected during
  configuration checks.
- `wasm_response_body_max` larger guest responses become a host-side `500`.
  Values above the 32-bit ABI limit (2 GiB) are rejected during configuration
  checks.
- `wasm_max_instances` bounds concurrent instances for the configured module.
- A trap, invalid ABI response, timeout, memory-limit failure, or module
  instantiation failure returns a predictable `500` and never terminates the
  worker process.

## Core module ABI v1

Modules must export:

```text
memory
starling_abi_version() -> i32          # returns 1
starling_alloc(size: i32) -> i32       # returns a guest pointer
starling_handle(ptr: i32, len: i32) -> i64
```

The host writes one request buffer through `starling_alloc`, then calls
`starling_handle`. The result packs a response pointer and length:

```text
pointer = (result as u64) >> 32
length  = result as u32
```

All integer fields are little-endian. Strings are UTF-8 and use the length
width shown below.

### Request buffer

```text
8 bytes   magic "STWLRQ01"
u16       ABI version (1)
string16  method
string32  raw request target
string32  normalized URI/path
string32  query string without '?'
string16  HTTP version
string16  scheme
string32  host
string32  remote address
string32  local address
u16       header count
  string16 header name
  string32 header value
u32       body length
bytes     body
```

### Response buffer

```text
8 bytes   magic "STWLRS01"
u16       ABI version (1)
u16       HTTP status (200..599)
u16       header count (maximum 256)
  string16 header name
  string32 header value
u32       body length
bytes     body
```

The host owns response framing and therefore rejects `Connection`,
`Content-Length`, `Keep-Alive`, `Proxy-Connection`, `TE`, `Trailer`,
`Transfer-Encoding`, and `Upgrade` headers from the guest. Header names must be
tokens, values cannot contain control characters other than horizontal tab,
total header data is limited to 64 KiB, and statuses `204`, `205`, and `304`
cannot carry a body. The host emits `Content-Length: 0` for `205` responses and
omits `Content-Length` for `204` and `304`.

## Minimal example

[`examples/wasm/hello.wat`](../examples/wasm/hello.wat) is an executable core
module that returns `200`, `Content-Type: text/plain`, `X-Wasm: hello`, and
`hello from wasm\n`.

Use it with:

```nginx
http {
    wasm_engine on;
    server {
        listen 8080;
        location / {
            wasm_module examples/wasm/hello.wat;
        }
    }
}
```

The module ignores the request body and uses a static response buffer. A
production module should normally implement a bump allocator with a capacity
check, parse only the fields it needs, and construct the response in guest
memory before returning its packed pointer/length.
