# Starling Features, Build, and Compatibility

[简体中文](FEATURES.md) | [English](FEATURES.en.md)

Starling is an Nginx-compatible web server and Layer 4 reverse proxy implemented
in Rust. It reads Nginx-style configuration directly and provides a familiar
`nginx` command line, PID/signal handling, static file serving, HTTP proxying,
TLS, and stream proxying.

This is an extensible compatibility implementation and does not claim to cover
every Nginx module. The project uses the official source and default
configuration in `nginx-release-1.31.9/` as its behavioral reference.

## Source Organization

Production code is split into focused submodules, while entry files retain only
module orchestration and stable public interfaces:

- `config/parser/`: `lexer`, `syntax`, and `loader` handle lexical analysis, the
  syntax tree, and include/glob loading.
- `config/compile/model/upstream/`: `types`, `server`, and `balance` handle
  upstream configuration, health state/concurrency counters, and scheduling
  algorithms.
- `config/compile/model/directives/`: configuration directive models split by
  access, limits, proxy, rewrite, headers, variables, and expires, re-exported
  through the module entry point.
- `config/compile/http/`: `http` main-context compilation is split into state,
  directive dispatch, final assembly, and recursive validation; `http.rs`
  retains only the compilation entry point and module orchestration.
- `config/compile/access_log.rs` and `config/compile/model/access_log.rs`:
  access-log directive parsing, scope inheritance models, and data structures
  for multiple log targets.
- `config/compile/directives/open_log_file_cache.rs`: log file cache parameter
  validation, `off` semantics, and per-scope instance inheritance.
- `config/compile/compat/`: centralized handling for directives kept only for
  configuration compatibility, plus TLS directives that rustls cannot safely
  implement equivalently and that must be rejected explicitly.
- `config/compile/syslog.rs`, `config/compile/model/syslog.rs`, and
  `logging/syslog.rs`: syslog target parsing, RFC 3164 message formatting, and
  UDP/UNIX datagram delivery.
- `logging/access.rs`: static/dynamic log paths, shared buffering, periodic
  flushing, and gzip-compressed writes.
- `logging/file_cache.rs`: `open_log_file_cache` policy-instance isolation, LRU
  capacity control, `inactive`/`min_uses`/`valid` lifetimes, inode
  revalidation, and cache invalidation on log rotation.
- `logging.rs`: multi-target error-log writes, independent level filtering,
  file reopening, and reload reconfiguration.
- `server/stream/error_log.rs`: runtime logger caching for stream main/server
  error logs, target selection by default/selected virtual server, and signal
  reopen and reload rebuild handling.
- `config/compile/stream_pass.rs` and `server/stream/pass.rs`: stream `pass`
  target resolution, local listener matching, pass-through counters, and
  runtime redistribution, avoiding further growth of `stream/tcp.rs`.
- `config/compile/memcached.rs`, `config/compile/model/memcached.rs`, and
  `server/upstream/memcached/`: Memcached directive parsing, response models,
  protocol encoding/decoding, key escaping, timeouts, named-upstream keepalive
  pools, and next_upstream retries split into focused files.
- `server/request/connection.rs`: HTTP/1 connection reads, keep-alive loops,
  response writes, and lingering close; `request/mod.rs` retains only submodule
  orchestration and exports.
- `server/socket_control.rs`: stores shared TCP socket handles and applies
  `TCP_NODELAY` before TLS handshakes and after final location selection,
  avoiding low-level socket operations scattered across HTTP/1, TLS, and
  HTTP/2 connection code.
- `config/compile/model/listen.rs` and `server/listener_socket.rs`: listen
  address/socket parameter models separated from TCP/UDP binding;
  `listener_socket/platform.rs` isolates Linux `TCP_FASTOPEN` and
  `TCP_DEFER_ACCEPT` socket operations.
- `server/request/keepalive.rs`: centralized request-level keep-alive
  decisions, inherited `Keep-Alive: timeout=`, and connection reuse
  conditions.
- `server/request/process/`: separates the normal request flow from the named
  location subrequest flow.
- `server/request/process/prepare.rs`: explicitly runs the
  `server rewrite -> find location -> location rewrite -> try_files` phase
  loop. `last`/internal redirects no longer recursively re-enter the entire
  request handler, and a shared 10-URI-change budget prevents rewrite loops
  from exhausting the worker stack.
- `server/request/rewrite/`: separates condition evaluation, rewrite/return
  rules, and ordered script execution. Scripts execute `set`, `rewrite`,
  `return`, `break`, and `if` in declaration order, using
  `None/Continue/Restart/Response` to represent control flow.
- `config/compile/server/rewrite.rs`: after scope compilation, emits rewrite
  action lists for HTTP server/location in source declaration order, avoiding
  semantics changes caused by field-by-field execution.
- `server/request/post_action.rs`: path/named-location internal execution after
  request completion, background-task isolation, a maximum 10-redirect chain,
  and preservation of the original client response.
- `http/request/head.rs`: bounded HTTP/1 request-line and header reads,
  per-line/total limits, invalid-header marking, and application of
  `ignore_invalid_headers` based on virtual-host selection.
- `http/response/body.rs`: response body representation, memory boundaries and
  length calculation for file/multipart bodies; `http/response.rs` retains only
  response control and the public API.
- `server/response/sent.rs`: synchronizes actual sent semantics during access
  logging, including `$sent_http_connection`, `$sent_http_keep_alive`,
  `$sent_http_transfer_encoding`, and the absence of
  `$sent_http_content_length` for chunked responses.
- `config/compile/directives/msie.rs` and `server/response/msie.rs`:
  MSIE/Chrome error-page padding, MSIE refresh-page rewriting, and UA
  compatibility checks, separated between configuration compilation and
  response handling.
- `config/compile/directives/dav.rs`, `config/compile/model/dav.rs`, and
  `server/response/dav/`: WebDAV directive parsing, scope models, and
  PUT/DELETE/MKCOL/COPY/MOVE file operations.
- `config/compile/directives/request_headers.rs`: request-header buffer and
  policy directive parsing, HTTP/server inheritance, and default validation.
- `server/early_hints.rs`, `http/response/early_hints.rs`, and
  `server/http2/informational.rs`: `early_hints` predicate evaluation, HTTP/1.1
  103 encoding, and HTTP/2 informational HEADERS sending are independent.
- `config/compile/directives/store.rs`, `config/compile/model/store.rs`, and
  `server/upstream/store.rs`: proxy, FastCGI, uWSGI, and SCGI `*_store`/
  `*_store_access` parsing, scope inheritance, and atomic response writes.
- `config/compile/directives/buffering.rs`,
  `config/compile/model/upstream_buffering.rs`, and
  `config/compile/temp_path.rs`: shared buffering parameter models for all four
  upstream types, Nginx default/merge validation, and path hierarchy resolution
  shared by request bodies and upstream temporary directories.
- `http/upstream/buffering.rs`: independent `X-Accel-Buffering` parsing and
  runtime buffering-policy selection, avoiding further header checks in
  proxy/gateway exchange code.
- `server/upstream/streaming.rs` and
  `server/upstream/proxy_v2/streaming.rs`: incremental HTTP/1 and HTTP/2
  upstream body/trailer reads, rate limiting, bounded-channel backpressure, and
  downstream streaming handoff. The HTTP/2 connection driver uses cloneable
  guards so the response stream is not aborted early by connection
  destruction.
- `server/upstream/tee.rs`, `server/upstream/cache/tee.rs`, and
  `server/upstream/store_stream.rs`: streaming body fan-out, temporary
  write/cancel-cleanup/complete-commit handling for cache entries and store
  files; downstream disconnects can cancel blocked prepare/write/finish calls,
  and cache locks are held by streaming tasks through atomic commit.
- `config/compile/directives/cache.rs`, `config/compile/model/cache.rs`, and
  `server/upstream/cache/`: `proxy_cache_path` zone models, `proxy_cache*` scope
  directives, cache keys, single-file metadata/body formats, atomic writes,
  runtime registries, zones, and keyed cache locks split by responsibility.
  `cache/loader.rs` scans and validates existing entries and removes corrupt
  files during cold start; `cache/manager.rs` handles `inactive`, `max_size`,
  and `min_free` eviction; `cache/fs.rs` provides shared directory traversal;
  and `cache/manager/disk.rs` isolates `statvfs` space queries.
- `http/temp_file.rs`, `http/upstream/{buffer,body_reader}.rs`, and
  `server/upstream/gateway/cgi.rs`: temporary-file guards shared by request
  bodies and upstream responses, incremental upstream body buffering, HTTP
  chunked/content-length reads, CGI header parsing, and streaming body writes
  to disk.
- `server/upstream/gateway/streaming.rs` and
  `server/upstream/gateway/fastcgi/streaming.rs`: FastCGI/uWSGI/SCGI streaming
  eligibility checks, incremental CGI header parsing, bounded body channels,
  and the FastCGI stdout/stderr/end-request state machine; header parsing and
  response construction are shared with the fully buffered path.
- `server/upstream/memcached/protocol.rs` and
  `server/upstream/memcached/streaming.rs`: incremental Memcached
  `VALUE` header/body/END parsing, bounded body channels, and keepalive
  connection return after body and terminator validation.
- `server/upstream/body_copy.rs`: streaming copies of in-memory, file-backed,
  and multipart response bodies shared by proxy/CGI `*_store` and proxy disk
  cache, avoiding a second full read into memory before writing.
- `server/request/lingering.rs`: HTTP/1.1 lingering close, close-timeout
  policy, and RST handling for timed-out connections.
- `server/response/ranges/`: independent Range parsing, single-range slicing,
  and `multipart/byteranges` responses.
- `server/http2/body.rs`: streaming HTTP/2 response bodies, including file,
  prefix-body, and multipart Range responses.
- `server/http3/`: `quinn` + `h3` QUIC listeners, HTTP/3 request mapping,
  request-body bridging, response header/body/trailer sending, and
  TLSv1.3/ALPN `h3` configuration.
- `server/upstream/proxy_v2/`: HTTP/2 upstream connections, request
  construction, informational response headers, bodies/trailers, streaming
  bodies, and keepalive connection lifecycles split into separate
  implementations.
- `server/upstream/grpc/`: gRPC target resolution, request pseudo-header/body
  construction, HTTP/2 exchange, response header/trailer filtering, and
  TLS/connection pools; configuration parsing lives under
  `config/compile/{parsing,directives,model}/grpc*`.
- `tests/config_compat/`: configuration compatibility tests split by access
  control, variables, routing, gateways, TLS, upstreams, and response behavior.
- `server/tests/`: larger HTTP/TLS, proxy, stream, access-control, and static
  file test sets are further split by behavior; the test entry point only
  handles module orchestration.
- `wasm/{mod,abi,runtime}.rs` and
  `config/compile/{directives,model}/wasm.rs`: Wasmtime engine, compiled module
  cache, instance concurrency limits, core module ABI v1, configuration
  validation, and content-handler runtime integration.

New logic should be placed in the corresponding submodule instead of growing
entry files. When production or test files approach 500 lines, check whether
they can be split further by responsibility.

## Implemented

- Nginx configuration lexing and syntax: quotes, comments, escapes, variables,
  nested blocks, and `${var}`.
- `include`: resolution relative to the configuration directory, with support
  for `*`, `?`, character classes, and cycle detection.
- Configuration contexts: `main`, `events`, `http`, `server`, `location`,
  `upstream`, `if`, and `stream`.
- CLI: `-t`, `-T`, `-q`, `-v`, `-V`, `-s`, `-p`, `-c`, `-e`, and `-g`.
- Processes: daemon mode, PID files, and `stop`/`quit`/`reload`/`reopen`
  signals. Reload re-reads configuration, TLS certificates, and error logs;
  listener socket changes require a restart.
- Error logs: repeatable `error_log` targets, file/`stderr` output, independent
  level filtering per target, and `syslog:server=...` UDP/UNIX datagram output;
  syslog writes dynamically generate RFC 3164 PRI values from the event level.
- HTTP/1.0 and HTTP/1.1: keep-alive, `keepalive_requests`, `keepalive_time`,
  `keepalive_timeout [header_timeout]` (the second argument controls
  `Keep-Alive: timeout=`), `keepalive_min_timeout` (delays entry into the
  reusable/waiting state without changing the total keepalive timeout),
  `keepalive_disable msie6|safari|none`, chunked request bodies,
  `Expect: 100-continue`, Host validation, `merge_slashes`, HTTP/server-level
  `client_header_buffer_size`, `large_client_header_buffers`,
  `ignore_invalid_headers`, `underscores_in_headers`, and `max_headers`
  request-header count/per-line/total limits. Invalid headers are ignored or
  return 400 according to the selected virtual host policy; overlong request
  lines return 414. Request-body limits are also supported, along with
  `client_header_timeout`/`client_body_timeout`/`send_timeout`, inherited
  HTTP/server/location `client_body_buffer_size`,
  `client_body_temp_path [level1 [level2 [level3]]]`,
  `client_body_in_file_only off|on|clean`, and
  `client_body_in_single_buffer`: request bodies exceeding the memory
  threshold or using file-only mode are written to temporary files. `clean`
  and ordinary overflow files are removed after the request, while `on` keeps
  the file. Inherited HTTP/server/location
  `lingering_close off|on|always`/`lingering_time`/`lingering_timeout` are
  supported, as is SO_LINGER RST close for timed-out connections with
  `reset_timedout_connection`; TCP and `listen unix:/path` UNIX domain socket
  listeners are supported. Inherited HTTP/server/location `tcp_nodelay on|off`
  (default `on`) is applied before the TLS handshake using the listener's
  default server configuration and before writing the client response using
  the final location configuration. As in Nginx, it only enables the option;
  `off` does not actively disable an option already enabled. HTTP
  proxy/FastCGI/uWSGI/SCGI/Memcached/gRPC upstream connections also apply the
  final location's `tcp_nodelay` setting. `listen` options
  `backlog=`, `rcvbuf=`, `sndbuf=`, `reuseport`, `bind`,
  `ipv6only=on|off`, `so_keepalive=on|off|idle:interval:count`, `fastopen=`,
  `deferred`, and Linux `multipath` (MPTCP) enter the unified socket binding
  flow. Duplicate socket parameters for the same address and duplicate
  `default_server` declarations are rejected during configuration validation;
  parameter changes trigger the reload protection requiring a restart when
  listener sockets change.
- HTTP/2: TLS ALPN negotiation through `listen ... ssl http2` or http/server
  `http2 on`, plus automatic prior-knowledge preface detection for cleartext
  listeners. `http2_max_concurrent_streams`, `http2_body_preread_size`,
  `http2_chunk_size`, and Nginx-compatible memory/index tuning directives are
  supported, along with concurrent request streams, request-body reads,
  `max_headers`, connection-level response-header filtering, and streaming
  static-file responses. H2 connection lifetime and GOAWAY are managed through
  `keepalive_requests`, `keepalive_timeout`, and `keepalive_time`.
  `http2_push`, `http2_push_preload`, `http2_max_concurrent_pushes`,
  `http2_max_requests`, `http2_recv_timeout`, `http2_idle_timeout`,
  `http2_max_field_size`, and `http2_max_header_size` are warned about and
  ignored as obsolete directives with Nginx 1.31.9 semantics; their argument
  values and duplicates are not parsed. Header-list limits are derived from
  `large_client_header_buffers`, and fields larger than a single buffer are
  rejected before routing. Request trailers are treated as a connection-level
  protocol error and receive `GOAWAY(PROTOCOL_ERROR)` with Nginx semantics;
  unconsumed ordinary request bodies are drained in the background before
  connection reuse. HTTP/1.1 fallback remains available.
- HTTP/3/QUIC: `listen ... quic` creates UDP QUIC listeners using TLSv1.3 and
  ALPN `h3`; supports HTTP/server-level `http3 on|off` inheritance, the
  `$http3` variable, concurrent requests, incremental request-body reads, the
  existing router/proxy/static response pipeline, streaming response bodies,
  request/response trailers, and HTTP 103 informational responses. Request
  trailers are forwarded to HTTP/2 upstreams; HTTP/1.1 upstreams carry them in
  a chunked request body. QUIC listeners reuse the HTTP TLS certificate and SNI
  configuration for the same address. Reload and ACME certificate renewal call
  `Endpoint::set_server_config` so new connections use the replacement
  certificate and TLS configuration while existing connections remain intact.
  Explicit `http3 off` does not bind a QUIC listener, and reload closes the
  endpoint for a removed QUIC listener. HTTP/1.1 and HTTP/2 responses
  automatically receive `Alt-Svc: h3=":<port>"; ma=86400` unless an `Alt-Svc`
  header is already present. The single-process runtime creates one UDP
  endpoint, and all QUIC datagrams are received and sent through one quinn
  endpoint driver; 0-RTT is disabled.
- PROXY protocol v1/v2: `listen ... proxy_protocol` for HTTP and stream
  streaming listeners, with v1 `TCP4`, `TCP6`, and `UNKNOWN`; v2 IPv4/IPv6
  address headers, TLVs, and addressless semantics for `LOCAL`/unsupported
  commands; and `$proxy_protocol_addr`, `$proxy_protocol_port`,
  `$proxy_protocol_server_addr`, and `$proxy_protocol_server_port`. v2 TLV
  support includes `alpn`, `authority`, `unique_id`, `ssl`, `netns`, `ssl_*`,
  and `0xNN` variables.
- Static files: `root`, `alias`, `index`, directory redirects, autoindex and
  `autoindex_format html|json|jsonp|xml`, single-range and
  `multipart/byteranges` multi-range Range requests, `max_ranges`,
  `random_index`, `empty_gif`, `flv` and `?start=` pseudo-streaming offsets,
  ETag, Last-Modified, `try_files`, and `error_page` with status preservation
  and override semantics. The error_page list replaces inherited entries as a
  whole per scope, and HTTP/server/location inheritance plus the default-off
  `recursive_error_pages` are supported.
- Path security: HTTP/server/location inherited
  `disable_symlinks off|on|if_not_owner [from=...]`, with support for variable
  `from=` prefixes; applies to static files, index, try_files, gzip_static, and
  flv candidate paths.
- Redirects: default absolute `Location`, plus inheritance and scope semantics
  for `absolute_redirect`, `server_name_in_redirect`, and `port_in_redirect`.
- WebDAV: HTTP/server/location inheritance for
  `dav_methods put|delete|mkcol|copy|move|off`, `create_full_put_path`,
  `min_delete_depth`, and `dav_access`; supports atomic PUT replacement,
  collection creation and recursive deletion, COPY/MOVE for files and
  directories, `Destination`, `Depth`, and `Overwrite` validation, and
  preservation of file permissions and modification times.
- WebAssembly content handler: HTTP-level `wasm_engine`/`wasm_module_cache` and
  server/location-level `wasm_module`, `wasm_phase content`, `wasm_timeout`,
  `wasm_memory_max`, `wasm_fuel`, `wasm_request_body_max`,
  `wasm_response_body_max`, `wasm_max_instances`, and
  `wasm_isolation in-process`; uses in-process Wasmtime, epoch wall-clock
  timeouts, optional fuel, linear-memory/request-body/response-body/
  response-header/instance concurrency limits, a compiled-module cache keyed by
  canonical path/size/mtime/digest, and reload snapshot switching. Implements
  versioned core module ABI v1 (`starling_abi_version`, `starling_alloc`, and
  `starling_handle`) and does not expose WASI file, network, environment, or
  subprocess capabilities.
- Error-page compatibility: inherited HTTP/server/location
  `msie_padding on|off` (default `on`) appends compatibility padding to
  built-in 4xx/5xx error pages for MSIE/Chrome;
  `msie_refresh on|off` (default `off`) rewrites MSIE 301/302 responses as
  200 Refresh HTML.
- Virtual hosts: exact, wildcard, and regex `server_name`, plus default server.
- Location: `=`, `^~`, prefix, regex, named, and nested locations.
- Rewrite: `return`, `rewrite`, `set`, `if`, `break`, variables, and
  request-phase redirects. `set`/`rewrite`/`return`/`if` form a single script in
  declaration order; actions inside `if` share current variable state with
  subsequent rules. `last` re-matches the location without replaying server
  rewrites; URI rewrites without a flag re-match after the script ends; `break`
  ends the script and keeps the current location. URI redirect loops return 500
  at the Nginx default limit. `return` status codes accept `0..=999` per the
  Nginx source, and single-argument redirects accept only `http://`,
  `https://`, or `$scheme` prefixes. External `rewrite` replacements also
  include `$scheme` and always generate 302, even with `last`/`break`. A
  trailing `?` in a replacement discards the original arguments while
  preserving the explicit query string before it. `post_action` supports
  HTTP/server/location/`if` inheritance, path or named-location targets,
  query-string forwarding, and a maximum 10-step chain; its result does not
  replace or write back the original client response.
- `mirror`: scope inheritance and background subrequests for `mirror uri|off`
  and `mirror_request_body`, preserving the original method and query
  parameters and preventing mirror subrequests from recursively mirroring.
- HTTP `CONNECT` tunnels: `tunnel_pass` supports server/location/`if` scopes,
  server-level inheritance, direct addresses, and named upstreams, with
  `$host:$request_port` as the default target. Supports `tunnel_bind`, socket
  parameters, connect/read/write timeouts, `tunnel_next_upstream` failover,
  original bidirectional half-close propagation, and `tunnel_buffer_size`.
  `tunnel_pass https://...` supports TLS upstreams and reuses
  `proxy_ssl_server_name`, `proxy_ssl_name`, `proxy_ssl_verify`,
  `proxy_ssl_trusted_certificate`, client certificates, and protocol/cipher
  configuration. `tunnel_send_lowat` is currently retained only as a
  configuration compatibility entry; platform differences remain a future
  compatibility item.
- Variables and mapping: exact, hostname-wildcard, and regex matching for
  `map`; IPv4/IPv6 CIDR, IPv4 ranges, `default`, and custom address variables
  for `geo`; deterministic MurmurHash2 distribution for `split_clients`; and
  common variables including `$arg_*`, `$cookie_*`, `$scheme`, `$https`,
  `$realip_remote_port`, `$proxy_host`, `$proxy_port`, `$request_time`,
  `$request_length`, `$request_body`, `$request_body_file`, `$upstream_status`,
  `$upstream_response_time`, dynamic `$upstream_http_<name>`/
  `$sent_http_<name>` response-header variables, protocol-generated
  `$sent_http_connection`/`$sent_http_keep_alive`/
  `$sent_http_transfer_encoding` variables,
  `$upstream_trailer_<name>`/`$sent_trailer_<name>` trailer variables,
  `$request_id`, and `$hostname`. In rate-limit keys,
  `$binary_remote_addr` uses 4-byte IPv4 or 16-byte IPv6 network-order
  addresses with Nginx semantics.
- Headers/access: `add_header`, `add_trailer` (HTTP/server/location/if,
  HTTP/1.1 chunked and HTTP/2, with variables, status filtering, and `always`),
  HTTP/server/location inherited `server_tokens off|on|build` (also affects the
  `Server` response header and built-in error-page version information),
  `chunked_transfer_encoding on|off` (disabling it omits HTTP/1.1 chunked
  trailers; HTTP/2 trailers are unaffected), HTTP/server/location/if
  `add_header_inherit`/`add_trailer_inherit` `on|off|merge` modes, `expires`
  with `off`/`epoch`/`max`, relative times, `@daily`, `modified`, and variables,
  `if_modified_since off|exact|before`, `charset`, `source_charset`,
  `override_charset`, `charset_types`, HTTP-level `charset_map` (two-way
  single-byte/UTF-8 conversion with entity fallback for unmapped characters),
  `allow`, and `deny`.
- Hotlink protection: `valid_referers` `none`, `blocked`, `server_names`,
  exact/wildcard hosts, host/path prefixes, and regex rules, plus the
  `$invalid_referer` variable and server/location inheritance override
  semantics.
- URL signing: `secure_link`, `secure_link_md5`, and legacy
  `secure_link_secret`, with `$secure_link`, `$secure_link_expires`, expiration
  checks, and HTTP/server/location inheritance overrides.
- Browser variables: `modern_browser`, `ancient_browser`, and corresponding
  value directives, with browser version comparison, `unlisted`,
  `netscape4`, `$msie`, `$modern_browser`, `$ancient_browser`, and scope
  inheritance.
- User identification: `userid off|log|v1|on` plus `userid_name`,
  `userid_domain`, `userid_path`, `userid_expires`, `userid_flags`,
  `userid_p3p`, and `userid_mark`; provides `$uid_got`, `$uid_set`,
  `$uid_reset`, and Cookie reuse/reset semantics.
- Access control: `allow`/`deny` and `limit_except` method-level rules. Per
  Nginx `NGX_HTTP_LMT_CONF`, this context accepts only `allow`, `deny`,
  `auth_basic`, `auth_basic_user_file`, `access_log`, and `proxy_pass` without
  a URI component; directives such as `grpc_pass`, `memcached_pass`, `charset`,
  and `post_action` are explicitly rejected. Without `proxy_pass`, the parent
  location's proxy handler is retained.
- HTTP Basic Auth: `auth_basic`, `auth_basic_user_file`, and `$remote_user`,
  supporting `{PLAIN}`, `{SHA}`, `{SSHA}`, APR1, bcrypt, and system `crypt()`
  password formats; `auth_delay` can add a fixed delay to 401 responses to slow
  credential probing.
- Subrequest authorization: `auth_request`, `auth_request_set`, and
  `satisfy any|all`.
- HTTP rate limiting: leaky-bucket rates, burst, delay, and nodelay for
  `limit_req_zone`/`limit_req`; concurrent connection limits for
  `limit_conn_zone`/`limit_conn`; custom status codes, dry run, and
  `$limit_req_status`/`$limit_conn_status`. Rate-limit keys use raw bytes and
  enforce Nginx's 65535/255-byte length limits.
- Runtime status: `stub_status`, `$connections_active`, `$connections_reading`,
  `$connections_writing`, and `$connections_waiting`.
- Real IP: trusted proxy IPv4/IPv6 addresses, CIDR and `unix:` peers,
  `X-Real-IP`/`X-Forwarded-For`, `proxy_protocol` request headers, and
  recursive address resolution.
- HTTP proxy: TCP or UNIX domain socket `proxy_pass`, path replacement and
  escaping, request header/body control, `proxy_set_header`, `proxy_method`,
  `proxy_set_body`, `proxy_redirect`, `proxy_cookie_domain`,
  `proxy_cookie_path`, `proxy_cookie_flags` (name/regex matching, variable
  flags, and SameSite normalization), upstream response-header hiding/passing,
  and `proxy_store on|off|path` (stores only complete `200` responses; paths
  support variables; relative paths are based on the prefix directory; writes
  to a temporary file in the same directory first, then atomically replaces it
  and applies `Last-Modified`) plus `proxy_store_access user|group|all`
  `rw|r` permission combinations. Also supports `proxy_ignore_headers`,
  optional upstream chunked trailer pass-through with `proxy_pass_trailers`,
  `X-Accel-Redirect` internal redirects, `X-Accel-Limit-Rate` response rate
  limiting, `X-Accel-Charset` charset overrides, `proxy_intercept_errors`,
  `proxy_force_ranges` byte-range responses (including
  `multipart/byteranges` multi-range), `proxy_limit_rate` upstream response read
  throttling, staged `proxy_connect_timeout`, `proxy_send_timeout`, and
  `proxy_read_timeout` timeouts, and `proxy_http_version 1.0|1.1|2` (HTTP/2 uses
  prior knowledge for cleartext upstreams and ALPN `h2` for HTTPS upstreams,
  with request/response bodies, upstream trailers, and 103 Early Hints).
  Under `proxy_buffering off` or `X-Accel-Buffering: no`, HTTP/1.1/HTTP/2
  upstream bodies are streamed chunk by chunk without waiting for upstream EOF,
  respecting `proxy_read_timeout` and `proxy_limit_rate`. Weighted round-robin,
  `least_conn`, `least_time header|last_byte [inflight]`, `ip_hash`,
  `hash [consistent]`, `random [two]`, upstream
  `max_conns`/`max_fails`/`fail_timeout` passive health checks, backup and
  failover, `proxy_next_upstream`/`tries`/`timeout` retry control, and
  `early_hints` predicate filtering of upstream HTTP 103 responses forwarded
  through HTTP/1.1 or HTTP/2 are supported, following
  `proxy_hide_header`/`proxy_pass_header` response-header rules. Upstream
  keepalive is enabled by default with
  `keepalive_time`/`keepalive_timeout`/`keepalive_requests` connection reuse.
  `proxy_http_version 2` supports HTTP/2 upstream connection reuse and
  concurrent streams on the same connection. `proxy_bind` supports
  http/server/location inheritance, variable evaluation, and TCP local source
  binding; `proxy_socket_keepalive`/`proxy_socket_rcvbuf`/`proxy_socket_sndbuf`
  configure HTTP upstream sockets and connection-pool isolation. HTTPS
  upstreams support TLS connection setup with `proxy_ssl_name`/SNI,
  `proxy_ssl_session_reuse`, `proxy_ssl_protocols`, `proxy_ssl_ciphers`,
  `proxy_ssl_verify`, `proxy_ssl_verify_depth`, trusted certificates, and CRL
  validation, plus `proxy_ssl_certificate`/`proxy_ssl_certificate_key` client
  certificates and `proxy_ssl_password_file` encrypted-key passphrases.
  `proxy_headers_hash_max_size`/`proxy_headers_hash_bucket_size` retain scope
  inheritance, defaults, and validation, although the Rust side does not use
  Nginx's hash-bucket implementation. `proxy_ssl_session_reuse` reuses sessions
  through a shared client TLS configuration cache, which is cleared on
  configuration reload.
- gRPC: `grpc_pass` supports bare addresses, `grpc://`, `grpcs://`, variables,
  and named upstreams. Cleartext upstreams use HTTP/2 prior knowledge;
  `grpcs://` uses TLS with ALPN `h2`. The original request `:path` is
  preserved, `:scheme` is generated from the target, `:authority` defaults to
  the target/named upstream and can be overridden by
  `grpc_set_header Host`; request bodies, response bodies, HTTP/2 trailers, and
  `TE: trailers` are forwarded. Response DATA and trailers are written back
  chunk by chunk after request headers complete without waiting for upstream
  EOF; HTTP/1.1 downstreams use chunked trailers and HTTP/2 downstreams use
  trailing HEADERS. Supports `grpc_hide_header`, `grpc_pass_header`,
  `grpc_ignore_headers`, `grpc_intercept_errors`, `grpc_buffer_size`,
  `grpc_connect_timeout`/`send_timeout`/`read_timeout`,
  `grpc_next_upstream`/tries/timeout, `grpc_bind`, socket
  keepalive/rcvbuf/sndbuf, and HTTP/2 connection reuse for named upstream
  `keepalive`. `grpcs://` also supports SNI/name, session reuse, protocols,
  cipher suites, certificate-chain and CRL validation, client certificates,
  and encrypted-key passphrases; `grpc_ssl_conf_command` explicitly errors
  under the rustls backend; `grpc_ssl_certificate_cache` currently remains only
  a configuration compatibility entry and does not control the existing
  internal `ClientTlsConfigCache`.
- FastCGI: TCP or UNIX domain socket `fastcgi_pass` and named upstreams,
  `include fastcgi_params`, `fastcgi_param ... if_not_empty`, `fastcgi_index`,
  `fastcgi_split_path_info`/`$fastcgi_path_info`, request header/body
  forwarding, `fastcgi_intercept_errors`, `fastcgi_force_ranges`,
  `fastcgi_limit_rate`, `fastcgi_next_upstream` retry control,
  `fastcgi_keep_conn`, `fastcgi_catch_stderr`, connect/read/write/send
  timeouts, and `$fastcgi_script_name`. Named upstreams with `keepalive` reuse
  FastCGI connections; a `fastcgi_catch_stderr` match returns an invalid
  upstream response. `fastcgi_hide_header`/`fastcgi_pass_header` control
  response-header filtering; `fastcgi_ignore_headers` supports Nginx's ignore
  items and applies `X-Accel-Redirect` internal redirects,
  `X-Accel-Limit-Rate` throttling, and `X-Accel-Charset` overrides by default.
  `fastcgi_bind`, `fastcgi_socket_keepalive`, `fastcgi_socket_rcvbuf`/
  `fastcgi_socket_sndbuf` support scope inheritance, variable evaluation, and
  connection-pool isolation. `fastcgi_store`/`fastcgi_store_access` support
  atomic writes of complete `200` responses to variable paths. CGI response
  headers such as `Status`, `Location`, and `Content-Type` are mapped.
  Under `fastcgi_buffering off` or `X-Accel-Buffering: no`, `FCGI_STDOUT` is
  forwarded record by record after CGI headers complete while
  `FCGI_STDERR` and `FCGI_END_REQUEST` continue to be processed; a
  `fastcgi_catch_stderr` match terminates the response stream.
- uWSGI and SCGI: TCP or UNIX domain socket `uwsgi_pass`/`scgi_pass` and named
  upstreams; uWSGI TLS connection setup for `suwsgi://`/`uwsgi_ssl_*`
  (including SNI, certificate validation, CRL, client certificates,
  protocol/cipher suites, and key passphrases); `include
  uwsgi_params`/`scgi_params`, parameters and `if_not_empty`, request
  header/body forwarding, `uwsgi_string` literal trailing data, uWSGI
  modifiers, `*_next_upstream` retry control, connect/read/write/send
  timeouts, error-page interception, `*_force_ranges`,
  `*_hide_header`/`*_pass_header` response-header filtering, `*_limit_rate`,
  `*_ignore_headers`, `X-Accel-Redirect` internal redirects,
  `X-Accel-Limit-Rate` throttling, and `X-Accel-Charset` overrides. Also
  supports `uwsgi_bind`/`scgi_bind`,
  `uwsgi_socket_keepalive`/`scgi_socket_keepalive`,
  `uwsgi_socket_rcvbuf`/`uwsgi_socket_sndbuf`, and
  `scgi_socket_rcvbuf`/`scgi_socket_sndbuf`; `uwsgi_store`/`scgi_store` and
  corresponding `*_store_access` provide the same response persistence and
  permission semantics as FastCGI. Under `uwsgi_buffering off`/
  `scgi_buffering off` or `X-Accel-Buffering: no`, bodies are written back
  chunk by chunk after CGI headers complete. Supports
  `$uwsgi_script_name`/`$scgi_script_name`.
- Memcached: `memcached_pass` supports TCP, UNIX domain sockets, and named
  upstreams. `$memcached_key` escapes control characters, spaces, and `%` per
  Nginx rules and only accepts `GET`/`HEAD`; `VALUE`/`END` responses, body
  lengths, and trailers are strictly validated, with 404 for misses.
  `memcached_gzip_flag` adds `Content-Encoding: gzip` on a match. Supports
  `memcached_bind`, `memcached_socket_keepalive`,
  `memcached_connect_timeout`/`send_timeout`/`read_timeout`,
  `memcached_buffer_size`, `memcached_next_upstream` (`error`, `timeout`,
  `invalid_response`, `not_found`, `off`) and its tries/timeout directives.
  Nginx's 404/error-page interception and force-ranges semantics are enabled by
  default; named upstreams configured with `keepalive` reuse backend
  connections after complete `VALUE` and `END` responses. Ordinary `GET`
  requests immediately stream the body chunk by chunk after parsing the
  `VALUE` length while validating the body length and `\r\nEND\r\n` marker in
  the background; successful connections return to the keepalive pool.
  `HEAD`, Range requests, or filtering features requiring the full body fall
  back to full buffering.
- Upstream byte ranges: only `*_force_ranges on` enables `Range` for buffered
  proxy/CGI responses with status 200; a single range returns `Content-Range`,
  while multiple ranges return `multipart/byteranges` and honor `max_ranges`.
  An upstream-provided `Accept-Ranges` header alone does not enable slicing.
- Upstream read throttling: `proxy_limit_rate`, `fastcgi_limit_rate`,
  `uwsgi_limit_rate`, and `scgi_limit_rate` apply asynchronous backpressure by
  cumulative byte rate while reading response bodies, with
  http/server/location inheritance, variable evaluation, and `0` (disabled).
- Upstream response buffering configuration: `proxy_*`, `fastcgi_*`, `uwsgi_*`,
  and `scgi_*` `*_buffering`, `*_buffers`, `*_buffer_size`,
  `*_busy_buffers_size`, `*_temp_path`, `*_max_temp_file_size`, and
  `*_temp_file_write_size` are parsed and validated with Nginx defaults, scope
  inheritance, and combination constraints. `*_buffer_size` also limits
  upstream/CGI response headers, returning `502` when exceeded. Response
  bodies first use the memory allowance represented by `*_buffers`, then spill
  to hashed directories under `*_temp_path` in chunks controlled by
  `*_temp_file_write_size`; exceeding the memory allowance plus
  `*_max_temp_file_size` returns `502`, and temporary files are removed after
  the response is sent. `proxy_store` and the three CGI store families can copy
  file-backed response bodies directly.
  HTTP/1.1 and HTTP/2 upstreams under `proxy_buffering off` or
  `X-Accel-Buffering: no`, when Range, trailers, gzip/gunzip, charset,
  sub_filter, addition, SSI, and error-page interception are not involved, are
  bridged chunk by chunk to downstream HTTP/1.1 chunked or HTTP/2 DATA with
  bounded channels applying backpressure. The direct producer also monitors
  downstream receiver closure and cancels body tasks still waiting on upstream
  I/O immediately after a downstream write failure. `X-Accel-Buffering: yes`
  forces configured memory/temp-file buffering, and
  `*_ignore_headers X-Accel-Buffering` restores the configured default. `no`
  also disables proxy cache; during streaming pass-through, cache and store
  tee into bounded asynchronous queues, and temporary files are atomically
  renamed only after the upstream completes and the downstream has not
  disconnected early. gRPC responses are written directly through the same
  HTTP/2 upstream streaming pipeline. FastCGI, uWSGI, and SCGI also enter the
  same chunked write-back path after CGI response headers complete; FastCGI
  streaming responses continue processing stderr and end records, and the
  connection is not returned to the keepalive pool. Memcached `VALUE` bodies
  are written chunk by chunk by declared length and the connection is reused
  only after the trailing `END` validation succeeds; features requiring a full
  response body still use full buffering. `*_busy_buffers_size` is used as the
  `BodyWindow` byte-limited send window, limiting memory and file-backed data
  simultaneously in the downstream busy state. A downstream disconnect cancels
  producers waiting for data, capacity, or external I/O and cleans up temporary
  files; per-buffer output-chain semantics still need further alignment with
  Nginx.
- HTTP proxy disk cache: HTTP-level `proxy_cache_path` (`levels`, `keys_zone`,
  `inactive`, `max_size`, `min_free`, `use_temp_path`, and loader/manager
  compatibility parameters), plus http/server/location-level `proxy_cache`,
  `proxy_cache_key`, `proxy_cache_valid`, `proxy_cache_min_uses`,
  `proxy_cache_max_range_offset`, `proxy_cache_methods`,
  `proxy_cache_convert_head`, `proxy_cache_bypass`, `proxy_no_cache`,
  `proxy_cache_lock`, `proxy_cache_lock_timeout`, `proxy_cache_lock_age`,
  `proxy_cache_revalidate`, `proxy_cache_background_update`, and
  `proxy_cache_use_stale`; exposes `$upstream_cache_status` values `MISS`,
  `HIT`, `EXPIRED`, `STALE`, `UPDATING`, `REVALIDATED`, and `BYPASS`.
  Cache keys use SHA-1 file names with directories according to `levels`;
  bodies and metadata are written to temporary files in the same directory and
  atomically renamed, and hits send file-backed bodies directly without
  re-reading them into memory. Cache timestamps use millisecond precision, so
  `proxy_cache_valid` `ms` values are not rounded up to at least one second.
  The entry format version is `STRLC004`; older cache files are removed by the
  loader. `GET`/`HEAD` are cached by default, and `POST` can be added through
  `proxy_cache_methods`. `proxy_cache_convert_head` converts cacheable `HEAD`
  origin requests to `GET` by default and shares the cache entry with `GET`;
  converted bodies are fully buffered first so that a downstream HEAD that
  does not read the body cannot leave an incomplete cache entry or discard the
  cache tee. Only complete responses are cached, following
  `Cache-Control`, `Expires`, `Set-Cookie`, `Vary: *`, and `X-Accel-Expires`
  no-cache/expiry semantics. Per Nginx rules, `Cache-Control` prefers
  `s-maxage`; invalid or zero `max-age`/`s-maxage` prevents caching; invalid
  `X-Accel-Expires` keeps the configured fallback; `@0` clears upstream
  overrides and falls back to `proxy_cache_valid` with Nginx semantics.
  `Cache-Control: stale-while-revalidate` can trigger background refresh when
  `proxy_cache_background_update on`, allowing concurrent requests during the
  update to use the stale response; `stale-if-error` falls back to stale
  responses during upstream connection failures without additional
  `proxy_cache_use_stale` configuration.
  `proxy_cache_max_range_offset` is unlimited by default; when set, with Nginx
  semantics only `bytes=` ranges whose start is below the threshold may be
  written to cache, suffix ranges are not cached, and `0` prevents all
  `Range` responses from being cached without affecting reads of existing
  entries. The background cache manager scans the zone every second and
  removes entries unused past `inactive`; expired entries are retained until
  inactive for stale fallback, and hits refresh file mtime. `max_size` and
  Unix `min_free` evict in least-recently-used order, with per-round deletion
  count and interval controlled by `manager_files`/`manager_sleep`. The
  `keys_zone` size is the in-memory cache-index budget; the loader loads
  existing entries within the budget and removes files beyond it. Unchanged
  zones are reused across reload, preserving indexes and `min_uses` counts.
  `proxy_cache_lock` coalesces concurrent misses by cache key within the
  process; requests waiting longer than `lock_timeout` go to origin without
  writing cache, and a holder exceeding `lock_age` allows one waiting request
  to take over. `proxy_cache_use_stale` supports `error`, `timeout`,
  `invalid_header`, `updating`, `http_403`, `http_404`, `http_429`, `http_500`,
  `http_502`, `http_503`, and `http_504`.
  `proxy_cache_revalidate on` sends cached `Last-Modified`/`ETag` conditional
  headers when revalidating expired entries; a `304` from the upstream
  continues sending the cached body and refreshes validity according to cache
  status configuration or the `304` freshness headers. With
  `proxy_cache_use_stale updating`, `proxy_cache_background_update on`
  immediately returns `STALE` content and refreshes the cache in a separate
  task protected by an internal update lock; concurrent requests during the
  update return `UPDATING`. Nginx OSS does not provide
  `proxy_cache_path ... purge=`, so Starling reports it as an invalid
  parameter consistent with OSS configuration validation.
- CGI gateway disk cache: FastCGI, uWSGI, and SCGI reuse the HTTP proxy cache's
  file format, zone/index, loader/manager, and keyed-lock implementation and
  support the corresponding `*_cache_path`, `*_cache`, `*_cache_key`,
  `*_cache_valid`, `*_cache_min_uses`, `*_cache_max_range_offset`,
  `*_cache_methods`, `*_cache_bypass`, `*_no_cache`, `*_cache_lock*`,
  `*_cache_revalidate`, `*_cache_background_update`, and `*_cache_use_stale`
  directives, with the same `$upstream_cache_status` states, 304
  revalidation, expired fallback, and background update semantics.
- Compression: `gzip`, `gzip_types`, `gzip_min_length`, `gzip_comp_level`,
  `gzip_vary`, `gzip_proxied`, `gzip_http_version 1.0|1.1`, `gzip_disable`
  with case-insensitive regex and built-in `msie6`, and
  `gzip_static on|always|off` precompressed files. Degradation detection is
  not implemented, so `gzip_disable degradation` explicitly errors during
  configuration validation.
- Decompression: `gunzip on|off` decompresses gzip responses when the client
  does not accept gzip and corrects `Content-Encoding`, `Content-Length`,
  `Accept-Ranges`, ETag, and `Vary`.
- Response substitution: `sub_filter`, `sub_filter_types`, `sub_filter_once`,
  and `sub_filter_last_modified`.
- Response addition: `add_before_body`, `add_after_body`, and `addition_types`,
  with server/location inheritance, variable URIs, subrequest concatenation,
  and corrected content length, ranges, and ETag.
- SSI: `ssi on|off`, `ssi_silent_errors`, `ssi_ignore_recycled_buffers`,
  `ssi_min_file_chunk`, `ssi_value_length`, `ssi_types`, and
  `ssi_last_modified`, supporting `include virtual|file`, `echo`, `config`,
  `set`, `if`/`elif`/`else`, regex captures, `block`/`stub`, and variable
  evaluation.
- Access logs: HTTP and stream `access_log`, `log_format` (with Nginx-compatible
  variable escaping via `escape=default|json|none`), multiple `access_log`
  targets in the same scope, wholesale target replacement in child scopes,
  `access_log off`, log-path variables, `buffer=`/`flush=` buffering, gzip
  compression with `gzip[=level]`, same-file conflicting-parameter validation,
  configuration-time validation for unknown/duplicate log formats, independent
  `access_log ... if=` filtering, `log_not_found` suppression, and
  HTTP/server/location and stream main/server inherited
  `open_log_file_cache` (`off` or `max=`, `inactive=`, `min_uses=`, `valid=`
  parameters). Supports `syslog:server=...` UDP/UNIX datagram targets,
  `facility=`/`severity=`/`tag=`/`nohostname` parameters, and RFC 3164 message
  formatting.
- TLS: `listen ... ssl`, certificate/private-key loading, rustls, SNI
  certificate selection, HTTP/stream inheritance for `ssl_protocols` and
  TLSv1.2/TLSv1.3 selection, per-SNI HTTPS virtual-host TLS/ALPN configuration,
  common OpenSSL expressions for `ssl_ciphers`/`proxy_ssl_ciphers`,
  `ssl_prefer_server_ciphers`, mutual TLS through
  `ssl_verify_client`/`ssl_verify_depth`/`ssl_client_certificate`/
  `ssl_trusted_certificate`/`ssl_crl` client certificate revocation checks,
  `$ssl_protocol`/`$ssl_cipher`/`$ssl_server_name`/`$ssl_alpn_protocol`/
  `$ssl_client_verify`, `$ssl_client_cert`, `$ssl_client_escaped_cert`,
  `$ssl_client_fingerprint`, `$ssl_client_serial`, and client certificate DN
  and validity variables, plus HTTP/1.1/HTTP/2 ALPN. HTTP/server and stream
  main/server support `ssl_stapling on|off`, `ssl_stapling_verify on|off`,
  `ssl_stapling_file`, `ssl_stapling_responder`, `resolver`, and
  `resolver_timeout`: HTTP and HTTPS OCSP responders are supported. HTTPS
  responder certificate chains are verified through rustls using
  `ssl_trusted_certificate` or the system CA bundle; OCSP response signatures
  are then verified against the trusted certificate and rustls
  `CertifiedKey.ocsp` is hot-swapped. Refresh is scheduled at
  `nextUpdate - 5m`, clamped between 5 minutes and 1 hour; failures keep the
  old response and retry after 5 minutes. `ssl_ocsp on|leaf` with
  `ssl_ocsp_responder` and `ssl_ocsp_cache` performs post-handshake online
  revocation checks for HTTP/1.1, HTTP/2, HTTP/3, and stream client
  certificates. HTTP/3 also exposes the same client-certificate variables as
  HTTP/1.1 and HTTP/2.
- Automatic HTTPS (enabled by default): `auto_https on|off` supports
  `cert_dir=`, `email=`, `staging=on|off`, `acme_directory=`,
  `acme_ca_file=`, `challenge=http-01|dns-01`, `dns_hook=`,
  `dns_propagation_delay=`, `redirect=on|off`, `http_port=`, and `https_port=`,
  or the corresponding standalone `auto_https_*` directives. When enabled, it
  adds HTTPS listeners for servers listening on HTTP ports, generates startup
  placeholder certificates, serves HTTP-01 validation responses on port 80,
  and redirects to HTTPS with 308 by default. Public names are automatically
  issued through ACME v2 and checked/renewed in the background; certificates
  are atomically written and then hot-updated in rustls configuration. HTTP-01
  is used by default; public wildcard names require `challenge=dns-01`.
  DNS-01 invokes an external executable through `dns_hook` to publish and
  clean TXT records, setting `STARLING_ACME_DNS_ACTION=present|cleanup`,
  `STARLING_ACME_DNS_NAME`, `STARLING_ACME_DNS_VALUE`, and
  `STARLING_ACME_DNS_DOMAIN`, and writing the same JSON to stdin. All pending
  records are published first, validation starts after
  `dns_propagation_delay` (default 30 seconds), and cleanup runs uniformly
  afterward; multiple TXT values for the same name invoke `present`/`cleanup`
  separately, so hooks should append and delete by value rather than replacing
  the entire record set. Hooks time out after 60 seconds by default.
  `localhost`, IPs, non-FQDNs, and development names under
  `.localhost`/`.local`/`.test`/`.invalid`/`.example` use a local CA (default
  `auto_https/local-ca.crt`) so browsers stop warning after trusting it.
  HTTP-01 does not support public wildcard names; a server containing both
  local and public names skips ACME and logs a warning to avoid covering both
  categories with one certificate. `auto_https_acme_directory` or `staging on`
  can target test directories such as Pebble or Let's Encrypt staging. See
  `docs/auto-https.en.md` for complete configuration, DNS hook contracts, and
  troubleshooting.
- Stream: TCP and `listen unix:/path` streaming listeners, TCP or UNIX domain
  socket upstream `proxy_pass`, upstream load balancing, `proxy_timeout`,
  `return`, `pass`, `set`, `proxy_next_upstream on|off`,
  `proxy_next_upstream_tries`, and `proxy_next_upstream_timeout` with
  main/server inheritance and connect-phase failover (no retry after
  bidirectional forwarding starts). Also supports stream
  `map`/`geo`/`split_clients` variables and stream main/server
  `allow`/`deny` access control; TCP/UDP session concurrency limits through
  `limit_conn_zone`/`limit_conn` with dry run, log level, and
  `$limit_conn_status`; stream main/server inherited `set_real_ip_from` and
  trusted PROXY protocol source rewriting of `$remote_addr`/`$remote_port`,
  along with `$realip_remote_addr`/`$realip_remote_port`; `ssl_preread on`
  ClientHello pre-reading, `$ssl_preread_protocol`/
  `$ssl_preread_server_name`/`$ssl_preread_alpn_protocols`, and inherited
  `preread_timeout`/`preread_buffer_size`; stream virtual-server selection by
  exact, wildcard, or regex `server_name` SNI with `default_server`; and target
  variable and dynamic upstream-name evaluation for `proxy_pass`. Sends
  `proxy_protocol on|v2` upstream (v1, v2 with CRC32C, written before TCP/TLS
  connection setup, and on the first UDP packet), with main/server inheritance
  and override for `proxy_protocol_timeout` (default 30s); main/server
  inheritance and one-sided EOF/bidirectional half-close semantics for
  `proxy_half_close`; UDP `proxy_requests`/`proxy_responses` datagram count
  limits; and variable-evaluated TCP/UDP bidirectional rate limits for
  `proxy_upload_rate`/`proxy_download_rate`. Supports main/server inheritance
  and TCP/UDP relay buffer control for `proxy_buffer_size`, compatibility
  aliases `proxy_downstream_buffer`/`proxy_upstream_buffer`, and upstream
  socket parameters through `proxy_socket_keepalive`,
  `proxy_socket_rcvbuf`, and `proxy_socket_sndbuf`. `proxy_bind` supports
  main/server inheritance, variable evaluation, and TCP/UDP local source
  binding (`transparent` explicitly errors). Stream TLS and upstream TLS
  directives support main/server inheritance (including certificates,
  verification, protocols, cipher suites, SNI, and ALPN), `listen ... ssl`
  client TLS termination, per-SNI stream virtual-server certificate, TLS
  protocol, cipher suite, and client-certificate verification configuration,
  upstream TLS through `proxy_ssl on` with
  `proxy_ssl_protocols`/`proxy_ssl_ciphers`/`proxy_ssl_alpn`, variable
  evaluation for `proxy_ssl_name`, and SNI switching through
  `proxy_ssl_server_name`; downstream TLS sessions send SNI, ALPN, TLS
  version, and cipher-suite TLVs to upstream PROXY protocol v2. Also supports
  `tcp_nodelay`, `ssl_handshake_timeout`, `ssl_reject_handshake`, `ssl_alpn`,
  and `listen ... udp` datagram proxying with `listen ... proxy_protocol`.
  TCP/UDP `access_log`, `log_format`, and common session, upstream, PROXY
  protocol, and SSL variables are available; stream main/server `error_log`
  supports inheritance, overrides, multiple targets, and independent level
  filtering. `pass` supports static or variable targets, matching local
  TCP/UNIX listeners by listener family/port (including wildcard listeners),
  the target listener's own TLS/PROXY protocol handling and socket parameters,
  and loop protection for up to 10 pass hops, while rejecting UDP and
  connections with preread data.

## Build

```bash
cargo build --release
cargo test --all-targets
```

The executable is written to `target/release/starling`.

## Run

The default prefix and configuration path follow Nginx conventions:

```bash
sudo ./target/release/starling
# Equivalent to:
sudo ./target/release/starling -p /usr/local/nginx -c conf/nginx.conf
```

During development, use the repository configuration and disable the daemon:

```bash
./target/release/starling -t -p "$PWD" -c conf/nginx.conf
./target/release/starling -p "$PWD" -c conf/nginx.conf -g 'daemon off;'
```

Test the configuration and print every included file:

```bash
./target/release/starling -T -p "$PWD" -c conf/nginx.conf
```

Send a signal to a running process:

```bash
./target/release/starling -p "$PWD" -c conf/nginx.conf -s reload
```

## CI and Releases

`scripts/ci.sh` runs formatting checks, Clippy, the complete test suite, and a
release build in sequence. The GitHub Actions `ci.yml` workflow runs the same
checks on pushes to `main`, pull requests, and manual dispatches, and also
validates release archive generation.

`scripts/package-release.sh` creates a release archive with this layout:

```text
starling-<version>-<target>/
├── bin/starling
├── conf/
├── docs/
├── html/
├── logs/
├── README.md
├── README.en.md
├── FEATURES.md
└── FEATURES.en.md
```

`release.yml` builds Linux x86_64/aarch64 archives when a `v*` tag is pushed,
generates `SHA256SUMS`, and publishes them to a GitHub Release. The tag version
must match `Cargo.toml`; a release can also be triggered manually from the
Actions page with an explicit tag.

## Configuration Examples

Static site, reverse proxy, virtual hosts, and TLS:

```nginx
worker_processes auto;
error_log logs/error.log notice;
pid logs/nginx.pid;

events {
    worker_connections 4096;
}

http {
    include mime.types;
    default_type application/octet-stream;
    sendfile on;
    keepalive_timeout 65;

    upstream application {
        server 127.0.0.1:9000 weight=3;
        server 127.0.0.1:9001;
    }

    server {
        listen 80 default_server;
        server_name example.com www.example.com;
        root html;
        index index.html;

        location /api/ {
            proxy_pass http://application/;
            proxy_set_header Host $host;
            proxy_set_header X-Real-IP $remote_addr;
        }

        location /assets/ {
            expires 7d;
            add_header Cache-Control "public";
        }

        location = /health {
            return 200 "ok\n";
        }
    }
}
```

Layer 4 TCP proxy:

```nginx
stream {
    upstream postgres {
        server 10.0.0.12:5432;
        server 10.0.0.13:5432;
    }

    server {
        listen 5432;
        proxy_pass postgres;
        proxy_timeout 10m;
    }
}
```

## Compatibility Boundaries

The following capabilities are not complete. Configuration using them reports
explicit errors instead of silently running with incorrect behavior:

- HTTP/3/QUIC initial implementation boundaries: response trailers, automatic
  Alt-Svc injection, QUIC endpoint server-config hot reload during reload/ACME
  renewal, removal of QUIC listeners, request/response trailers, informational
  responses, HTTP/3 client-certificate variables, and `ssl_ocsp` integration
  are supported; 0-RTT is not. Adding a QUIC listener or rebinding its UDP
  address still requires a restart. The UDP socket currently creates one
  endpoint; adding Tokio worker threads does not add independent QUIC receive
  paths. `listen ... quic` enables HTTP/3 implicitly unless `http3` is
  explicitly configured; explicit `http3 off` does not create a QUIC listener.
- Legacy SSL/TLS protocols; rustls supports only TLSv1.2 and TLSv1.3. For
  compatibility with existing configurations, SSLv2, SSLv3, TLSv1, and TLSv1.1
  in `ssl_protocols` are accepted alongside TLSv1.2/TLSv1.3, but `-t` and the
  runtime error log emit `[warn]` and only TLSv1.2/TLSv1.3 are actually
  enabled. If neither TLSv1.2 nor TLSv1.3 is present in the list, the
  configuration still reports an explicit error.
- `ssl_ciphers`/`proxy_ssl_ciphers`/`grpc_ssl_ciphers`/`uwsgi_ssl_ciphers`
  support rustls ECDHE AEAD suites and common OpenSSL alias, exclusion, and
  combination syntax. Common DHE, CBC/SHA, and other suites that depend on
  legacy OpenSSL algorithms are recognized, omitted from the effective runtime
  set, and reported with `[warn]` in `-t` and the runtime error log. If no
  rustls-supported suite remains, the configuration still reports an explicit
  error.
- `proxy_ssl_conf_command` depends on OpenSSL `SSL_CONF`; rustls has no safe
  equivalent interface for arbitrary command execution, so configuration
  reports an explicit error. Use structured directives such as
  `proxy_ssl_protocols` and `proxy_ssl_ciphers` instead.
- `grpc_ssl_conf_command` behaves like `proxy_ssl_conf_command` and reports an
  explicit error; use `grpc_ssl_protocols` and `grpc_ssl_ciphers` instead.
- `uwsgi_ssl_conf_command` also depends on OpenSSL `SSL_CONF` and reports an
  explicit error at configuration time; use `uwsgi_ssl_protocols` and
  `uwsgi_ssl_ciphers` instead. `uwsgi_ssl_certificate_cache` remains a
  configuration compatibility entry; upstream `ClientConfig` reuse is handled
  internally by `ClientTlsConfigCache`, which ignores the directive's `max=`,
  `inactive=`, `valid=`, and `off` parameters.
- `ssl_verify_client` supports `on`, `optional`, and `optional_no_ca`, with
  client certificate revocation checking through `ssl_crl`; HTTP/stream
  upstreams also support `proxy_ssl_crl`, `proxy_ssl_verify_depth`,
  `uwsgi_ssl_crl`/`uwsgi_ssl_verify_depth`, and client certificates. Encrypted
  PEM private keys for the server and upstreams can be unlocked through
  `ssl_password_file`, `proxy_ssl_password_file`, and
  `uwsgi_ssl_password_file`. `ssl_stapling_verify on` requires
  `ssl_trusted_certificate`, otherwise configuration reports an error.
  HTTP/3 client-certificate information and `ssl_ocsp` checks are integrated;
  see the runtime boundaries above.
- Downstream HTTP/2 supports TLS ALPN and cleartext prior knowledge
  (`listen ... http2` or `http2 on`). Consistent with Nginx 1.31.9, HTTP/1.1
  `Upgrade: h2c` is not supported; HTTP/2 server push was removed in Nginx
  1.25.1, and related legacy directives are warned about as compatibility
  entries and ignored. Cleartext listeners with HTTP/2 enabled still fall back
  to HTTP/1.1 when a complete HTTP/2 preface is not received. HTTP proxy
  upstream `proxy_http_version 2` and gRPC upstreams support cleartext prior
  knowledge and TLS ALPN, and reuse HTTP/2 connections according to the named
  upstream `keepalive` parameter. Proxy responses under `proxy_buffering off`
  or `X-Accel-Buffering: no` can be returned immediately after request headers
  complete, and the connection can continue to carry concurrent streams.
  Ordinary HTTP proxies with `proxy_request_buffering off` forward
  Content-Length or chunked request bodies to HTTP/1.1 and HTTP/2 upstreams
  while reading; gRPC under `grpc_request_buffering off` and the three CGI
  gateways under their corresponding `*_request_buffering off` reuse the same
  request-body streaming path. Inbound HTTP/2 request bodies are also read
  incrementally by `H2BodyTask`; the corresponding buffering remains fully
  buffered when `on`. gRPC, FastCGI, uWSGI, and SCGI response bodies all
  support incremental forwarding.
- Dynamic modules through `load_module`.
- The mail module.
- Advanced FastCGI cache details. Basic forwarding, UNIX domain sockets, disk
  caching, `fastcgi_keep_conn`, named-upstream keepalive reuse, buffering
  configuration, response-header limits, and response-body temporary files are
  supported; other advanced directives remain configuration compatibility
  entries.
- Advanced uWSGI/SCGI cache details and keepalive. Basic gateway forwarding,
  UNIX domain sockets, uWSGI `suwsgi://` TLS, buffering configuration,
  response-header limits, and response-body temporary files are supported.
  FastCGI/uWSGI/SCGI `*_cache_path`, `*_cache`, and key/valid/methods/lock/
  use_stale cache configuration have entered the runtime and support lookup,
  writes, 304 refresh, and stale fallback. All three CGI gateways and the HTTP
  proxy support response-header hide/pass, `*_ignore_headers` configuration,
  `X-Accel-Redirect` internal redirects, `X-Accel-Limit-Rate` response
  throttling, and `X-Accel-Charset` charset overrides. The HTTP proxy cache
  handles `X-Accel-Expires`, `Expires`, `Cache-Control`, `Set-Cookie`, and
  `Vary` with Nginx semantics. `X-Accel-Buffering` changes the runtime
  memory/temp-file buffering policy, and all three CGI gateways participate in
  chunked streaming write-back under `*_buffering off` or
  `X-Accel-Buffering: no`. Gateway cache and store can tee to disk while
  writing chunk by chunk; Range, error-page interception, or body-rewrite
  filters still fall back to full buffering to preserve response-rewrite
  semantics.
- Memcached ordinary `GET` bodies are written chunk by chunk by `VALUE` length
  and the keepalive connection is returned after `END` validation succeeds;
  `HEAD`, Range, and body-rewrite filters still fall back to full buffering.
- Nginx OSS 1.31.9's stream module does not provide `if` or `limit_req`, and
  Starling likewise does not treat them as stream compatibility features.
  Basic stream access logs, `buffer=`/`flush=`/`gzip`/`if=`, and syslog targets
  are supported.
- Stream UDP reuses upstream sessions by client address and supports multiple
  datagrams and responses; UDP upstream TLS is not implemented.
- Linux listener sockets support `reuseport`, `multipath` (MPTCP), `fastopen=`,
  `deferred`, `ipv6only=`, `rcvbuf=`/`sndbuf=`, and tunable `so_keepalive`;
  FreeBSD-only `setfib=` is explicitly rejected. `accept_filter=` remains a
  configuration compatibility entry as in Nginx on platforms without
  `SO_ACCEPTFILTER`, but does not change the socket. Stream UDP rejects
  `backlog`, `fastopen`, `deferred`, `multipath`, `so_keepalive`, and other
  parameters that apply only to connection-oriented sockets.
- Stream upstreams support
  `least_time connect|first_byte|last_byte [inflight]`.
- Remaining Nginx rewrite module boundaries (full module loc_conf rollback
  semantics across complex `if` branches) and HTTP cache purge extensions.
  The cache loader supports cold-start scanning, validation, corrupt-entry
  cleanup, and `keys_zone` capacity constraints.
- The native master/multi-worker process model; the current implementation uses
  one master process and a Tokio multithreaded runtime.

Socket tuning, cache-manager parameters, and SSL parameters that can be parsed
but do not yet change behavior retain compatibility entries. These currently
include `proxy_send_lowat`, `fastcgi_send_lowat`, `gzip_window`, `gzip_hash`,
`gzip_no_buffer`, `postpone_gzipping`, `ssl_certificate_cache`,
`proxy_ssl_certificate_cache`, `grpc_ssl_certificate_cache`, and
`uwsgi_ssl_certificate_cache`. They allow standard Nginx configurations to pass
migration checks and their argument forms are validated according to Nginx
constraints, but they do not change Starling's socket or zlib buffering
policies. Server certificates are loaded only when TLS configuration is built
at startup or reload, so `ssl_certificate_cache` currently has no runtime
effect. The three upstream certificate-cache directives do not control the
internal `ClientTlsConfigCache`, which reuses at most 64 `ClientConfig` entries
and is cleared on reload. `proxy_send_lowat` and `fastcgi_send_lowat` also emit
explicit "no effect" diagnostics to avoid silent misinterpretation.

Before production deployment, run `-t` with the target configuration and use
integration tests to verify proxy, TLS, and timeout behavior.

## Code Layout

- `src/config/`: configuration AST, lexer/parser, includes, and compilation to
  runtime models.
  - `compile/access_log.rs`: access-log parsing and scope override logic.
  - `compile/server/`: HTTP server/location orchestration, ordered rewrite
    scripts, `if`, `limit_except`, listener and route matching.
  - `compile/directives/`: directive dispatch by TLS, response headers,
    proxy/gateway buffering, access limits, rewrite, CGI, and content modules.
  - `compile/variables/`: separate compilation for `map`, `geo`,
    `split_clients`, and rate-limit configuration.
- `src/http/`: HTTP request/response parsing, validation, and upstream response
  parsing.
- `src/server/request/`: HTTP request lifecycle, explicit rewrite phase loops,
  routing, access control, and ordered rewrite scripts.
- `src/server/response/`: static files, error pages, response headers, and
  filter chains.
- `src/server/variables/`: variable evaluation, map/geo lookup, PROXY protocol
  TLVs, and regex captures.
- `src/server/limits.rs`: connection-count state and RAII guards shared by HTTP
  and stream.
- `src/server/upstream/`: proxying, gRPC, retries, load balancing, connection
  socket parameters, and FastCGI/uWSGI/SCGI gateways.
- `src/server/stream/`: stream routing, access control, return, TCP/UDP
  forwarding, and logging.
- `src/server/tls/`: HTTP/stream TLS configuration, certificate loading, and
  client-certificate verification.
- `src/server/tls/ocsp/`: certificate issuer/AIA parsing, OCSP HTTP fetching,
  response validation, shared staple state, and the `nextUpdate` refresh loop;
  `src/server/resolver/` provides an A/AAAA DNS client with TTL caching.
- `src/runtime.rs`: Nginx-style CLI, daemon, PID, and signal commands.
- `src/logging.rs`: error log, access log, and time formatting.
- `src/logging/access.rs`: access-log file buffering, periodic flushing, and
  gzip output.
- `src/logging/file_cache.rs`: access-log file descriptor cache, LRU eviction,
  and rotation validation.

Modules are split by configuration context, request phase, or protocol
responsibility. Entry files handle orchestration and a small amount of glue;
new directive families, filters, or variable types should go into dedicated
submodules instead of growing a single file.
