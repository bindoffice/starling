#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
    cat <<'EOF'
Usage: scripts/package-release.sh [options]

Options:
  --version VERSION          Package version (default: Cargo.toml package version)
  --target TARGET            Rust target triple (default: host target)
  --platform PLATFORM        Target platform: auto, linux, macos, windows, other
                             (default: inferred from --target)
  --archive-format FORMAT    Archive format: auto, tar.gz, zip
                             (default: zip for Windows, tar.gz otherwise)
  --binary PATH              Path to the release binary
  --output-dir PATH          Directory for archives (default: dist)
  -h, --help                 Show this help
EOF
}

VERSION=""
TARGET=""
PLATFORM="auto"
ARCHIVE_FORMAT="auto"
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
        --platform)
            PLATFORM="${2:?missing value for --platform}"
            shift 2
            ;;
        --archive-format)
            ARCHIVE_FORMAT="${2:?missing value for --archive-format}"
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

detect_platform() {
    case "$TARGET" in
        *-windows-*)
            echo windows
            ;;
        *-apple-darwin)
            echo macos
            ;;
        *-linux-*)
            echo linux
            ;;
        *)
            echo other
            ;;
    esac
}

if [[ "$PLATFORM" == "auto" ]]; then
    PLATFORM="$(detect_platform)"
fi
case "$PLATFORM" in
    linux | macos | windows | other) ;;
    *)
        echo "unsupported platform: $PLATFORM (expected auto, linux, macos, windows, or other)" >&2
        exit 2
        ;;
esac

if [[ "$ARCHIVE_FORMAT" == "auto" ]]; then
    if [[ "$PLATFORM" == "windows" ]]; then
        ARCHIVE_FORMAT="zip"
    else
        ARCHIVE_FORMAT="tar.gz"
    fi
fi
case "$ARCHIVE_FORMAT" in
    tar.gz | zip) ;;
    *)
        echo "unsupported archive format: $ARCHIVE_FORMAT (expected auto, tar.gz, or zip)" >&2
        exit 2
        ;;
esac

BIN_NAME="starling"
if [[ "$PLATFORM" == "windows" ]]; then
    BIN_NAME="starling.exe"
fi

if [[ -z "$BINARY" ]]; then
    if [[ -f "target/${TARGET}/release/${BIN_NAME}" ]]; then
        BINARY="target/${TARGET}/release/${BIN_NAME}"
    else
        BINARY="target/release/${BIN_NAME}"
    fi
fi

if [[ ! -f "$BINARY" ]]; then
    echo "release binary not found: $BINARY" >&2
    echo "run: cargo build --release --locked --features vendored-openssl --target $TARGET" >&2
    exit 1
fi
if [[ "$PLATFORM" != "windows" && ! -x "$BINARY" ]]; then
    echo "release binary is not executable: $BINARY" >&2
    exit 1
fi

PACKAGE_NAME="starling-${VERSION}-${TARGET}"
PACKAGE_DIR="${OUTPUT_DIR}/${PACKAGE_NAME}"
ARCHIVE="${OUTPUT_DIR}/${PACKAGE_NAME}.${ARCHIVE_FORMAT}"

rm -rf "$PACKAGE_DIR" "$ARCHIVE" "${ARCHIVE}.sha256"
mkdir -p \
    "$PACKAGE_DIR/bin" \
    "$PACKAGE_DIR/conf" \
    "$PACKAGE_DIR/docs" \
    "$PACKAGE_DIR/html" \
    "$PACKAGE_DIR/logs"

cp "$BINARY" "$PACKAGE_DIR/bin/${BIN_NAME}"
if [[ "$PLATFORM" != "windows" ]]; then
    chmod 755 "$PACKAGE_DIR/bin/${BIN_NAME}"
fi
cp -R conf/. "$PACKAGE_DIR/conf/"
cp -R html/. "$PACKAGE_DIR/html/"
cp -R docs/. "$PACKAGE_DIR/docs/"

for file in README.md README.en.md FEATURES.md FEATURES.en.md LICENSE LICENSE.md; do
    if [[ -f "$file" ]]; then
        cp "$file" "$PACKAGE_DIR/"
    fi
done

mkdir -p "$OUTPUT_DIR"
archive_name="$(basename "$ARCHIVE")"
case "$ARCHIVE_FORMAT" in
    tar.gz)
        (
            cd "$OUTPUT_DIR"
            tar -czf "$archive_name" "$PACKAGE_NAME"
        )
        ;;
    zip)
        if command -v zip >/dev/null 2>&1; then
            (
                cd "$OUTPUT_DIR"
                zip -q -r "$archive_name" "$PACKAGE_NAME"
            )
        elif command -v 7z >/dev/null 2>&1; then
            (
                cd "$OUTPUT_DIR"
                7z a -tzip "$archive_name" "$PACKAGE_NAME" >/dev/null
            )
        elif command -v bsdtar >/dev/null 2>&1; then
            (
                cd "$OUTPUT_DIR"
                bsdtar -a -cf "$archive_name" "$PACKAGE_NAME"
            )
        elif command -v tar >/dev/null 2>&1 &&
            tar --version 2>/dev/null | grep -qi bsdtar; then
            (
                cd "$OUTPUT_DIR"
                tar -a -c -f "$archive_name" "$PACKAGE_NAME"
            )
        elif command -v powershell.exe >/dev/null 2>&1; then
            (
                cd "$OUTPUT_DIR"
                powershell.exe -NoProfile -Command \
                    "Compress-Archive -LiteralPath '$PACKAGE_NAME' -DestinationPath '$archive_name' -Force"
            )
        else
            echo "no ZIP creator found (tried zip, 7z, bsdtar, tar, and powershell.exe)" >&2
            exit 1
        fi
        ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
    (
        cd "$OUTPUT_DIR"
        sha256sum "$archive_name" >"${archive_name}.sha256"
    )
elif command -v shasum >/dev/null 2>&1; then
    (
        cd "$OUTPUT_DIR"
        shasum -a 256 "$archive_name" >"${archive_name}.sha256"
    )
elif command -v openssl >/dev/null 2>&1; then
    (
        cd "$OUTPUT_DIR"
        openssl dgst -sha256 -r "$archive_name" >"${archive_name}.sha256"
    )
else
    echo "no SHA-256 tool found (tried sha256sum, shasum, and openssl)" >&2
    exit 1
fi

echo "created $ARCHIVE"
echo "created ${ARCHIVE}.sha256"
