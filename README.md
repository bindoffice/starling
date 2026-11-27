# Starling

[简体中文](README.md) | [English](README.en.md)

Starling 是一个使用 Rust 编写的 Nginx 兼容 Web 服务器与四层反向代理。
它直接读取 Nginx 风格配置，并保留 `nginx` 风格的命令行、PID、信号和
配置重载方式。

项目目标不是简单复刻接口，而是在 Rust/Tokio 异步运行时上实现一套可维护、
可验证、兼容常见 Nginx 配置的服务器核心，包括 HTTP/1.1、HTTP/2、HTTP/3、
TLS、静态文件、上游代理、缓存、压缩、FastCGI、uWSGI、SCGI、gRPC 和
TCP/UDP stream 代理。

> 当前版本为 `0.1.3`，仍处于持续开发阶段。Starling 不声称覆盖 Nginx
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
scripts/package-release.sh --version 0.1.3 --target x86_64-unknown-linux-musl
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

## 项目结构

```text
src/config/        Nginx 配置词法、语法、include 和运行时模型编译
src/http/          HTTP 请求、响应和 upstream 协议
src/server/        HTTP/TLS/stream 运行时、路由、代理、缓存和自动 HTTPS
src/logging.rs     日志系统和日志文件缓存
src/runtime.rs     Nginx 风格 CLI、daemon、PID 和信号处理
tests/             配置兼容、服务器行为和运行时迁移测试
```

生产代码按配置阶段、协议和请求生命周期拆分为独立子模块。入口文件主要
负责模块编排，复杂指令族和运行时能力应继续进入对应子模块。

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
git tag v0.1.3
git push origin v0.1.3
```

本地打包示例：

```bash
cargo build --release --locked --features vendored-openssl
scripts/package-release.sh --version 0.1.3
```

也可以显式指定目标平台和归档格式：

```bash
scripts/package-release.sh \
  --version 0.1.3 \
  --target aarch64-apple-darwin \
  --archive-format tar.gz

scripts/package-release.sh \
  --version 0.1.3 \
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
AMD Ryzen 7 9700X（16 个逻辑 CPU）、Linux `7.0.0-43-generic`、16 GiB 内存。
除协议差异外，两边都使用 `worker_processes 1`、关闭 access log，并服务同一组
静态文件。Nginx 使用本机源码构建的 `1.31.9 + OpenSSL 3.5.5`，同时启用
`--with-http_v2_module` 和 `--with-http_v3_module`。

### HTTP/1.1 与 HTTP/2（h2c）

本组使用明文 TCP，以隔离 HTTP 协议栈开销；HTTP/2 采用 prior-knowledge `h2c`。
HTTP/1.1 小响应使用 128 个连接，大响应使用 32 个连接，每组 3 轮、每轮 5 秒。
HTTP/2 小响应使用 8 个连接、每连接 16 个 stream、每轮 100 万请求；8 MiB 下载
使用 8 个连接、每连接 4 个 stream、每轮 1024 请求。HTTP/2 的 p50/p99 来自
`h2load` 单请求日志。HTTP/1.1 每组预热 1 秒，HTTP/2 预热轮使用每轮请求数的
10%，结果取 3 轮中位数。

| 场景 | 实现 | 请求吞吐 (req/s) | 传输吞吐 (GiB/s) | p50 (ms) | p99 (ms) | 失败 |
| :--- | :--- | ---: | ---: | ---: | ---: | ---: |
| HTTP/1.1 小响应 | Starling | 195,061 | - | 0.657 | 0.675 | 0 |
| HTTP/1.1 小响应 | Nginx 1.31.9 | 272,925 | - | 0.469 | 0.484 | 0 |
| HTTP/1.1 8 MiB 下载 | Starling | 4,127.64 | 32.26 | 7.800 | 15.950 | 0 |
| HTTP/1.1 8 MiB 下载 | Nginx 1.31.9 | 4,165.32 | 32.57 | 7.700 | 13.330 | 0 |
| HTTP/2 小响应 | Starling | 136,059 | - | 0.902 | 1.145 | 0 |
| HTTP/2 小响应 | Nginx 1.31.9 | 223,423 | - | 0.568 | 1.110 | 0 |
| HTTP/2 8 MiB 下载 | Starling | 1,448.15 | 10.54 | 22.300 | 45.378 | 0 |
| HTTP/2 8 MiB 下载 | Nginx 1.31.9 | 1,050.55 | 8.220 | 28.444 | 36.162 | 0 |

HTTP/1.1 小响应下 Starling 达到 Nginx 的 71.5%，约落后 1.40 倍；HTTP/2 小响应
达到 60.9%，约落后 1.64 倍。8 MiB HTTP/1.1 下载的吞吐已基本持平（99.1%），但
Starling p99 高约 19.7%。HTTP/2 8 MiB 下载优化后达到 Nginx 的 137.9%，约为
优化前 203.11 req/s 的 7.13 倍；p99 仍比 Nginx 高约 25.5%，长尾还有继续优化
空间。该路径在 Linux 明文 TCP 且 `sendfile on` 时由 h2 直接通过内核 `sendfile`
发送 DATA，并用 `MSG_MORE` + `TCP_CORK` 合并帧头与正文；TLS、UNIX socket、
非 Linux 平台或 `sendfile off` 会自动回退到内存读取路径。H1/H2 与 H3 使用不同
传输栈，不能把某一协议的倍率直接外推到其他协议。

可复现命令：

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
