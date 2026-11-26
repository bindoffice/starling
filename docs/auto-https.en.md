# Automatic HTTPS Configuration Guide

[简体中文](auto-https.md) | [English](auto-https.en.md)

This guide explains how to configure Starling automatic HTTPS, including
HTTP-01, DNS-01, local development certificates, the ACME staging directory,
certificate renewal, and common troubleshooting steps.

## 1. Overview

Automatic HTTPS is enabled by default. When Starling starts or reloads its
configuration, it performs the following work:

1. Derive the domains that need coverage from `server_name` and `listen`.
2. Automatically add matching HTTPS listeners for servers listening on the
   HTTP port.
3. Generate a local placeholder certificate when a certificate has not yet
   been issued, so the HTTPS port can start.
4. Automatically request certificates for public domains through ACME v2.
5. Generate certificates signed by a local CA for `localhost`, IP addresses,
   and local development domains.
6. Atomically write renewed certificates to disk and hot-reload the TLS
   configuration.
7. Periodically check certificates in the background and renew them before
   expiration.

You do not need to write `auto_https on` in the default configuration. The
following configuration already enables HTTPS automatically for
`your-domain.com` and `www.your-domain.com`:

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

Starling automatically adds a `443` listener and uses a temporary certificate
until a certificate is issued. When using public ACME for the first time, an
HTTP request may still receive a 308 redirect to HTTPS, but the browser may
show an untrusted certificate until issuance completes.

## 2. Configuration Syntax

Automatic HTTPS directives belong in the `main` context. They cannot be
written inside `http`, `server`, or `location` blocks.

The aggregate form is:

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

You can also use individual directives:

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

`auto_https off` completely disables automatic certificates, HTTP-01
responses, and automatic HTTPS redirects:

```nginx
auto_https off;
```

## 3. Configuration Options

| Option | Default | Description |
| --- | --- | --- |
| `on` / `off` | `on` | Whether automatic HTTPS is enabled |
| `cert_dir=` | `auto_https` | Directory for certificates, account keys, and the local CA |
| `email=` | unset | ACME account contact email, usually used for Let's Encrypt |
| `staging=` | `off` | Use the Let's Encrypt staging directory |
| `acme_directory=` | determined by `staging` | Custom ACME Directory URL |
| `acme_ca_file=` | unset | Additional CA file used to verify the ACME HTTPS server |
| `challenge=` | `http-01` | ACME validation method: `http-01` or `dns-01` |
| `dns_hook=` | unset | Executable hook used for DNS-01 |
| `dns_propagation_delay=` | `30s` | Time to wait after publishing all TXT records |
| `redirect=` | `on` | Whether to redirect HTTP requests to HTTPS with 308 |
| `http_port=` | `80` | HTTP port recognized by automatic HTTPS |
| `https_port=` | `443` | Automatically generated HTTPS port |

The corresponding individual directives are:

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

Time values support `ms`, `s`, `m`, `h`, `d`, `w`, `M`, and `y`, for example
`500ms`, `30s`, `5m`, and `1h`.

When automatic HTTPS is enabled, `http_port` and `https_port` must be
different. Port values must be in the `1..=65535` range.

### Relative paths

Relative paths for `cert_dir`, `acme_ca_file`, and `dns_hook` are resolved
against Starling's configuration prefix directory, not the current working
directory.

## 4. HTTP-01

HTTP-01 is the default validation method. It works for ordinary public domains
but does not support wildcard domains.

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

The equivalent explicit configuration is:

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

When requesting a certificate, Starling:

1. Stores the ACME HTTP-01 token response in memory.
2. Serves the validation content at:

   ```text
   /.well-known/acme-challenge/<token>
   ```

3. Triggers ACME validation and polls the authorization status.
4. Downloads the certificate and hot-reloads it.

HTTP-01 validation responses take precedence over HTTPS redirects, so ACME
validation requests still return normally even when `redirect=on`.

### HTTP-01 prerequisites

- Domains in `server_name` must be resolvable by the ACME provider.
- A public CA's HTTP-01 validation always connects to TCP port 80 on the
  target domain from the public internet.
- The domain must point to the current server, and firewalls, load balancers,
  and security groups must allow access.
- If public port 80 is NAT-mapped to a different internal port, set
  `http_port=` to the actual listening port.
- The domain cannot be a wildcard such as `*.your-domain.com`.
- CAA records must allow the selected ACME CA.

## 5. DNS-01

DNS-01 validates by modifying `_acme-challenge` TXT records and supports
public wildcard domains:

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

Configuration validation fails when `challenge=dns-01` is set without a
`dns_hook`.

DNS-01 processing follows this sequence:

1. Retrieve all pending authorizations in the order.
2. Compute the TXT record name and value for every authorization.
3. Call the hook's `present` action for each record.
4. Wait for `dns_propagation_delay` after all records are published.
5. Trigger all ACME challenges.
6. Poll each authorization status.
7. After all processing finishes, call the hook's `cleanup` action.

When one order contains both `your-domain.com` and `*.your-domain.com`, both
use the same TXT record name, `_acme-challenge.your-domain.com`, but with
different values. Starling keeps all values until every authorization has
been validated and does not clean up one value early after the first
authorization completes.

A DNS hook must support appending multiple TXT values for the same record name
and must remove only the record matching the given value during `cleanup`. It
must not overwrite or delete the entire record set.

### TXT record format

For an authorization identifier of `your-domain.com`:

```text
Record type: TXT
Record name: _acme-challenge.your-domain.com
Record value: STARLING_ACME_DNS_VALUE
```

The wildcard `*.your-domain.com` uses the same name:

```text
_acme-challenge.your-domain.com
```

Starling has already computed the TXT value; the hook does not need to perform
SHA-256 or base64url encoding itself.

### Hook execution contract

`dns_hook` directly executes an executable file without a shell and without
command-line arguments. Relative paths are resolved against the configuration
prefix directory.

Each invocation provides these environment variables:

| Variable | Description |
| --- | --- |
| `STARLING_ACME_DNS_ACTION` | `present` or `cleanup` |
| `STARLING_ACME_DNS_NAME` | TXT record name, for example `_acme-challenge.your-domain.com` |
| `STARLING_ACME_DNS_VALUE` | TXT value to add or remove |
| `STARLING_ACME_DNS_DOMAIN` | Certificate domain for the current authorization; wildcards retain `*.` |

The same data is also written as JSON to the hook's stdin:

```json
{
  "action": "present",
  "name": "_acme-challenge.your-domain.com",
  "value": "base64url-sha256-value",
  "domain": "*.your-domain.com"
}
```

Hook conventions:

- Exit code `0` means success.
- A non-zero `present` exit aborts the current certificate request and cleans
  up records published during this attempt.
- A failed `cleanup` does not overwrite the original ACME success or failure
  result; it only logs a warning.
- Each invocation times out after 60 seconds.
- stdout and stderr are each limited to 64 KiB.
- stdout/stderr is primarily included in hook failure logs.
- The hook may read only environment variables or also read the stdin JSON.

### Hook skeleton

The following provider-independent skeleton shows the execution contract:

```sh
#!/bin/sh
set -eu

action=$STARLING_ACME_DNS_ACTION
name=$STARLING_ACME_DNS_NAME
value=$STARLING_ACME_DNS_VALUE
domain=$STARLING_ACME_DNS_DOMAIN

case "$action" in
    present)
        # Call the DNS provider API to append the TXT record for name/value.
        ;;
    cleanup)
        # Call the DNS provider API to remove only the matching TXT value.
        ;;
    *)
        echo "unknown action: $action" >&2
        exit 1
        ;;
esac
```

The executable permission must be set after deployment:

```sh
chmod 755 /usr/local/libexec/starling-dns-hook
```

If the hook needs an API token, provide it through the Starling process
environment, a permission-restricted credentials file, or a system secret
service. Do not store long-lived credentials in a world-readable
configuration.

### Verify that the hook is invoked

Before integrating a provider API, use the following script to validate the
environment:

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

This script only records invocations and does not create real TXT records, so
it cannot be used to request production certificates.

## 6. Local Development Certificates

The following names are classified as local development names and do not
request public ACME certificates:

- `localhost`
- `*.localhost`
- `localhost.localdomain`
- `*.localhost.localdomain`
- `*.local`
- `localdomain` and `*.localdomain`
- `*.test`
- `*.invalid`
- `*.example`
- `internal` and `*.internal`
- `home.arpa` and `*.home.arpa`
- `example.com`, `example.net`, `example.org`, and their subdomains
- IP addresses
- Non-FQDNs without a dot

Example:

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

The local CA is stored by default at:

```text
auto_https/local-ca.crt
```

The local CA private key is stored at:

```text
auto_https/local-ca.key
```

Import `local-ca.crt` into the system or browser trust store to stop local
HTTPS warnings.

### Linux

Debian/Ubuntu example:

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

Run this from an administrator command prompt:

```bat
certutil -addstore -f Root auto_https\local-ca.crt
```

Browsers such as Firefox that use a separate certificate store may require
you to import the CA separately in browser settings.

The local CA private key can issue trusted certificates and must be access
restricted. On Unix systems, Starling attempts to set the certificate
directory permissions to `0700` and private key file permissions to `0600`.

## 7. Using an ACME Test Environment

Let's Encrypt staging avoids production rate limits:

```nginx
auto_https on
    email=admin@your-domain.com
    staging=on;
```

Staging certificates are not trusted by public browsers and should only be
used to validate the issuance flow.

You can also set an ACME Directory explicitly:

```nginx
auto_https on
    email=admin@your-domain.com
    acme_directory=https://acme-staging-v02.api.letsencrypt.org/directory;
```

When using Pebble or another private ACME service, you can also specify its CA:

```nginx
auto_https on
    email=admin@your-domain.com
    acme_directory=http://127.0.0.1:14000/dir
    acme_ca_file=pebble-ca.crt
    challenge=http-01;
```

`acme_ca_file` verifies the HTTPS ACME server certificate. It is not served
to clients as the final site certificate. This option can remain set even when
the ACME Directory uses `http://`.

## 8. Custom Listening Ports

Unprivileged ports can be used in test or container environments:

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

HTTP requests are redirected to:

```text
https://localhost:8443/...
```

Public ACME HTTP-01 validation still requires the CA to access the target
domain's public port 80. `http_port=8080` works with HTTP-01 only when external
port 80 is mapped to internal port 8080.

DNS-01 does not depend on inbound port 80, but an HTTP listener is usually
still configured for HTTP-to-HTTPS redirects and ordinary access.

## 9. Certificate Coverage

Starling generates one certificate covering all derivable names in each
server's `server_name`.

- Exact names are included in the certificate SAN.
- Leading wildcards such as `*.your-domain.com` are included as wildcard SANs;
  only `dns-01` supports them.
- Regular-expression `server_name` values are not included in certificate
  SANs.
- Servers that already have explicit `ssl_certificate` and
  `ssl_certificate_key` are not overwritten by automatic certificates, but
  automatic HTTPS listener logic is still applied.
- `ssl_certificate` and `ssl_certificate_key` must be configured together.
- When the same server contains both local and public names, automatic ACME is
  skipped and a warning is logged because one public certificate should not
  cover local development names.

If you need both local and public names, split them into separate servers or
provide certificates explicitly.

## 10. HTTP-to-HTTPS Redirects

By default, Starling returns 308 when all of the following are true:

- The request arrives on a non-TLS `http_port` listener.
- The current server has a corresponding TLS listener on `https_port`.
- `redirect=on`.
- The Host has not been rejected by another rule.

Disable redirects while keeping automatic HTTPS:

```nginx
auto_https on redirect=off;
```

HTTP-01 challenge paths continue to work when `redirect=off`; the automatic
HTTPS module still serves them during HTTP-01 issuance.

## 11. Certificate Directory and Renewal

The default directory layout is:

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

- `acme-account.key` is the ACME account private key.
- Certificates and private keys are updated by atomic replacement.
- `certs/*.source` records the certificate source, such as ACME or a local
  placeholder certificate.
- `.auto-https.lock` and per-certificate locks prevent concurrent issuance
  during startup, renewal, and multi-process operation.

The current default renewal policy is:

- Background checks run every 12 hours.
- Renewal starts when less than 30 days of validity remain.
- After a failed request, backoff starts at 5 minutes, doubles each time, and
  is capped at 1 hour.
- After successful renewal, the old certificate remains available for
  serving while the rustls configuration is hot-replaced.

These renewal parameters are internal defaults and currently have no
configuration directives.

Do not delete `acme-account.key` without understanding the consequences.
Doing so creates a new account; existing orders and account-side state cannot
be reused, and additional ACME rate limits may be triggered.

## 12. Complete Examples

### Public ordinary domain: HTTP-01

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

### Public wildcard: DNS-01

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

### Local development

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

## 13. Checks and Troubleshooting

First validate the configuration syntax:

```sh
starling -t -c /path/to/nginx.conf
```

View the error log:

```sh
tail -f /path/to/logs/error.log
```

Common issues:

### No public certificate was generated

- Confirm that the domain is classified as public rather than a local name
  such as `localhost`, an IP address, or a `.test` name.
- Confirm that the server listens on `http_port` or `https_port`.
- Confirm that no explicit `ssl_certificate` is taking over certificate
  management.
- Check whether the server mixes local and public names.
- Check whether the ACME Directory and outbound HTTPS/HTTP network are
  reachable.

### HTTP-01 fails

- Confirm that public DNS A/AAAA records are correct.
- Confirm that public TCP port 80 is reachable.
- Confirm that no proxy intercepts `/.well-known/acme-challenge/`.
- Confirm that the CDN, WAF, or load balancer forwards the challenge path.
- Check CAA records.
- If using staging or Pebble, confirm that the validation service can reach
  the current instance.

### DNS-01 fails

- Confirm that `dns_hook` exists and is executable.
- Run the hook manually and check provider credentials and API permissions.
- Check hook stderr in the error log.
- Confirm that the provider supports multiple TXT values for the same record
  name.
- Check whether the `_acme-challenge` record is incorrectly shadowed by a
  static record or CNAME.
- Increase `dns_propagation_delay`, for example from `30s` to `60s` or `120s`.
- Use DNS query tools to confirm that multiple authoritative DNS servers
  already return the TXT value.

### The browser does not trust the local certificate

- Confirm that you imported `auto_https/local-ca.crt`, not the site leaf
  certificate.
- Confirm that the browser or operating system actually trusts the CA.
- Firefox may require a separate import.
- Re-import the CA after it is regenerated.

### The redirect URL has the wrong port

Check `auto_https_https_port`. When the HTTPS port is not 443, the redirect
Location explicitly includes the port.

### Configuration error `challenge=dns-01 requires a dns_hook`

DNS-01 requires a hook:

```nginx
auto_https on challenge=dns-01 dns_hook=/path/to/hook;
```

## 14. Current Limitations

- No DNS provider SDK is built in. DNS-01 is integrated through an external
  hook.
- The current implementation supports HTTP-01 and DNS-01, but not
  TLS-ALPN-01.
- HTTP-01 does not support wildcard domains.
- Regular-expression `server_name` values do not participate in automatic
  certificate name generation.
- Automatic HTTPS directives are supported only in the main context and
  cannot be overridden per server.
- ACME is skipped when local and public names are mixed in the same server.
- Automatic certificates cover all derivable names in a server; arbitrary
  subsets of SANs cannot be selected separately.
