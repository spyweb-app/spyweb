#!/bin/bash

# SpyWeb Release Script
# This script builds and packages SpyWeb into a release tarball.
# Usage: ./release.sh          # default (redb) build
#        ./release.sh sql      # SQLite variant

set -e

# 1. Configuration
VERSION=${SPYWEB_VERSION:-$(grep '^version =' Cargo.toml | head -1 | cut -d '"' -f 2)}
NAME="spyweb"
DIST_DIR="dist"
PACKAGE_DIR="${NAME}"

SUFFIX=""
CARGO_FEATURES=""
TRAY_FEATURES="tray"

if [ "$1" == "sql" ]; then
    SUFFIX="-sql"
    CARGO_FEATURES="--features sqlite"
    TRAY_FEATURES="sqlite tray"
fi

# Determine OS, Arch and Binary Extension
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
EXE_EXT=""

if [[ "$OS_NAME" == "darwin" ]]; then
    OS_NAME="macos"
elif [[ "$OS_NAME" == *"mingw"* || "$OS_NAME" == *"msys"* || "$OS_NAME" == *"windows_nt"* ]]; then
    OS_NAME="windows"
    EXE_EXT=".exe"
fi

echo "[*] Releasing SpyWeb v${VERSION}${SUFFIX} for ${OS_NAME} (${ARCH})..."

# 2. Clean and Prepare
echo "[*] Cleaning previous distribution..."
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/$PACKAGE_DIR"

# 3. Build & Package Helper
create_archive() {
    local arch=$1
    local file_name="${NAME}-v${VERSION}-${OS_NAME}-${arch}${SUFFIX}.tar.gz"

    cd "$DIST_DIR"
    echo "[*] Archiving $file_name..."
    tar -czf "../$file_name" "$PACKAGE_DIR"
    cd ..
    echo "[+] Created: $file_name"
}

assemble_package() {
    mkdir -p "$DIST_DIR/$PACKAGE_DIR/jobs/starter-kit"
    if [ -d "examples/starter-kit" ]; then
        cp -r examples/starter-kit/* "$DIST_DIR/$PACKAGE_DIR/jobs/starter-kit/"
    fi

    if [ -f "jobs.toml.example" ]; then
        cp jobs.toml.example "$DIST_DIR/$PACKAGE_DIR/jobs.toml"
    fi

    if [ -d "docs" ]; then
        cp -r docs "$DIST_DIR/$PACKAGE_DIR/"
        cp LICENSE-MIT "$DIST_DIR/$PACKAGE_DIR/docs/" 2>/dev/null || true
        cp LICENSE-APACHE "$DIST_DIR/$PACKAGE_DIR/docs/" 2>/dev/null || true
    fi

    if [ -d "examples" ]; then cp -r examples "$DIST_DIR/$PACKAGE_DIR/"; fi
    if [ -d "ui" ]; then cp -r ui "$DIST_DIR/$PACKAGE_DIR/"; fi
}

# 4. Build Binaries
if [[ "$OS_NAME" == "macos" ]]; then
    echo "[*] Building for macOS (Intel + Apple Silicon)..."
    rustup target add x86_64-apple-darwin aarch64-apple-darwin

    cargo build --release $CARGO_FEATURES --target x86_64-apple-darwin --bin spyweb
    cargo build --release --features "$TRAY_FEATURES" --target x86_64-apple-darwin --bin spyweb-tray
    cargo build --release $CARGO_FEATURES --target aarch64-apple-darwin --bin spyweb
    cargo build --release --features "$TRAY_FEATURES" --target aarch64-apple-darwin --bin spyweb-tray

    for target_arch in "aarch64-apple-darwin:arm64" "x86_64-apple-darwin:x86_64"; do
        target=${target_arch%%:*}
        arch_name=${target_arch#*:}

        echo "[*] Packaging lean $arch_name build..."
        rm -rf "$DIST_DIR/$PACKAGE_DIR"
        mkdir -p "$DIST_DIR/$PACKAGE_DIR"
        assemble_package
        cp "target/$target/release/spyweb" "$DIST_DIR/$PACKAGE_DIR/"
        cp "target/$target/release/spyweb-tray" "$DIST_DIR/$PACKAGE_DIR/"
        create_archive "$arch_name"
    done
else
    # Linux or Windows build
    echo "[*] Building binaries (release)..."
    cargo build --release $CARGO_FEATURES --bin spyweb
    cargo build --release --features "$TRAY_FEATURES" --bin spyweb-tray
    cp "target/release/spyweb${EXE_EXT}" "$DIST_DIR/$PACKAGE_DIR/"
    cp "target/release/spyweb-tray${EXE_EXT}" "$DIST_DIR/$PACKAGE_DIR/"

    assemble_package
    create_archive "$ARCH"
fi
