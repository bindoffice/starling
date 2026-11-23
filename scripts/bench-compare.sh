#!/usr/bin/env bash
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/bench-compare.sh [options]

Runs repeatable Starling/native-Nginx loopback benchmarks and records TSV/JSON
summaries. Each server is started alone on the same port with equivalent
generated configuration.

Options:
  --starling-bin PATH       Starling binary (default: target/release/starling)
  --nginx-bin PATH          Native Nginx binary (default: nginx from PATH)
  --wrk-bin PATH            wrk binary (default: wrk from PATH)
  --h2load-bin PATH         h2load binary (default: h2load from PATH)
  --library-path PATH       Extra dynamic-library path for both servers
  --port PORT               Loopback port (default: 18080)
  --rounds N                Measured rounds per scenario (default: 3)
  --duration DURATION       wrk duration (default: 5s)
  --warmup DURATION         wrk warmup duration (default: 1s)
  --threads N               wrk/h2load client threads (default: 4)
  --connections N           HTTP/1.1 connections (default: 128)
  --large-connections N     HTTP/1.1 large-file connections (default: 16)
  --h2-clients N            HTTP/2 clients (default: 64)
  --h2-streams N            HTTP/2 streams per connection (default: 16)
  --h2-requests N           HTTP/2 total requests (default: 500000)
  --upload-requests N       HTTP/2 1 MiB upload requests (default: 1024)
  --workers VALUE           worker_processes value (default: auto)
  --small-bytes N           Small response size (default: 14)
  --large-mib N             Large response size in MiB (default: 8)
  --output-dir PATH         Artifact directory (default: mktemp under TMPDIR)
  --no-build                Do not run cargo build --release
  --skip-native             Run Starling only
  --skip-starling           Run native Nginx only
  --skip-large              Skip the 8 MiB download scenario
  --skip-upload             Skip the HTTP/2 upload scenario
  --keep                    Keep the generated artifact directory on exit
  -h, --help                Show this help
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

need_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

STARLING_BIN="target/release/starling"
NGINX_BIN=""
WRK_BIN=""
H2LOAD_BIN=""
LIBRARY_PATH_OPT=""
CLIENT_LIBRARY_PATH=""
PORT=18080
ROUNDS=3
DURATION=5s
WARMUP=1s
THREADS=4
CONNECTIONS=128
LARGE_CONNECTIONS=16
H2_CLIENTS=64
H2_STREAMS=16
H2_REQUESTS=500000
UPLOAD_REQUESTS=1024
WORKERS=auto
SMALL_BYTES=14
LARGE_MIB=8
OUTPUT_DIR=""
BUILD=1
RUN_STARLING=1
RUN_NATIVE=1
RUN_LARGE=1
RUN_UPLOAD=1
KEEP=0

while (($#)); do
    case "$1" in
        --starling-bin) STARLING_BIN="${2:?missing value}"; shift 2 ;;
        --nginx-bin) NGINX_BIN="${2:?missing value}"; shift 2 ;;
        --wrk-bin) WRK_BIN="${2:?missing value}"; shift 2 ;;
        --h2load-bin) H2LOAD_BIN="${2:?missing value}"; shift 2 ;;
        --library-path) LIBRARY_PATH_OPT="${2:?missing value}"; shift 2 ;;
        --port) PORT="${2:?missing value}"; shift 2 ;;
        --rounds) ROUNDS="${2:?missing value}"; shift 2 ;;
        --duration) DURATION="${2:?missing value}"; shift 2 ;;
        --warmup) WARMUP="${2:?missing value}"; shift 2 ;;
        --threads) THREADS="${2:?missing value}"; shift 2 ;;
        --connections) CONNECTIONS="${2:?missing value}"; shift 2 ;;
        --large-connections) LARGE_CONNECTIONS="${2:?missing value}"; shift 2 ;;
        --h2-clients) H2_CLIENTS="${2:?missing value}"; shift 2 ;;
        --h2-streams) H2_STREAMS="${2:?missing value}"; shift 2 ;;
        --h2-requests) H2_REQUESTS="${2:?missing value}"; shift 2 ;;
        --upload-requests) UPLOAD_REQUESTS="${2:?missing value}"; shift 2 ;;
        --workers) WORKERS="${2:?missing value}"; shift 2 ;;
        --small-bytes) SMALL_BYTES="${2:?missing value}"; shift 2 ;;
        --large-mib) LARGE_MIB="${2:?missing value}"; shift 2 ;;
        --output-dir) OUTPUT_DIR="${2:?missing value}"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --skip-native) RUN_NATIVE=0; shift ;;
        --skip-starling) RUN_STARLING=0; shift ;;
        --skip-large) RUN_LARGE=0; shift ;;
        --skip-upload) RUN_UPLOAD=0; shift ;;
        --keep) KEEP=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

[[ "$PORT" =~ ^[0-9]+$ ]] || die "invalid port: $PORT"
[[ "$ROUNDS" =~ ^[1-9][0-9]*$ ]] || die "invalid rounds: $ROUNDS"
[[ "$THREADS" =~ ^[1-9][0-9]*$ ]] || die "invalid threads: $THREADS"
[[ "$CONNECTIONS" =~ ^[1-9][0-9]*$ ]] || die "invalid connections: $CONNECTIONS"
[[ "$LARGE_CONNECTIONS" =~ ^[1-9][0-9]*$ ]] || die "invalid large connections: $LARGE_CONNECTIONS"
[[ "$H2_CLIENTS" =~ ^[1-9][0-9]*$ ]] || die "invalid h2 clients: $H2_CLIENTS"
[[ "$H2_STREAMS" =~ ^[1-9][0-9]*$ ]] || die "invalid h2 streams: $H2_STREAMS"
[[ "$H2_REQUESTS" =~ ^[1-9][0-9]*$ ]] || die "invalid h2 requests: $H2_REQUESTS"
[[ "$UPLOAD_REQUESTS" =~ ^[1-9][0-9]*$ ]] || die "invalid upload requests: $UPLOAD_REQUESTS"
((H2_REQUESTS >= H2_CLIENTS)) || die "h2 requests must be at least h2 clients"
if ((RUN_UPLOAD)); then
    ((UPLOAD_REQUESTS >= H2_CLIENTS)) || die "upload requests must be at least h2 clients"
fi
[[ "$SMALL_BYTES" =~ ^[1-9][0-9]*$ ]] || die "invalid small size: $SMALL_BYTES"
[[ "$LARGE_MIB" =~ ^[1-9][0-9]*$ ]] || die "invalid large size: $LARGE_MIB"
((RUN_STARLING || RUN_NATIVE)) || die "both implementations were skipped"

WRK_BIN="${WRK_BIN:-$(command -v wrk || true)}"
H2LOAD_BIN="${H2LOAD_BIN:-$(command -v h2load || true)}"
NGINX_BIN="${NGINX_BIN:-$(command -v nginx || true)}"

[[ -n "$WRK_BIN" ]] || die "wrk not found; pass --wrk-bin"
[[ -n "$H2LOAD_BIN" ]] || die "h2load not found; pass --h2load-bin"
[[ -x "$WRK_BIN" ]] || die "wrk is not executable: $WRK_BIN"
[[ -x "$H2LOAD_BIN" ]] || die "h2load is not executable: $H2LOAD_BIN"
if ((RUN_NATIVE)); then
    [[ -n "$NGINX_BIN" ]] || die "nginx not found; pass --nginx-bin"
    [[ -x "$NGINX_BIN" ]] || die "nginx is not executable: $NGINX_BIN"
fi
need_command curl
need_command pgrep
need_command realpath
WRK_BIN="$(realpath "$WRK_BIN")"
H2LOAD_BIN="$(realpath "$H2LOAD_BIN")"
STARLING_BIN="$(realpath "$STARLING_BIN")"
if ((RUN_NATIVE)); then
    NGINX_BIN="$(realpath "$NGINX_BIN")"
fi

if [[ -z "$LIBRARY_PATH_OPT" && "$NGINX_BIN" == */root/usr/sbin/nginx ]]; then
    PACKAGE_ROOT="${NGINX_BIN%/usr/sbin/nginx}"
    LIBRARY_PATH_OPT="$PACKAGE_ROOT/usr/lib/x86_64-linux-gnu"
fi

add_client_library_root() {
    local binary="$1"
    [[ "$binary" == */root/usr/bin/* ]] || return 0
    local root="${binary%/usr/bin/*}"
    local library_path="$root/usr/lib/x86_64-linux-gnu"
    [[ -d "$library_path" ]] || return 0
    case ":$CLIENT_LIBRARY_PATH:" in
        *":$library_path:"*) ;;
        *) CLIENT_LIBRARY_PATH="${CLIENT_LIBRARY_PATH:+$CLIENT_LIBRARY_PATH:}$library_path" ;;
    esac
}
add_client_library_root "$WRK_BIN"
add_client_library_root "$H2LOAD_BIN"

if ((BUILD)); then
    need_command cargo
    cargo build --release --offline
fi
[[ -x "$STARLING_BIN" ]] || die "Starling binary is not executable: $STARLING_BIN"

OUTPUT_DIR="${OUTPUT_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/starling-bench.XXXXXX")}"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR="$(cd "$OUTPUT_DIR" && pwd)"
HTML_DIR="$OUTPUT_DIR/html"
SUMMARY_TSV="$OUTPUT_DIR/summary.tsv"
SUMMARY_JSON="$OUTPUT_DIR/summary.json"
mkdir -p "$HTML_DIR"

head -c "$SMALL_BYTES" /dev/zero | tr '\0' 'x' >"$HTML_DIR/small"
head -c "$((LARGE_MIB * 1024 * 1024))" /dev/zero | tr '\0' 'y' >"$HTML_DIR/large.bin"
head -c "$((1024 * 1024))" /dev/zero | tr '\0' 'u' >"$OUTPUT_DIR/upload-1mib.bin"

SERVER_PID=""
SAMPLER_PID=""
SERVER_NAME=""
SERVER_PREFIX=""
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/bench/runners.sh"

stop_sampler() {
    if [[ -n "$SAMPLER_PID" ]]; then
        kill "$SAMPLER_PID" 2>/dev/null || true
        wait "$SAMPLER_PID" 2>/dev/null || true
        SAMPLER_PID=""
    fi
}

stop_server() {
    stop_sampler
    if [[ -n "$SERVER_PID" ]]; then
        if kill -0 "$SERVER_PID" 2>/dev/null; then
            kill -QUIT "$SERVER_PID" 2>/dev/null || true
            for _ in {1..50}; do
                kill -0 "$SERVER_PID" 2>/dev/null || break
                sleep 0.1
            done
        fi
        if kill -0 "$SERVER_PID" 2>/dev/null; then
            kill -TERM "$SERVER_PID" 2>/dev/null || true
            sleep 0.2
        fi
        if kill -0 "$SERVER_PID" 2>/dev/null; then
            kill -KILL "$SERVER_PID" 2>/dev/null || true
        fi
        wait "$SERVER_PID" 2>/dev/null || true
        SERVER_PID=""
    fi
}

cleanup() {
    stop_server
    if ((KEEP)); then
        printf 'artifacts kept in %s\n' "$OUTPUT_DIR"
    else
        rm -rf "$OUTPUT_DIR"
    fi
}
trap cleanup EXIT INT TERM

write_config() {
    local prefix="$1"
    local config="$prefix/nginx.conf"
    mkdir -p "$prefix/logs"
    mkdir -p "$prefix/temp/client_body" "$prefix/temp/proxy" "$prefix/temp/fastcgi"
    mkdir -p "$prefix/temp/uwsgi" "$prefix/temp/scgi"
    cat >"$config" <<EOF
daemon off;
worker_processes $WORKERS;
pid logs/nginx.pid;
error_log logs/error.log warn;
events {
    worker_connections 4096;
}
http {
    access_log off;
    sendfile on;
    tcp_nopush on;
    keepalive_requests 100000000;
    keepalive_timeout 300s;
    client_body_temp_path $prefix/temp/client_body;
    proxy_temp_path $prefix/temp/proxy;
    fastcgi_temp_path $prefix/temp/fastcgi;
    uwsgi_temp_path $prefix/temp/uwsgi;
    scgi_temp_path $prefix/temp/scgi;
    server {
        listen 127.0.0.1:$PORT http2;
        server_name localhost;
        root "$HTML_DIR";
        location / {
        }
        location = /upload {
            client_body_buffer_size 64k;
            proxy_request_buffering on;
            proxy_pass http://127.0.0.1:$PORT/__drain;
        }
        location = /__drain {
            return 204;
        }
    }
}
EOF
}

wait_for_server() {
    local name="$1"
    local error_log="$2"
    for _ in {1..100}; do
        if ! kill -0 "$SERVER_PID" 2>/dev/null; then
            tail -n 40 "$error_log" >&2 || true
            die "$name exited during startup"
        fi
        if curl -fsS --max-time 1 "http://127.0.0.1:$PORT/small" >/dev/null 2>&1; then
            return 0
        fi
        sleep 0.1
    done
    tail -n 40 "$error_log" >&2 || true
    die "$name did not become ready on port $PORT"
}

start_server() {
    local name="$1"
    local binary="$2"
    SERVER_NAME="$name"
    SERVER_PREFIX="$OUTPUT_DIR/$name"
    rm -rf "$SERVER_PREFIX"
    write_config "$SERVER_PREFIX"
    local stdout="$OUTPUT_DIR/$name.stdout"
    local stderr="$OUTPUT_DIR/$name.stderr"
    local env_command=(env)
    if [[ -n "$LIBRARY_PATH_OPT" ]]; then
        env_command+=("LD_LIBRARY_PATH=$LIBRARY_PATH_OPT${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}")
    fi
    (
        cd "$SERVER_PREFIX"
        "${env_command[@]}" "$binary" -p "$SERVER_PREFIX/" -c nginx.conf
    ) >"$stdout" 2>"$stderr" &
    SERVER_PID=$!
    wait_for_server "$name" "$SERVER_PREFIX/logs/error.log"
}

collect_tree() {
    local root="$1"
    local -a queue=("$root")
    local -a result=()
    local index=0
    while ((index < ${#queue[@]})); do
        local pid="${queue[$index]}"
        index=$((index + 1))
        [[ "$pid" =~ ^[0-9]+$ ]] || continue
        [[ -d "/proc/$pid" ]] || continue
        result+=("$pid")
        while read -r child; do
            [[ -n "$child" ]] && queue+=("$child")
        done < <(pgrep -P "$pid" 2>/dev/null || true)
    done
    ((${#result[@]})) && printf '%s\n' "${result[@]}"
}

snapshot_resources() {
    local root="$1"
    local rss=0
    local threads=0
    local fds=0
    local pid value
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        value="$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status" 2>/dev/null || true)"
        [[ -n "$value" ]] && rss=$((rss + value))
        value="$(awk '/^Threads:/ {print $2}' "/proc/$pid/status" 2>/dev/null || true)"
        [[ -n "$value" ]] && threads=$((threads + value))
        value="$(find "/proc/$pid/fd" -mindepth 1 -maxdepth 1 2>/dev/null | wc -l)"
        fds=$((fds + value))
    done < <(collect_tree "$root")
    printf '%s\t%s\t%s\n' "$rss" "$threads" "$fds"
}

start_sampler() {
    local root="$1"
    local output="$2"
    printf 'rss_kib\tthreads\tfds\n' >"$output"
    snapshot_resources "$root" >>"$output"
    (
        while kill -0 "$root" 2>/dev/null; do
            snapshot_resources "$root" >>"$output"
            sleep 0.2
        done
    ) &
    SAMPLER_PID=$!
}

run_implementation() {
    local implementation="$1"
    local binary="$2"
    start_server "$implementation" "$binary"
    run_wrk_scenario "$implementation" h1-small /small "$CONNECTIONS"
    if ((RUN_LARGE)); then
        run_wrk_scenario "$implementation" h1-large /large.bin "$LARGE_CONNECTIONS"
    fi
    run_h2_scenario "$implementation" small /small "$H2_REQUESTS"
    if ((RUN_UPLOAD)); then
        run_h2_scenario "$implementation" upload-1mib /upload "$UPLOAD_REQUESTS" \
            "$OUTPUT_DIR/upload-1mib.bin"
    fi
    stop_server
}

printf 'implementation\ttransport\tscenario\tround\treq_per_sec\tp50_ms\tp99_ms\tmean_request_ms\ttransfer_mb_s\trequests\tsucceeded\tfailed\terrored\ttimeout\tpeak_rss_kib\tpeak_threads\tpeak_fds\traw_file\n' >"$SUMMARY_TSV"

if ((RUN_STARLING)); then
    run_implementation starling "$STARLING_BIN"
fi
if ((RUN_NATIVE)); then
    run_implementation native "$NGINX_BIN"
fi

if command -v jq >/dev/null 2>&1; then
    jq -Rn '
        def number_or_null:
            if . == "-" or . == "" then null else tonumber end;
        [
            inputs
            | select(startswith("implementation\t") | not)
            | split("\t") as $f
            | {
                implementation: $f[0],
                transport: $f[1],
                scenario: $f[2],
                round: ($f[3] | tonumber),
                req_per_sec: ($f[4] | number_or_null),
                p50_ms: ($f[5] | number_or_null),
                p99_ms: ($f[6] | number_or_null),
                mean_request_ms: ($f[7] | number_or_null),
                transfer_mb_s: ($f[8] | number_or_null),
                requests: ($f[9] | number_or_null),
                succeeded: ($f[10] | number_or_null),
                failed: ($f[11] | number_or_null),
                errored: ($f[12] | number_or_null),
                timeout: ($f[13] | number_or_null),
                peak_rss_kib: ($f[14] | number_or_null),
                peak_threads: ($f[15] | number_or_null),
                peak_fds: ($f[16] | number_or_null),
                raw_file: $f[17]
              }
        ]
    ' <"$SUMMARY_TSV" >"$SUMMARY_JSON"
fi

printf 'summary: %s\n' "$SUMMARY_TSV"
if [[ -f "$SUMMARY_JSON" ]]; then
    printf 'json:    %s\n' "$SUMMARY_JSON"
fi
column -t -s $'\t' "$SUMMARY_TSV" 2>/dev/null || cat "$SUMMARY_TSV"

if ((KEEP)); then
    trap - EXIT
    printf 'artifacts kept in %s\n' "$OUTPUT_DIR"
else
    KEEP=0
fi
