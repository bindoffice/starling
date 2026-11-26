# Starling

Starling 是一个使用 Rust 编写的 Nginx 兼容 Web 服务器与四层反向代理。
它直接读取 Nginx 风格配置，并保留 `nginx` 风格的命令行、PID、信号和
配置重载方式。

项目目标不是简单复刻接口，而是在 Rust/Tokio 异步运行时上实现一套可维护、
可验证、兼容常见 Nginx 配置的服务器核心，包括 HTTP/1.1、HTTP/2、TLS、
静态文件、上游代理、缓存、压缩、FastCGI、uWSGI、SCGI、gRPC 和
TCP/UDP stream 代理。

> 当前版本为 `0.1.0`，仍处于持续开发阶段。Starling 不声称覆盖 Nginx
> 全部模块，生产使用前应使用实际配置执行 `-t` 并进行集成验证。

## 核心能力

- **Nginx 风格配置**：支持引号、变量、嵌套块、`include`、`map`、
  `geo`、`split_clients`、location、rewrite、if 和多级作用域继承。
- **现代 HTTP**：HTTP/1.0、HTTP/1.1、HTTP/2、TLS ALPN、keep-alive、
  chunked、trailer、PROXY protocol v1/v2 和 `Expect: 100-continue`。
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

## 兼容性说明

Starling 以 Nginx 1.31.9 的配置和运行行为作为主要参考。尚未支持或存在
明确边界的能力包括 HTTP/3/QUIC、动态模块、mail 模块、原生多 worker
master 模型及部分 Nginx 高级模块细节。

配置中存在尚未实现的指令时，Starling 会尽量在配置检查阶段明确报错，
而不是静默忽略并产生难以定位的运行时行为。具体边界见
[FEATURES.md](FEATURES.md#兼容性边界)。

## 许可证

BSD-2-Clause
