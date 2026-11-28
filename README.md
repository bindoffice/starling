# Starling

[简体中文](README.md) | [English](README.en.md)

Starling 是一个使用 Rust 编写的 Nginx 兼容 Web 服务器与四层反向代理。
它直接读取 Nginx 风格配置，并保留 `nginx` 风格的命令行、PID、信号和
配置重载方式。

项目目标不是简单复刻接口，而是在 Rust/Tokio 异步运行时上实现一套可维护、
可验证、兼容常见 Nginx 配置的服务器核心，包括 HTTP/1.1、HTTP/2、HTTP/3、
TLS、静态文件、上游代理、缓存、压缩、FastCGI、uWSGI、SCGI、gRPC 和
TCP/UDP stream 代理。

> 当前版本为 `0.2.0`，仍处于持续开发阶段。Starling 不声称覆盖 Nginx
> 全部模块，生产使用前应使用实际配置执行 `-t` 并进行集成验证。

## 核心能力

- **Nginx 风格配置**：支持引号、变量、嵌套块、`include`、`map`、
  `geo`、`split_clients`、location、rewrite、if 和多级作用域继承。
- **现代 HTTP**：HTTP/1.0、HTTP/1.1、HTTP/2、HTTP/3/QUIC、TLS ALPN、
  keep-alive、chunked、trailer、PROXY protocol v1/v2 和
  `Expect: 100-continue`。
- **静态与内容处理**：静态文件、Range、autoindex、gzip/gunzip、
  `try_files`、`error_page`、SSI、响应替换、WebDAV 和用户 ID。
- **反向代理与网关**：HTTP/HTTPS upstream、gRPC、FastCGI、uWSGI、
  SCGI、Memcached、负载均衡、重试、连接复用和动态上游变量。
- **缓存与流式处理**：代理/CGI 磁盘缓存、stale/background update、
  请求和响应缓冲、临时文件、Range 和背压式流式正文转发。
- **访问与安全**：Basic Auth、子请求鉴权、allow/deny、限流、防盗链、
  URL 签名、客户端 TLS 证书、CRL 和 OCSP Stapling。
- **Stream 代理**：TCP、UDP、UNIX socket、TLS 终止、SNI 路由、
  PROXY protocol、连接限流和半关闭控制。
- **自动 HTTPS**：默认开启，自动生成本地开发证书；公网域名通过 ACME
  自动申请和续期，支持 HTTP-01 与通用 DNS-01 hook，通配符域名可使用
  DNS-01。

## 快速开始

### 构建

需要支持 Rust 2024 Edition 的 Rust 工具链。

```bash
cargo build --release
```

生成的可执行文件位于：

```text
target/release/starling
```

Linux 和 macOS 发布包使用 `vendored-openssl` 特性构建，将 OpenSSL
静态链接到可执行文件，避免目标机器缺少 OpenSSL 共享库时无法启动。Linux
发布包使用 `x86_64-unknown-linux-musl` 或 `aarch64-unknown-linux-musl`
目标，避免依赖构建机器的 glibc 版本：

```bash
rustup target add x86_64-unknown-linux-musl
sudo apt-get install musl-tools  # Debian/Ubuntu
CC_x86_64_unknown_linux_musl=musl-gcc \
CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
cargo build --release --locked --features vendored-openssl --target x86_64-unknown-linux-musl
scripts/package-release.sh --version 0.2.0 --target x86_64-unknown-linux-musl
```

运行测试：

```bash
cargo test --all-targets
```

### 最小配置

创建 `conf/nginx.conf`：

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

### 校验并启动

```bash
./target/release/starling -t -p "$PWD" -c conf/nginx.conf
./target/release/starling -p "$PWD" -c conf/nginx.conf -g 'daemon off;'
```

另一个终端访问：

```bash
curl http://127.0.0.1:8080/
```

### 启用自动 HTTPS

当 server 监听默认 `http_port=80` 时，可以省略 `auto_https` 指令。
Starling 会默认增加 HTTPS 监听，并为公网普通域名使用 HTTP-01：

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

需要公网通配符证书时使用 DNS-01：

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

完整的 DNS hook 契约、配置项和排障说明见
[docs/auto-https.md](docs/auto-https.md)。

## 模块概览

Starling 从配置加载到请求处理的主链路如下：

```text
main.rs
  +- runtime        CLI、配置装载、daemon/PID/信号
     +- config      Nginx 配置 -> AST -> RuntimeConfig
     +- logging     error log / access log
     +- server      监听、协议、请求生命周期与上游转发
        +- http        HTTP 报文、正文和 upstream 解析基础类型
        +- request     路由、访问控制、rewrite、请求体与连接管理
        +- response    静态文件、错误页、过滤器、Range 和响应头
        +- upstream    代理、缓存、FastCGI/uWSGI/SCGI、gRPC、重试
        +- stream      TCP/UDP/UNIX stream 代理
        +- tls/http2/http3/auto_https/variables/limits
```

| 模块 | 职责 |
| --- | --- |
| `src/runtime.rs` | Nginx 风格命令行、`-t` 配置检查、daemon、PID、信号和 reload 流程。 |
| `src/config/` | `parser/` 负责词法、语法和 include/glob；`compile/` 负责指令校验、作用域继承，并生成 `RuntimeConfig`。 |
| `src/http/` | 与运行时无关的 HTTP 基础类型，包括请求头/正文、响应体、缓冲/临时文件，以及 upstream HTTP 响应解析。 |
| `src/server/` | 异步服务器编排：监听 socket、连接接入、协议分派、请求生命周期、共享状态、优雅关闭和 reload 快照。 |
| `src/server/request/` | HTTP/1 连接循环、keep-alive、路由与 location 匹配、访问控制、rewrite 阶段、请求正文、镜像和内部重定向。 |
| `src/server/response/` | 静态文件、autoindex、错误页、条件请求、Range、gzip/gunzip、响应头/过滤器、WebDAV 和 userid。 |
| `src/server/upstream/` | HTTP/HTTPS 反向代理、连接池、重试与负载均衡、磁盘缓存、流式转发、FastCGI/uWSGI/SCGI、gRPC 和 Memcached。 |
| `src/server/stream/` | 四层 TCP/UDP/UNIX socket 代理、SNI 预读、PROXY protocol、TLS 终止、retry 和半关闭。 |
| `src/server/http2.rs`、`src/server/http3.rs` | HTTP/2 帧/流处理及 HTTP/3/QUIC 请求、正文和 trailer 映射。 |
| `src/server/tls/`、`src/server/auto_https/` | TLS 证书与客户端校验、SNI/ALPN、ACME、本地 CA 和证书续期。 |
| `src/server/variables/`、`src/server/limits.rs`、`src/server/resolver/` | Nginx 变量求值、map/geo 查询、连接限流、DNS 解析与缓存。 |
| `src/logging/` | error log 多目标写入，access log 格式、文件缓存、轮转重开、gzip 和 syslog。 |
| `src/auth.rs` | Basic Auth 解析及 `{PLAIN}`、`{SHA}`、`{SSHA}`、apr1、crypt 密码校验。 |
| `src/wasm/` | Wasmtime 引擎、模块缓存、资源限制和 core module ABI v1。 |

配置阶段与运行时阶段通过 `RuntimeConfig` 解耦：`runtime` 在启动或 reload 时
重新构建运行时快照，`server` 使用 `ArcSwap` 原子切换配置，进行中的请求继续
使用旧快照直到完成。生产代码按配置上下文、协议和请求阶段拆分；入口文件
只负责模块编排，新增指令族、过滤器或协议能力应进入对应子模块。

## 文档

- [FEATURES.md](FEATURES.md)：已实现功能、构建运行、配置示例、兼容性边界
  和完整代码结构说明。
- [docs/auto-https.md](docs/auto-https.md)：自动 HTTPS、本地 CA、HTTP-01、
  DNS-01 hook、证书续期和排障。
- [docs/wasm.md](docs/wasm.md)：WASM content handler 配置、资源限制、
  core module ABI v1 和最小示例。

## CI 与发布

仓库提供以下自动化入口：

- `.github/workflows/ci.yml`：在 push 到 `main`、Pull Request 和手动触发时运行
  `cargo fmt`、Clippy、完整测试，并构建一次发布包验证打包流程。
- `.github/workflows/release.yml`：推送 `v*` 标签或手动触发时，在 Linux、
  macOS 和 Windows runner 上构建 Linux/macOS 的 x86_64/aarch64 及 Windows
  x86_64 二进制，生成平台对应的 `tar.gz` 或 `zip` 发布包、`SHA256SUMS`，
  并创建或更新 GitHub Release。
- `scripts/ci.sh`：在本地执行与 CI 相同的格式、静态检查、测试和 release
  构建。
- `scripts/package-release.sh`：生成包含二进制、默认配置、静态页面、日志目录
  和文档的平台发布包。Windows 目标默认生成包含 `starling.exe` 的 `zip`，
  macOS/Linux 目标默认生成 `tar.gz`。

发布标签必须与 `Cargo.toml` 中的版本一致：

```bash
git tag v0.2.0
git push origin v0.2.0
```

本地打包示例：

```bash
cargo build --release --locked --features vendored-openssl
scripts/package-release.sh --version 0.2.0
```

也可以显式指定目标平台和归档格式：

```bash
scripts/package-release.sh \
  --version 0.2.0 \
  --target aarch64-apple-darwin \
  --archive-format tar.gz

scripts/package-release.sh \
  --version 0.2.0 \
  --target x86_64-pc-windows-msvc \
  --archive-format zip
```

## 兼容性说明

Starling 以 Nginx 1.31.9 的配置和运行行为作为主要参考。HTTP/3 已提供
`quinn` + `h3` 的初始实现，包括 `listen ... quic`、TLSv1.3/ALPN `h3`、
请求/响应映射、流式请求正文和响应 trailers；当前仍不支持 0-RTT、Alt-Svc
自动注入、HTTP/3 连接 reload 时热替换证书，以及多 worker UDP 分发。
其他明确边界包括动态模块、mail 模块、原生多 worker master 模型及部分
Nginx 高级模块细节。

配置中存在尚未实现的指令时，Starling 会尽量在配置检查阶段明确报错，
而不是静默忽略并产生难以定位的运行时行为。具体边界见
[FEATURES.md](FEATURES.md#兼容性边界)。

## HTTP 性能基线

以下结果均来自同宿主机 loopback 对照，不是跨机器或公网基准。测试环境为
AMD Ryzen 7 9700X（16 个逻辑 CPU）、Linux `7.0.0-44-generic`、16 GiB 内存。
除协议差异外，两边都关闭 access log，并服务同一组静态文件。Nginx 使用本机
源码构建的 `1.31.9 + OpenSSL 3.5.5`，同时启用 `--with-http_v2_module` 和
`--with-http_v3_module`。

### HTTP/1.1 与 HTTP/2（h2c）

本组使用明文 TCP，以隔离 HTTP 协议栈开销；HTTP/2 采用 prior-knowledge `h2c`。
HTTP/1.1 小响应使用 128 个连接，大响应使用 16 个连接，每组 3 轮、每轮 5 秒。
HTTP/2 小响应使用 64 个连接、每连接 16 个 stream、每轮 50 万请求；8 MiB 下载
使用 64 个连接、每连接 4 个 stream、每轮 1024 请求。HTTP/2 的 p50/p99 来自
`h2load` 单请求日志。HTTP/1.1 每组预热 1 秒，HTTP/2 预热轮使用每轮请求数的
10%，结果取 3 轮中位数。

| 场景 | 实现 | 请求吞吐 (req/s) | 传输吞吐 (GiB/s) | p50 (ms) | p99 (ms) | 失败 |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| HTTP/1.1 小响应 | Starling | 1,217,725.45 | - | 0.080 | 0.327 | 0 |
| HTTP/1.1 小响应 | Nginx 1.31.9 | 1,378,789.48 | - | 0.046 | 0.107 | 0 |
| HTTP/1.1 proxy small（keepalive） | Starling | 495,711.25 | - | 0.221 | 0.764 | 0 |
| HTTP/1.1 proxy small（keepalive） | Nginx 1.31.9 | 840,255.64 | - | 0.096 | 2.310 | 0 |
| HTTP/1.1 8 MiB 下载 | Starling | 6,771.99 | 52.91 | 1.650 | 5.880 | 0 |
| HTTP/1.1 8 MiB 下载 | Nginx 1.31.9 | 6,743.30 | 52.68 | 1.680 | 5.850 | 0 |
| HTTP/2 小响应 | Starling | 870,690.30 | - | 0.912 | 2.696 | 0 |
| HTTP/2 小响应 | Nginx 1.31.9 | 854,548.93 | - | 0.884 | 1.485 | 0 |
| HTTP/2 proxy small（keepalive） | Starling | 186,592.02 | - | 3.456 | 23.608 | 0 |
| HTTP/2 proxy small（keepalive） | Nginx 1.31.9 | 109,673.82 | - | 2.669 | 62.719 | 0 |
| HTTP/2 8 MiB 下载 | Starling | 5,788.97 | 45.25 | 32.708 | 84.690 | 0 |
| HTTP/2 8 MiB 下载 | Nginx 1.31.9 | 3,216.39 | 25.16 | 46.519 | 113.122 | 0 |

该路径在 Linux 明文 TCP 且 `sendfile on` 时由 h2 直接通过内核 `sendfile` 发送
DATA，并用 `MSG_MORE` + `TCP_CORK` 合并帧头与正文；TLS、UNIX socket、非 Linux
平台或 `sendfile off` 会自动回退到内存读取路径。H1/H2 与 H3 使用不同传输栈，
不能把某一协议的倍率直接外推到其他协议。

可复现命令：

```bash
scripts/bench-compare.sh
```

### HTTP/3/QUIC

Starling 使用 `quinn` + `h3`，Nginx 启用 `--with-http_v3_module`；两边均使用
TLSv1.3 和 ALPN `h3`。小响应为 5 字节静态文件，使用 8 个 QUIC 连接、每连接
16 个并发请求；大响应为 8 MiB 静态文件，使用 8 个 QUIC 连接、每连接 4 个并发
请求。每组先预热 1 秒，再独立运行 3 轮、每轮 5 秒，取中位数；每轮重新建立
客户端连接，避免把连接级保活限制混入吞吐结果。

| 场景 | 实现 | 请求吞吐 (req/s) | 传输吞吐 (GiB/s) | p50 (ms) | p99 (ms) | 失败 |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| HTTP/3 小响应 | Starling `quinn` + `h3` | 122,647 | - | 1.054 | 1.231 | 0 |
| HTTP/3 小响应 | Nginx 1.31.9 | 166,581 | - | 0.622 | 2.105 | 0 |
| HTTP/3 8 MiB 下载 | Starling `quinn` + `h3` | 190.82 | 1.491 | 167.483 | 171.169 | 0 |
| HTTP/3 8 MiB 下载 | Nginx 1.31.9 | 108.16 | 0.845 | 244.966 | 790.959 | 0 |

这组数据体现的是两个实现不同的路径特征：小响应下 Nginx 的单核请求吞吐约为
Starling 的 1.36 倍，但 Starling 的 p99 更低；8 MiB 下载下 Starling 的吞吐
约为 Nginx 的 1.76 倍，p99 也明显更低。结果不能外推为公网、多 worker 或
带丢包网络下的普遍结论。当前 Starling 的 QUIC endpoint 只接入第一个 worker
shard，因此本表刻意固定为单 worker；多 worker UDP 分发、0-RTT、Alt-Svc
自动注入和长时间连接复用仍需后续专项测试。

可复现的客户端位于 [examples/h3load.rs](examples/h3load.rs)。示例命令：

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

## 许可证

Starling 使用 Apache License 2.0 授权，完整条款见 [LICENSE](LICENSE)。
