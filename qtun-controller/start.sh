#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/qtun-controller"

if [ ! -f "$BINARY" ]; then
    echo "Binary not found, building..."
    cd "$SCRIPT_DIR"
    cargo build --release
fi

cd "$SCRIPT_DIR"

# Kill any running instance
if pgrep -x qtun-controller > /dev/null; then
    echo "Stopping existing instance..."
    pkill -x qtun-controller
    sleep 1
fi

echo "Starting qtun-controller..."
exec sudo -E "$BINARY" --config "$SCRIPT_DIR/config.toml"
