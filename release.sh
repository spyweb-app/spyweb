#!/bin/bash

# SpyWeb Release Script
# This script builds and packages SpyWeb into a release ZIP exactly as described in README.md.

set -e

# 1. Configuration
# Extract version from Cargo.toml
VERSION=$(grep '^version =' Cargo.toml | head -1 | cut -d '"' -f 2)
NAME="spyweb"
DIST_DIR="dist"
PACKAGE_DIR="${NAME}" 

# Determine OS, Arch and Binary Extension
OS_NAME=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)
EXE_EXT=""

# Normalize OS names for user-friendly filenames
if [[ "$OS_NAME" == "darwin" ]]; then
    OS_NAME="macos"
elif [[ "$OS_NAME" == *"mingw"* || "$OS_NAME" == *"msys"* || "$OS_NAME" == *"windows_nt"* ]]; then
    OS_NAME="windows"
    EXE_EXT=".exe"
fi

echo "[*] Releasing SpyWeb v${VERSION} for ${OS_NAME} (${ARCH})..."

# 2. Clean and Prepare
echo "[*] Cleaning previous distribution..."
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR/$PACKAGE_DIR"

# 3. Build Binaries
if [[ "$OS_NAME" == "macos" ]]; then
    echo "[*] Building Universal Binaries for macOS (Intel + Apple Silicon)..."
    rustup target add x86_64-apple-darwin aarch64-apple-darwin
    
    cargo build --release --target x86_64-apple-darwin --bin spyweb --bin spyweb-tray --features tray
    cargo build --release --target aarch64-apple-darwin --bin spyweb --bin spyweb-tray --features tray
    
    mkdir -p target/universal
    lipo -create -output target/universal/spyweb target/x86_64-apple-darwin/release/spyweb target/aarch64-apple-darwin/release/spyweb
    lipo -create -output target/universal/spyweb-tray target/x86_64-apple-darwin/release/spyweb-tray target/aarch64-apple-darwin/release/spyweb-tray
    
    # Copy from universal folder
    cp target/universal/spyweb "$DIST_DIR/$PACKAGE_DIR/"
    cp target/universal/spyweb-tray "$DIST_DIR/$PACKAGE_DIR/"
    ARCH="universal"
else
    # Linux or Windows build
    echo "[*] Building binaries (release)..."
    cargo build --release --bin spyweb --bin spyweb-tray --features tray
    cp "target/release/spyweb${EXE_EXT}" "$DIST_DIR/$PACKAGE_DIR/"
    cp "target/release/spyweb-tray${EXE_EXT}" "$DIST_DIR/$PACKAGE_DIR/"
fi

# 4. Assemble the Package
echo "[*] Assembling package in $DIST_DIR/$PACKAGE_DIR..."

# Create and populate jobs/ directory from starter-kit
mkdir -p "$DIST_DIR/$PACKAGE_DIR/jobs/starter-kit"
if [ -d "examples/starter-kit" ]; then
    cp -r examples/starter-kit/* "$DIST_DIR/$PACKAGE_DIR/jobs/starter-kit/"
fi

# Copy jobs.toml.example as jobs.toml
if [ -f "jobs.toml.example" ]; then
    cp jobs.toml.example "$DIST_DIR/$PACKAGE_DIR/jobs.toml"
fi

# Copy Documentation, Examples & UI
if [ -d "docs" ]; then 
    cp -r docs "$DIST_DIR/$PACKAGE_DIR/"
    # Move licenses into docs to keep root clean
    cp LICENSE-MIT "$DIST_DIR/$PACKAGE_DIR/docs/" 2>/dev/null || true
    cp LICENSE-APACHE "$DIST_DIR/$PACKAGE_DIR/docs/" 2>/dev/null || true
fi

if [ -d "examples" ]; then cp -r examples "$DIST_DIR/$PACKAGE_DIR/"; fi
if [ -d "ui" ]; then cp -r ui "$DIST_DIR/$PACKAGE_DIR/"; fi

# No README in the release ZIP - keep it on GitHub only

# 5. Create the Final Archive
echo "[*] Creating archive..."
ARCHIVE_FILE="${NAME}-v${VERSION}-${OS_NAME}-${ARCH}.zip"

cd "$DIST_DIR"
if command -v zip >/dev/null 2>&1; then
    zip -r "../$ARCHIVE_FILE" "$PACKAGE_DIR"
    echo "[+] Success! Archive created: $ARCHIVE_FILE"
elif command -v 7z >/dev/null 2>&1; then
    # Windows runners always have 7z
    7z a "../$ARCHIVE_FILE" "$PACKAGE_DIR"
    echo "[+] Success! Archive created: $ARCHIVE_FILE"
else
    # Fallback to tar.gz
    ARCHIVE_FILE="${NAME}-v${VERSION}-${OS_NAME}-${ARCH}.tar.gz"
    tar -czvf "../$ARCHIVE_FILE" "$PACKAGE_DIR"
    echo "[+] Success! Archive created: $ARCHIVE_FILE"
fi
cd ..
