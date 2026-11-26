#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
    cat <<'EOF'
Usage: scripts/package-release.sh [options]

Options:
  --version VERSION     Package version (default: Cargo.toml package version)
  --target TARGET       Rust target triple (default: host target)
  --binary PATH         Path to the release binary
  --output-dir PATH     Directory for archives (default: dist)
  -h, --help            Show this help
EOF
}

VERSION=""
TARGET=""
BINARY=""
OUTPUT_DIR="dist"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="${2:?missing value for --version}"
            shift 2
            ;;
        --target)
            TARGET="${2:?missing value for --target}"
            shift 2
            ;;
        --binary)
            BINARY="${2:?missing value for --binary}"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="${2:?missing value for --output-dir}"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            echo "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

if [[ -z "$VERSION" ]]; then
    VERSION="$(
        awk -F '"' '
            /^\[package\]/ { in_package = 1; next }
            /^\[/ && in_package { exit }
            in_package && /^version[[:space:]]*=/ { print $2; exit }
        ' Cargo.toml
    )"
fi
VERSION="${VERSION#v}"

if [[ -z "$VERSION" ]]; then
    echo "failed to determine package version" >&2
    exit 1
fi

if [[ -z "$TARGET" ]]; then
    TARGET="$(rustc -vV | awk '/^host:/ { print $2 }')"
fi

if [[ -z "$BINARY" ]]; then
    if [[ -x "target/${TARGET}/release/starling" ]]; then
        BINARY="target/${TARGET}/release/starling"
    else
        BINARY="target/release/starling"
    fi
fi

if [[ ! -f "$BINARY" ]]; then
    echo "release binary not found: $BINARY" >&2
    echo "run: cargo build --release --locked --target $TARGET" >&2
    exit 1
fi
if [[ ! -x "$BINARY" ]]; then
    echo "release binary is not executable: $BINARY" >&2
    exit 1
fi

PACKAGE_NAME="starling-${VERSION}-${TARGET}"
PACKAGE_DIR="${OUTPUT_DIR}/${PACKAGE_NAME}"
ARCHIVE="${OUTPUT_DIR}/${PACKAGE_NAME}.tar.gz"

rm -rf "$PACKAGE_DIR" "$ARCHIVE" "${ARCHIVE}.sha256"
mkdir -p \
    "$PACKAGE_DIR/bin" \
    "$PACKAGE_DIR/conf" \
    "$PACKAGE_DIR/docs" \
    "$PACKAGE_DIR/html" \
    "$PACKAGE_DIR/logs"

install -m 755 "$BINARY" "$PACKAGE_DIR/bin/starling"
cp -R conf/. "$PACKAGE_DIR/conf/"
cp -R html/. "$PACKAGE_DIR/html/"
cp -R docs/. "$PACKAGE_DIR/docs/"

for file in README.md README.en.md FEATURES.md FEATURES.en.md LICENSE LICENSE.md; do
    if [[ -f "$file" ]]; then
        cp "$file" "$PACKAGE_DIR/"
    fi
done

mkdir -p "$OUTPUT_DIR"
tar -C "$OUTPUT_DIR" -czf "$ARCHIVE" "$PACKAGE_NAME"

if command -v sha256sum >/dev/null 2>&1; then
    (
        cd "$OUTPUT_DIR"
        sha256sum "$(basename "$ARCHIVE")" >"$(basename "$ARCHIVE").sha256"
    )
else
    (
        cd "$OUTPUT_DIR"
        shasum -a 256 "$(basename "$ARCHIVE")" >"$(basename "$ARCHIVE").sha256"
    )
fi

echo "created $ARCHIVE"
echo "created ${ARCHIVE}.sha256"
