#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"
export RUST_BACKTRACE="${RUST_BACKTRACE:-1}"
if [[ -z "${TMPDIR:-}" ]]; then
    export TMPDIR="$ROOT_DIR/target/ci-tmp"
    mkdir -p "$TMPDIR"
fi

echo "==> cargo fmt"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --all-targets --locked -- -D warnings

echo "==> cargo test"
cargo test --all-targets --locked

echo "==> cargo build --release"
cargo build --release --locked --features vendored-openssl

echo "CI checks passed."
