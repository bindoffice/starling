# 自动 HTTPS 配置指南

[简体中文](auto-https.md) | [English](auto-https.en.md)

本文档说明 Starling 自动 HTTPS 的配置方式，包括 HTTP-01、DNS-01、
本地开发证书、ACME 测试目录、证书续期和常见问题。

## 1. 功能概述

自动 HTTPS 默认开启。服务器启动或重新加载配置时会执行以下工作：

1. 根据 `server_name` 和 `listen` 推导需要覆盖的域名。
2. 为监听 HTTP 端口的 server 自动增加对应 HTTPS 监听。
3. 在证书尚未签发时生成本地占位证书，保证 HTTPS 端口能够启动。
4. 对公网域名通过 ACME v2 自动申请证书。
5. 对 `localhost`、IP 和本地开发域名生成本地 CA 签发的证书。
6. 证书更新后以原子方式写入磁盘，并热更新 TLS 配置。
7. 后台定期检查证书，在到期前自动续期。

默认配置不需要显式写出 `auto_https on`。下面的配置已经会为
`your-domain.com` 和 `www.your-domain.com` 自动启用 HTTPS：

```nginx
events {}

http {
    server {
        listen 80;
        server_name your-domain.com www.your-domain.com;

        location / {
            return 200 "ok\n";
        }
    }
}
```

服务器会自动增加 `443` 端口监听，并在证书签发前使用临时证书。
如果正在使用 public ACME，首次启动后访问 HTTP 仍可能收到 308
跳转到 HTTPS，但浏览器在证书签发完成前可能显示证书不受信任。

## 2. 配置语法

自动 HTTPS 指令位于配置文件的 `main` 上下文，不能写在
`http`、`server` 或 `location` 块内。

可以使用聚合形式：

```nginx
auto_https on
    cert_dir=auto_https
    email=admin@your-domain.com
    staging=off
    challenge=http-01
    redirect=on
    http_port=80
    https_port=443;
```

也可以使用独立指令：

```nginx
auto_https on;
auto_https_cert_dir auto_https;
auto_https_email admin@your-domain.com;
auto_https_staging off;
auto_https_challenge http-01;
auto_https_redirect on;
auto_https_http_port 80;
auto_https_https_port 443;
```

`auto_https off` 会完全关闭自动证书、HTTP-01 响应和自动 HTTPS 跳转：

```nginx
auto_https off;
```

## 3. 配置项

| 配置项 | 默认值 | 说明 |
| --- | --- | --- |
| `on` / `off` | `on` | 是否启用自动 HTTPS |
| `cert_dir=` | `auto_https` | 证书、账户密钥和本地 CA 的保存目录 |
| `email=` | 未设置 | ACME 账户联系邮箱，通常用于 Let's Encrypt |
| `staging=` | `off` | 使用 Let's Encrypt staging 目录 |
| `acme_directory=` | 由 `staging` 决定 | 自定义 ACME Directory URL |
| `acme_ca_file=` | 未设置 | 校验 ACME HTTPS 服务端时使用的额外 CA 文件 |
| `challenge=` | `http-01` | ACME 校验方式：`http-01` 或 `dns-01` |
| `dns_hook=` | 未设置 | DNS-01 使用的可执行 hook |
| `dns_propagation_delay=` | `30s` | 发布全部 TXT 记录后等待的时间 |
| `redirect=` | `on` | 是否把 HTTP 请求 308 跳转到 HTTPS |
| `http_port=` | `80` | 自动 HTTPS 识别的 HTTP 端口 |
| `https_port=` | `443` | 自动生成的 HTTPS 端口 |

独立指令名称分别是：

```text
auto_https_cert_dir
auto_https_email
auto_https_staging
auto_https_acme_directory
auto_https_acme_ca_file
auto_https_challenge
auto_https_dns_hook
auto_https_dns_propagation_delay
auto_https_redirect
auto_https_http_port
auto_https_https_port
```

时间值支持 `ms`、`s`、`m`、`h`、`d`、`w`、`M`、`y`。例如：
`500ms`、`30s`、`5m`、`1h`。

启用自动 HTTPS 时，`http_port` 和 `https_port` 必须不同。
端口值必须在 `1..=65535` 范围内。

### 相对路径

`cert_dir`、`acme_ca_file` 和 `dns_hook` 的相对路径都相对于
Starling 的配置前缀目录，而不是当前工作目录。

## 4. HTTP-01

HTTP-01 是默认校验方式，适用于普通公网域名，但不支持通配符域名。

```nginx
events {}

http {
    server {
        listen 80;
        server_name your-domain.com www.your-domain.com;

        location / {
            return 200 "hello\n";
        }
    }
}
```

等价的显式配置：

```nginx
auto_https on challenge=http-01 redirect=on;

events {}

http {
    server {
        listen 80;
        server_name your-domain.com www.your-domain.com;
    }
}
```

申请证书时，Starling 会：

1. 在内存中保存 ACME HTTP-01 token 对应的响应。
2. 在以下路径提供校验内容：

   ```text
   /.well-known/acme-challenge/<token>
   ```

3. 触发 ACME 校验并轮询 authorization 状态。
4. 下载证书并热加载。

HTTP-01 校验响应优先于 HTTPS 跳转处理，因此即使 `redirect=on`，
ACME 校验请求仍然可以正常返回。

### HTTP-01 前提

- `server_name` 中的域名必须能够被 ACME 服务商解析。
- 公网 CA 的 HTTP-01 校验固定从公网访问目标域名的 TCP 80 端口。
- 域名必须指向当前服务器，且防火墙、负载均衡和安全组允许访问。
- 如果公网 80 端口通过 NAT 映射到内部其他端口，可以设置
  `http_port=` 为实际监听端口。
- 域名不能是 `*.your-domain.com` 这种通配符。
- CAA 记录必须允许所选 ACME CA。

## 5. DNS-01

DNS-01 通过修改 `_acme-challenge` TXT 记录完成校验，并支持公网通配符：

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

        location / {
            return 200 "hello\n";
        }
    }
}
```

当 `challenge=dns-01` 但没有配置 `dns_hook` 时，配置检查会失败。

DNS-01 的处理顺序如下：

1. 获取订单中的全部 pending authorization。
2. 为每个 authorization 计算 TXT 记录名和值。
3. 逐条调用 hook 的 `present` 动作。
4. 所有记录发布成功后，等待 `dns_propagation_delay`。
5. 触发全部 ACME challenge。
6. 逐个轮询 authorization 状态。
7. 全部处理结束后，统一调用 hook 的 `cleanup` 动作。

同一订单同时包含 `your-domain.com` 和 `*.your-domain.com` 时，两者会使用
相同的 TXT 记录名 `_acme-challenge.your-domain.com`，但值不同。Starling
会保留全部值直到所有 authorization 验证结束，不会完成一个就提前清理。

DNS hook 必须支持同一记录名追加多个 TXT 值，并在 `cleanup` 时只删除
与给定 value 对应的记录，不能直接覆盖或删除整个记录集合。

### TXT 记录格式

对于 authorization identifier `your-domain.com`：

```text
记录类型: TXT
记录名:   _acme-challenge.your-domain.com
记录值:   STARLING_ACME_DNS_VALUE
```

通配符 `*.your-domain.com` 同样使用：

```text
_acme-challenge.your-domain.com
```

Starling 已经计算好 TXT 值，hook 不需要自行执行 SHA-256 或
base64url 编码。

### Hook 执行契约

`dns_hook` 直接执行一个可执行文件，不经过 shell，不附加命令行参数。
相对路径相对于配置前缀目录。

每次调用会提供以下环境变量：

| 环境变量 | 说明 |
| --- | --- |
| `STARLING_ACME_DNS_ACTION` | `present` 或 `cleanup` |
| `STARLING_ACME_DNS_NAME` | TXT 记录名，例如 `_acme-challenge.your-domain.com` |
| `STARLING_ACME_DNS_VALUE` | 要写入或删除的 TXT 值 |
| `STARLING_ACME_DNS_DOMAIN` | 当前 authorization 对应的证书域名，通配符保留 `*.` |

同样的数据也会以 JSON 写入 hook 的 stdin：

```json
{
  "action": "present",
  "name": "_acme-challenge.your-domain.com",
  "value": "base64url-sha256-value",
  "domain": "*.your-domain.com"
}
```

Hook 的约定：

- 返回退出码 `0` 表示成功。
- `present` 返回非零会中止当前证书申请，并清理本次已尝试发布的记录。
- `cleanup` 失败不会覆盖原始 ACME 成功或失败结果，只会记录告警。
- 单次执行超时时间为 60 秒。
- stdout 和 stderr 各限制为 64 KiB。
- stdout/stderr 主要会出现在 hook 失败日志中。
- hook 可以只读取环境变量，也可以同时读取 stdin JSON。

### Hook 骨架

下面是一个 provider 无关的执行骨架：

```sh
#!/bin/sh
set -eu

action=$STARLING_ACME_DNS_ACTION
name=$STARLING_ACME_DNS_NAME
value=$STARLING_ACME_DNS_VALUE
domain=$STARLING_ACME_DNS_DOMAIN

case "$action" in
    present)
        # 调用 DNS provider API，为 name 追加 value 对应的 TXT 记录。
        ;;
    cleanup)
        # 调用 DNS provider API，只删除 name 下与 value 匹配的 TXT 记录。
        ;;
    *)
        echo "unknown action: $action" >&2
        exit 1
        ;;
esac
```

部署后必须赋予执行权限：

```sh
chmod 755 /usr/local/libexec/starling-dns-hook
```

如果 hook 需要 API token，建议通过 Starling 进程环境、权限受限的
凭据文件或系统密钥服务提供，不要把长期凭据写入 world-readable 配置。

### 验证 hook 是否被调用

在接入 provider API 前，可以先用下面的脚本验证环境：

```sh
#!/bin/sh
set -eu

printf '%s|%s|%s|%s\n' \
    "$STARLING_ACME_DNS_ACTION" \
    "$STARLING_ACME_DNS_NAME" \
    "$STARLING_ACME_DNS_VALUE" \
    "$STARLING_ACME_DNS_DOMAIN" \
    >> /tmp/starling-dns-hook.log
```

该脚本只记录调用，不会真正创建 TXT 记录，因此不能用于申请正式证书。

## 6. 本地开发证书

以下名称会被归类为本地开发名称，不会请求公网 ACME：

- `localhost`
- `*.localhost`
- `localhost.localdomain`
- `*.localhost.localdomain`
- `*.local`
- `localdomain` 和 `*.localdomain`
- `*.test`
- `*.invalid`
- `*.example`
- `internal` 和 `*.internal`
- `home.arpa` 和 `*.home.arpa`
- `example.com`、`example.net`、`example.org` 及其子域名
- IP 地址
- 不包含点号的非 FQDN

示例：

```nginx
auto_https_http_port 8080;
auto_https_https_port 8443;

events {}

http {
    server {
        listen 127.0.0.1:8080;
        server_name localhost dev.local;

        location / {
            return 200 "local https\n";
        }
    }
}
```

本地 CA 默认位于：

```text
auto_https/local-ca.crt
```

本地 CA 私钥位于：

```text
auto_https/local-ca.key
```

将 `local-ca.crt` 导入系统或浏览器信任库后，本地 HTTPS 不再告警。

### Linux

Debian/Ubuntu 示例：

```sh
sudo cp auto_https/local-ca.crt \
    /usr/local/share/ca-certificates/starling-local-ca.crt
sudo update-ca-certificates
```

### macOS

```sh
sudo security add-trusted-cert \
    -d -r trustRoot \
    -k /Library/Keychains/System.keychain \
    auto_https/local-ca.crt
```

### Windows

在管理员命令提示符中执行：

```bat
certutil -addstore -f Root auto_https\local-ca.crt
```

Firefox 等使用独立证书库的浏览器可能还需要在浏览器设置中单独导入。

本地 CA 私钥能够签发被信任的证书，必须限制访问权限。Unix 系统上
Starling 会尝试将证书目录权限设置为 `0700`，私钥文件设置为 `0600`。

## 7. 使用测试 ACME 环境

Let's Encrypt staging 可以避免正式环境速率限制：

```nginx
auto_https on
    email=admin@your-domain.com
    staging=on;
```

Staging 证书不会被公共浏览器信任，只用于验证申请流程。

也可以显式指定 ACME Directory：

```nginx
auto_https on
    email=admin@your-domain.com
    acme_directory=https://acme-staging-v02.api.letsencrypt.org/directory;
```

使用 Pebble 或其他私有 ACME 服务时，可以同时指定其 CA：

```nginx
auto_https on
    email=admin@your-domain.com
    acme_directory=http://127.0.0.1:14000/dir
    acme_ca_file=pebble-ca.crt
    challenge=http-01;
```

`acme_ca_file` 用于校验 HTTPS ACME 服务端证书，不会作为最终站点证书
直接提供给客户端。即使 ACME Directory 是 `http://`，该配置项也可以保留。

## 8. 自定义监听端口

测试或容器环境中可以使用非特权端口：

```nginx
auto_https_http_port 8080;
auto_https_https_port 8443;
auto_https_redirect on;

events {}

http {
    server {
        listen 127.0.0.1:8080;
        server_name localhost;
    }
}
```

HTTP 请求会跳转到：

```text
https://localhost:8443/...
```

公网 ACME HTTP-01 校验仍由 CA 访问目标域名的公网 80 端口。
`http_port=8080` 只在外部 80 端口被映射到内部 8080 时才能配合
HTTP-01 使用。

DNS-01 不依赖入站 80 端口，但出于 HTTP 到 HTTPS 跳转和普通访问需要，
通常仍会配置 HTTP 监听。

## 9. 证书申请范围

Starling 会从每个 server 可推导的 `server_name` 生成一张覆盖全部名称
的证书。

- 精确名称会进入证书 SAN。
- 前导通配符 `*.your-domain.com` 会作为通配符 SAN；只有 `dns-01` 支持。
- 正则 `server_name` 不会进入证书 SAN。
- 已经有显式 `ssl_certificate` 和 `ssl_certificate_key` 的 server
  不会被自动证书覆盖，但仍会应用自动 HTTPS 监听逻辑。
- `ssl_certificate` 与 `ssl_certificate_key` 必须成对配置。
- 同一个 server 同时包含本地名称和公网名称时，会跳过自动 ACME 并记录
  告警，因为一张公网证书不应覆盖本地开发名称。

如果需要同时使用本地与公网名称，建议拆分为不同 server 或显式提供证书。

## 10. HTTP 到 HTTPS 跳转

默认在满足以下条件时返回 308：

- 请求进入非 TLS 的 `http_port` 监听。
- 当前 server 存在对应 `https_port` 的 TLS 监听。
- `redirect=on`。
- Host 尚未被其他规则拒绝。

关闭跳转但保留自动 HTTPS：

```nginx
auto_https on redirect=off;
```

HTTP-01 challenge 路径不会因为 `redirect=off` 而失效；在 HTTP-01
申请期间，它仍然由自动 HTTPS 模块提供。

## 11. 证书目录与续期

默认目录结构：

```text
auto_https/
  local-ca.crt
  local-ca.key
  acme-account.key
  certs/
    <name-set-hash>.crt
    <name-set-hash>.key
    <name-set-hash>.source
  .auto-https.lock
```

- `acme-account.key` 是 ACME 账户私钥。
- 证书和私钥以原子替换方式更新。
- `certs/*.source` 记录证书来源，例如 ACME 或本地占位证书。
- `.auto-https.lock` 和每张证书对应的锁用于避免启动、续期和多进程
  同时申请。

当前默认续期策略：

- 后台检查周期为 12 小时。
- 证书剩余有效期不足 30 天时进入续期流程。
- 请求失败后从 5 分钟开始退避，逐次翻倍，最长 1 小时。
- 成功续期后保留旧证书服务能力并热替换 rustls 配置。

这些续期参数目前是内部默认值，没有对应的配置指令。

不要在不了解后果的情况下删除 `acme-account.key`。删除后会创建新账户，
已有订单和账户侧状态不能继续复用，也可能触发额外的 ACME 速率限制。

## 12. 完整示例

### 公网普通域名：HTTP-01

```nginx
auto_https on
    email=admin@your-domain.com
    staging=off
    challenge=http-01
    redirect=on;

events {
    worker_connections 1024;
}

http {
    server {
        listen 80 default_server;
        server_name your-domain.com www.your-domain.com;

        location / {
            return 200 "https ready\n";
        }
    }
}
```

### 公网通配符：DNS-01

```nginx
auto_https on
    email=admin@your-domain.com
    challenge=dns-01
    dns_hook=/usr/local/libexec/starling-dns-hook
    dns_propagation_delay=60s
    redirect=on;

events {
    worker_connections 1024;
}

http {
    server {
        listen 80 default_server;
        server_name your-domain.com *.your-domain.com;

        location / {
            return 200 "wildcard https ready\n";
        }
    }
}
```

### 本地开发

```nginx
auto_https_http_port 8080;
auto_https_https_port 8443;

events {
    worker_connections 256;
}

http {
    server {
        listen 127.0.0.1:8080;
        server_name localhost dev.local;

        location / {
            return 200 "local https\n";
        }
    }
}
```

## 13. 检查与排障

首先检查配置语法：

```sh
starling -t -c /path/to/nginx.conf
```

查看错误日志：

```sh
tail -f /path/to/logs/error.log
```

常见问题：

### 没有生成公网证书

- 确认域名被归类为公网域名，而不是 `localhost`、IP 或 `.test` 等本地名称。
- 确认 server 监听了 `http_port` 或 `https_port`。
- 确认没有显式配置 `ssl_certificate` 来接管证书。
- 检查 server 是否混用了本地和公网名称。
- 检查 ACME Directory 和出站 HTTPS/HTTP 网络是否可达。

### HTTP-01 失败

- 确认公网 DNS 的 A/AAAA 记录正确。
- 确认公网 TCP 80 可达。
- 确认没有代理服务器拦截 `/.well-known/acme-challenge/`。
- 确认 CDN、WAF 或负载均衡会转发 challenge 路径。
- 检查 CAA 记录。
- 如果使用 staging 或 Pebble，确认校验服务能够访问当前实例。

### DNS-01 失败

- 确认 `dns_hook` 存在且具有执行权限。
- 手动执行 hook，检查 provider 凭据和 API 权限。
- 检查错误日志中的 hook stderr。
- 确认 provider 支持同一记录名下多个 TXT 值。
- 检查 `_acme-challenge` 记录是否被静态记录或 CNAME 错误覆盖。
- 提高 `dns_propagation_delay`，例如从 `30s` 调整为 `60s` 或 `120s`。
- 使用 DNS 查询工具确认多个权威 DNS 节点已经返回 TXT 值。

### 浏览器不信任本地证书

- 确认导入的是 `auto_https/local-ca.crt`，不是站点叶证书。
- 确认浏览器或操作系统实际信任了该 CA。
- Firefox 可能需要单独导入。
- 本地 CA 重建后需要重新导入。

### 跳转地址端口不正确

检查 `auto_https_https_port`。当 HTTPS 端口不是 443 时，跳转 Location
会显式包含端口。

### 配置报错 `challenge=dns-01 requires a dns_hook`

DNS-01 必须配置 hook：

```nginx
auto_https on challenge=dns-01 dns_hook=/path/to/hook;
```

## 14. 当前限制

- 不内置任何特定 DNS provider SDK，DNS-01 统一通过外部 hook 接入。
- 当前实现支持 HTTP-01 和 DNS-01，不支持 TLS-ALPN-01。
- HTTP-01 不支持通配符域名。
- 正则 `server_name` 不参与自动证书域名生成。
- 自动 HTTPS 指令只支持 main 上下文，不支持按 server 单独覆盖。
- 本地与公网名称混在同一个 server 时会跳过 ACME。
- 自动证书基于 server 的全部可推导域名生成，不支持任意拆分 SAN 子集。
