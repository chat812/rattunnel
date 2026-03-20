#!/usr/bin/env bash
#
# Build script for RatTunnel
#
# Builds:
#   - rathole server (dynamic, default features + api) — for Docker
#   - rathole agent  (static, client-only with rustls) — for distribution
#   - qtun-controller (static) — for Docker/distribution
#
# All binaries are copied to ./build/

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[*]${NC} $1"; }
ok()    { echo -e "${GREEN}[+]${NC} $1"; }
fail()  { echo -e "${RED}[-]${NC} $1"; exit 1; }

mkdir -p "$BUILD_DIR"

# ---------- rathole server (dynamic, default features) ----------
info "Building rathole server (dynamic, default features + api)..."
cd "$SCRIPT_DIR/rathole"
cargo build --release 2>&1 | tail -5
cp target/release/rathole "$BUILD_DIR/rathole-server"
ok "rathole-server built"

# ---------- rathole agent (static, client + rustls) ----------
info "Building rathole agent (static, client-only + rustls)..."
cd "$SCRIPT_DIR/rathole"
RUSTFLAGS='-C target-feature=+crt-static' cargo build --release \
    --no-default-features \
    --features "client,rustls,noise,websocket-rustls" \
    2>&1 | tail -5
cp target/release/rathole "$BUILD_DIR/rathole-agent"
ok "rathole-agent built"

# ---------- qtun-controller (static) ----------
info "Building qtun-controller (static)..."
cd "$SCRIPT_DIR/qtun-controller"
OPENSSL_STATIC=1 RUSTFLAGS='-C target-feature=+crt-static' cargo build --release 2>&1 | tail -5
cp target/release/qtun-controller "$BUILD_DIR/qtun-controller"
ok "qtun-controller built"

# ---------- Summary ----------
echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Build Complete${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo "  Binaries in $BUILD_DIR:"
echo ""
for bin in rathole-server rathole-agent qtun-controller; do
    if [ -f "$BUILD_DIR/$bin" ]; then
        size=$(du -h "$BUILD_DIR/$bin" | cut -f1)
        echo -e "    ${CYAN}$bin${NC}  ($size)"
    fi
done
echo ""
