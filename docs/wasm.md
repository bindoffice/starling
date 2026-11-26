# WASM 内容处理器

[简体中文](wasm.md) | [English](wasm.en.md)

Starling 可以将 WebAssembly 模块作为 location 级内容处理器运行。宿主仍是
原生 Starling 进程；WASM 模块无法通过 WASI 访问 Starling 文件、套接字、
环境变量或 TLS 私钥。

当前实现支持：

- 在 core WASM 模块上运行带缓冲的请求/响应内容处理器。
- 使用 Wasmtime 进程内执行，每次调用使用一个 store 和一个模块实例。
- 进程级编译模块缓存，键由规范路径、大小、mtime 和源码摘要组成。
- 基于 epoch 的实际耗时中断，以及可选的 fuel 限制。
- 线性内存、请求正文、响应正文、头部和实例并发限制。
- 原子配置重载。进行中的请求会继续使用旧模块和配置快照，直到完成。

请求过滤器、响应过滤器、流式正文、component-model ABI 和进程隔离后端
尚未实现。这些未实现模式的配置指令会在执行 `starling -t` 时失败，而不
会静默回退。

进程内执行不是严格的多租户安全边界：它依赖 Wasmtime 自身隔离 guest
代码。在进程隔离后端可用之前，此模式只能使用经过审查的模块。

## 配置

必须显式在 `http` 上下文中启用 WASM：

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
            wasm_fuel 10000000;          # 可选；省略或设为 0 表示不限制 fuel
            wasm_request_body_max 1m;
            wasm_response_body_max 8m;
            wasm_max_instances 32;
            wasm_isolation in-process;
        }
    }
}
```

`wasm_module`、`wasm_phase` 和资源限制指令会被嵌套 location 继承，并可
在 server 或 location 作用域中覆盖。`wasm_engine` 和
`wasm_module_cache` 只能用于 `http` 作用域。

默认值如下：

| 指令 | 默认值 |
| --- | --- |
| `wasm_engine` | `off` |
| `wasm_module_cache` | `64m` |
| `wasm_phase` | `content` |
| `wasm_timeout` | `100ms` |
| `wasm_memory_max` | `64m` |
| `wasm_fuel` | 不限制 |
| `wasm_request_body_max` | `1m` |
| `wasm_response_body_max` | `8m` |
| `wasm_max_instances` | `32` |
| `wasm_isolation` | `in-process` |

`wasm_module` 与 `proxy_pass`、`grpc_pass`、FastCGI、uWSGI、SCGI、
Memcached、tunnel、`stub_status`、WebDAV、`empty_gif` 和 `flv` 内容处理器
冲突。加载配置时，Starling 会编译 `.wasm` 文件或文本格式的 `.wat` 模块，
并校验其 ABI 导出、内存形态、不存在 host imports，以及
`starling_abi_version` 的值。语法错误和不兼容模块会拒绝 `starling -t`
或重载，而不是等到请求时才失败。运行时在每次调用前还会再次检查版本。

## 运行时限制

- `wasm_timeout` 通过 Wasmtime epoch 中断 CPU 密集型 guest 代码，同时
  限制等待可用实例槽位的时间。
- `wasm_memory_max` 拒绝超过配置大小的线性内存增长。
- `wasm_fuel` 是可选的确定性指令预算，不能替代实际耗时超时。
- `wasm_request_body_max` 超限请求返回 `413`。`client_max_body_size`
  仍是独立上限，更严格的值生效。超过 32 位 ABI 上限（2 GiB）的值会在
  配置检查期间被拒绝。
- `wasm_response_body_max` 超限 guest 响应会由宿主返回 `500`。超过
  32 位 ABI 上限（2 GiB）的值会在配置检查期间被拒绝。
- `wasm_max_instances` 限制配置模块的并发实例数。
- trap、无效 ABI 响应、超时、内存限制失败或模块实例化失败都会返回可
  预测的 `500`，并且绝不会终止 worker 进程。

## Core module ABI v1

模块必须导出：

```text
memory
starling_abi_version() -> i32          # 返回 1
starling_alloc(size: i32) -> i32       # 返回 guest 指针
starling_handle(ptr: i32, len: i32) -> i64
```

宿主通过 `starling_alloc` 写入一个请求缓冲区，然后调用
`starling_handle`。返回值打包响应指针和长度：

```text
pointer = (result as u64) >> 32
length  = result as u32
```

所有整数字段均为小端序。字符串为 UTF-8，并使用下文所示长度宽度。

### 请求缓冲区

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

### 响应缓冲区

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

响应分帧由宿主负责，因此宿主会拒绝 guest 返回的 `Connection`、
`Content-Length`、`Keep-Alive`、`Proxy-Connection`、`TE`、`Trailer`、
`Transfer-Encoding` 和 `Upgrade` 头部。头部名称必须是 token，值不能包含
除水平制表符之外的控制字符，头部数据总量限制为 64 KiB，状态码 `204`、
`205` 和 `304` 不能携带正文。对于 `205` 响应，宿主会生成
`Content-Length: 0`；对于 `204` 和 `304`，宿主会省略 `Content-Length`。

## 最小示例

[`examples/wasm/hello.wat`](../examples/wasm/hello.wat) 是一个可执行的
core module，返回 `200`、`Content-Type: text/plain`、`X-Wasm: hello` 和
`hello from wasm\n`。

使用方式：

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

该模块忽略请求正文并使用静态响应缓冲区。生产模块通常应实现带容量检查
的 bump allocator，只解析需要的字段，并在返回打包的指针/长度之前，在
guest 内存中构造响应。
