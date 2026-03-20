#!/usr/bin/env bash
#
# Build script for RatTunnel
#
# Builds all binaries inside Docker and copies them to ./build/
#   - rathole server (dynamic, default features + api) — for Docker
#   - rathole agent  (static, client-only with rustls) — for distribution
#   - qtun-controller (static) — for Docker/distribution
#

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

info "Building all binaries in Docker..."
cd "$SCRIPT_DIR"

docker build -f Dockerfile.build --target export --output "type=local,dest=$BUILD_DIR" . \
    || fail "Docker build failed"

ok "All binaries built"

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
