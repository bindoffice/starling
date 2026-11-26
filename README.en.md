# Starling

[简体中文](README.md) | [English](README.en.md)

Starling is an Nginx-compatible web server and Layer 4 reverse proxy written in
Rust. It reads Nginx-style configuration directly and preserves the familiar
`nginx` command line, PID file, signal, and configuration reload behavior.

The goal is not merely to reproduce an interface, but to build a maintainable,
testable server core on the Rust/Tokio asynchronous runtime that is compatible
with common Nginx configurations. This includes HTTP/1.1, HTTP/2, HTTP/3, TLS,
static files, upstream proxying, caching, compression, FastCGI, uWSGI, SCGI,
gRPC, and TCP/UDP stream proxying.

> The current version is `0.1.0` and remains under active development. Starling
> does not claim full Nginx module coverage. Before production use, validate
> your actual configuration with `-t` and run integration tests.

## Highlights

- **Nginx-style configuration**: quotes, variables, nested blocks, `include`,
  `map`, `geo`, `split_clients`, locations, rewrites, `if`, and multi-level
  scope inheritance.
- **Modern HTTP**: HTTP/1.0, HTTP/1.1, HTTP/2, HTTP/3/QUIC, TLS ALPN,
  keep-alive, chunked transfer encoding, trailers, PROXY protocol v1/v2, and
  `Expect: 100-continue`.
- **Static and content handling**: static files, Range requests, autoindex,
  gzip/gunzip, `try_files`, `error_page`, SSI, response substitution, WebDAV,
  and user IDs.
- **Reverse proxy and gateways**: HTTP/HTTPS upstreams, gRPC, FastCGI, uWSGI,
  SCGI, Memcached, load balancing, retries, connection reuse, and dynamic
  upstream variables.
- **Caching and streaming**: proxy/CGI disk caching, stale responses and
  background updates, request and response buffering, temporary files, Range
  requests, and backpressured streaming body forwarding.
- **Access control and security**: Basic Auth, subrequest authorization,
  allow/deny, rate limiting, hotlink protection, URL signing, client TLS
  certificates, CRL checks, and OCSP Stapling.
- **Stream proxying**: TCP, UDP, UNIX sockets, TLS termination, SNI routing,
  PROXY protocol, connection limits, and half-close control.
- **Automatic HTTPS**: enabled by default with automatic local development
  certificates; public domains are issued and renewed through ACME, with
  HTTP-01 and a generic DNS-01 hook. Wildcard domains can use DNS-01.

## Quick Start

### Build

A Rust toolchain with Rust 2024 Edition support is required.

```bash
cargo build --release
```

The executable is written to:

```text
target/release/starling
```

Run the tests:

```bash
cargo test --all-targets
```

### Minimal configuration

Create `conf/nginx.conf`:

```nginx
auto_https off;

events {}

http {
    server {
        listen 127.0.0.1:8080;
        server_name localhost;

        location / {
            return 200 "hello from Starling\n";
        }
    }
}
```

### Validate and start

```bash
./target/release/starling -t -p "$PWD" -c conf/nginx.conf
./target/release/starling -p "$PWD" -c conf/nginx.conf -g 'daemon off;'
```

From another terminal:

```bash
curl http://127.0.0.1:8080/
```

### Enable automatic HTTPS

When a server listens on the default `http_port=80`, the `auto_https` directive
can be omitted. Starling adds an HTTPS listener by default and uses HTTP-01 for
ordinary public domains:

```nginx
auto_https on
    email=admin@your-domain.com
    challenge=http-01;

events {}

http {
    server {
        listen 80;
        server_name your-domain.com www.your-domain.com;

        location / {
            return 200 "automatic HTTPS\n";
        }
    }
}
```

Use DNS-01 when a public wildcard certificate is required:

```nginx
auto_https on
    email=admin@your-domain.com
    challenge=dns-01
    dns_hook=/usr/local/libexec/starling-dns-hook
    dns_propagation_delay=30s;

events {}

http {
    server {
        listen 80;
        server_name your-domain.com *.your-domain.com;
    }
}
```

See [docs/auto-https.md](docs/auto-https.md) for the complete DNS hook
contract, configuration options, and troubleshooting guidance.

## Project Layout

```text
src/config/        Nginx configuration lexer, parser, include handling, and runtime model compiler
src/http/          HTTP request, response, and upstream protocols
src/server/        HTTP/TLS/stream runtime, routing, proxying, caching, and automatic HTTPS
src/logging.rs     Logging system and log file cache
src/runtime.rs     Nginx-style CLI, daemon, PID, and signal handling
tests/             Configuration compatibility, server behavior, and runtime migration tests
```

Production code is split into focused modules by configuration phase, protocol,
and request lifecycle. Entry files primarily orchestrate modules; complex
directive families and runtime capabilities should continue to live in their
corresponding submodules.

## Documentation

- [FEATURES.en.md](FEATURES.en.md): implemented features, build and run
  instructions, configuration examples, compatibility boundaries, and the full
  code layout.
- [docs/auto-https.md](docs/auto-https.md): automatic HTTPS, local CA, HTTP-01,
  DNS-01 hooks, certificate renewal, and troubleshooting.
- [docs/wasm.md](docs/wasm.md): WASM content handler configuration, resource
  limits, core module ABI v1, and a minimal example.

## CI and Releases

The repository provides the following automation:

- `.github/workflows/ci.yml`: runs `cargo fmt`, Clippy, the full test suite, and
  a release packaging check on pushes to `main`, pull requests, and manual
  dispatches.
- `.github/workflows/release.yml`: on a `v*` tag push or manual dispatch,
  builds Linux x86_64 and aarch64 binaries, creates `tar.gz` release archives
  and `SHA256SUMS`, and creates or updates a GitHub Release.
- `scripts/ci.sh`: runs the same formatting, lint, test, and release build
  checks locally.
- `scripts/package-release.sh`: creates a platform archive containing the
  binary, default configuration, static pages, log directory, and
  documentation.

The release tag must match the version in `Cargo.toml`:

```bash
git tag v0.1.0
git push origin v0.1.0
```

Build a package locally:

```bash
cargo build --release --locked
scripts/package-release.sh --version 0.1.0
```

## Compatibility Notes

Starling primarily targets the configuration and runtime behavior of Nginx
1.31.9. HTTP/3 has an initial `quinn` + `h3` implementation covering
`listen ... quic`, TLSv1.3 with ALPN `h3`, request/response mapping, streaming
request bodies, and response trailers. It does not yet support 0-RTT,
automatic Alt-Svc injection, hot certificate replacement for HTTP/3
connections during reload, or multi-worker UDP distribution. Other explicit
boundaries include dynamic modules, the mail module, the native multi-worker
master model, and some advanced Nginx module details.

When a configuration contains an unimplemented directive, Starling reports a
clear error during configuration validation whenever possible instead of
silently ignoring it and producing hard-to-diagnose runtime behavior. See
[FEATURES.en.md](FEATURES.en.md#compatibility-boundaries) for details.

## HTTP Performance Baseline

The following results come from loopback comparisons on the same host; they are
not cross-machine or public-network benchmarks. The test system used an AMD
Ryzen 7 9700X (16 logical CPUs), Linux `7.0.0-43-generic`, and 16 GiB of memory.
Except for protocol differences, both sides used `worker_processes 1`, disabled
access logging, and served the same set of static files. Nginx was built from
local source as `1.31.9` with `OpenSSL 3.5.5`, with both
`--with-http_v2_module` and
`--with-http_v3_module` enabled.

### HTTP/1.1 and HTTP/2 (h2c)

This group uses cleartext TCP to isolate HTTP stack overhead; HTTP/2 uses
prior-knowledge `h2c`. Small HTTP/1.1 responses use 128 connections and large
responses use 32 connections, with 3 rounds of 5 seconds each. Small HTTP/2
responses use 8 connections with 16 streams per connection and 1 million
requests per round; 8 MiB downloads use 8 connections with 4 streams per
connection and 1024 requests per round. HTTP/2 p50/p99 values come from
per-request `h2load` logs. HTTP/1.1 warms up for 1 second per group; HTTP/2
warmup rounds use 10% of the requests per round. Results are the median of 3
rounds.

| Scenario | Implementation | Request throughput (req/s) | Transfer throughput (GiB/s) | p50 (ms) | p99 (ms) | Failures |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| HTTP/1.1 small response | Starling | 195,061 | - | 0.657 | 0.675 | 0 |
| HTTP/1.1 small response | Nginx 1.31.9 | 272,925 | - | 0.469 | 0.484 | 0 |
| HTTP/1.1 8 MiB download | Starling | 4,127.64 | 32.26 | 7.800 | 15.950 | 0 |
| HTTP/1.1 8 MiB download | Nginx 1.31.9 | 4,165.32 | 32.57 | 7.700 | 13.330 | 0 |
| HTTP/2 small response | Starling | 136,059 | - | 0.902 | 1.145 | 0 |
| HTTP/2 small response | Nginx 1.31.9 | 223,423 | - | 0.568 | 1.110 | 0 |
| HTTP/2 8 MiB download | Starling | 1,448.15 | 10.54 | 22.300 | 45.378 | 0 |
| HTTP/2 8 MiB download | Nginx 1.31.9 | 1,050.55 | 8.220 | 28.444 | 36.162 | 0 |

For small HTTP/1.1 responses, Starling reaches 71.5% of Nginx throughput and
trails by about 1.40x. For small HTTP/2 responses it reaches 60.9% and trails
by about 1.64x. HTTP/1.1 8 MiB download throughput is essentially at parity
(99.1%), while Starling p99 is about 19.7% higher. After optimization, HTTP/2
8 MiB downloads reach 137.9% of Nginx and are about 7.13x the pre-optimization
rate of 203.11 req/s; p99 remains about 25.5% higher than Nginx, leaving room
for further tail-latency work. On Linux cleartext TCP with `sendfile on`, this
path sends DATA directly through the kernel `sendfile` from h2 and combines
frame headers with the body using `MSG_MORE` + `TCP_CORK`. TLS, UNIX sockets,
non-Linux platforms, or `sendfile off` automatically fall back to the memory
read path. H1/H2 and H3 use different transport stacks, so ratios from one
protocol should not be extrapolated to another.

Reproduction command:

```bash
scripts/bench-compare.sh \
  --nginx-bin /path/to/nginx \
  --workers 1 \
  --rounds 3 \
  --duration 5s \
  --warmup 1s \
  --connections 128 \
  --large-connections 32 \
  --h2-clients 8 \
  --h2-streams 16 \
  --h2-requests 1000000 \
  --h2-large-streams 4 \
  --h2-large-requests 1024 \
  --skip-upload
```

### HTTP/3/QUIC

Starling uses `quinn` + `h3`; Nginx enables `--with-http_v3_module`. Both use
TLSv1.3 and ALPN `h3`. Small responses use a 5-byte static file with 8 QUIC
connections and 16 concurrent requests per connection. Large responses use an
8 MiB static file with 8 QUIC connections and 4 concurrent requests per
connection. Each group warms up for 1 second, then runs 3 independent rounds of
5 seconds and takes the median. Client connections are re-established for each
round so connection-level keep-alive limits do not distort throughput.

| Scenario | Implementation | Request throughput (req/s) | Transfer throughput (GiB/s) | p50 (ms) | p99 (ms) | Failures |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| HTTP/3 small response | Starling `quinn` + `h3` | 122,647 | - | 1.054 | 1.231 | 0 |
| HTTP/3 small response | Nginx 1.31.9 | 166,581 | - | 0.622 | 2.105 | 0 |
| HTTP/3 8 MiB download | Starling `quinn` + `h3` | 190.82 | 1.491 | 167.483 | 171.169 | 0 |
| HTTP/3 8 MiB download | Nginx 1.31.9 | 108.16 | 0.845 | 244.966 | 790.959 | 0 |

These results show different path characteristics. For small responses, Nginx
single-core request throughput is about 1.36x Starling, but Starling has lower
p99 latency. For 8 MiB downloads, Starling throughput is about 1.76x Nginx and
p99 is also substantially lower. The results do not generalize to public
networks, multiple workers, or lossy links. The current Starling QUIC endpoint
is only attached to the first worker shard, so this table intentionally fixes
the worker count at one; multi-worker UDP distribution, 0-RTT, automatic
Alt-Svc injection, and long-lived connection reuse still need dedicated
testing.

The reproducible client is in [examples/h3load.rs](examples/h3load.rs). Example
command:

```bash
cargo build --release --example h3load
target/release/examples/h3load \
  --addr 127.0.0.1:18443 \
  --sni localhost \
  --ca tests/fixtures/ca.crt \
  --path /small \
  --connections 8 \
  --streams 16 \
  --warmup 1 \
  --duration 5 \
  --rounds 3
```

## License

Starling is licensed under the Apache License, Version 2.0. See
[LICENSE](LICENSE) for the full license text.
