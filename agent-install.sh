#!/usr/bin/env bash
#
# RatTunnel Agent Installer
#
# Builds the rathole client, runs first-time setup, and installs as a systemd service.
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/chat812/rattunnel/main/agent-install.sh | bash
#   or:
#   ./agent-install.sh [SERVER_API_ADDR]
#
# Examples:
#   ./agent-install.sh                    # prompts for server address
#   ./agent-install.sh 1.2.3.4:9090      # uses provided address

set -euo pipefail

# --- Configuration ---
INSTALL_DIR="/opt/rattunnel"
SERVICE_NAME="rattunnel-agent"
CONFIG_FILE="$INSTALL_DIR/client.toml"
REPO_URL="https://github.com/chat812/rattunnel.git"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[*]${NC} $1"; }
ok()    { echo -e "${GREEN}[+]${NC} $1"; }
warn()  { echo -e "${YELLOW}[!]${NC} $1"; }
err()   { echo -e "${RED}[-]${NC} $1"; exit 1; }

# --- Root check ---
if [ "$(id -u)" -ne 0 ]; then
    err "This script must be run as root (use sudo)"
fi

# --- Get server address ---
SERVER_API_ADDR="${1:-}"
if [ -z "$SERVER_API_ADDR" ]; then
    echo -ne "${CYAN}Enter server API address (e.g. 1.2.3.4:9090): ${NC}"
    read -r SERVER_API_ADDR
fi
[ -z "$SERVER_API_ADDR" ] && err "Server address is required"

info "Server API: $SERVER_API_ADDR"

# --- Check dependencies ---
check_dep() {
    if ! command -v "$1" &>/dev/null; then
        info "Installing $1..."
        apt-get update -qq && apt-get install -y -qq "$1" >/dev/null 2>&1 || true
    fi
}

# --- Install Rust if needed ---
if ! command -v cargo &>/dev/null; then
    info "Installing Rust toolchain..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --quiet
    source "$HOME/.cargo/env"
fi

# Ensure build deps
check_dep git
check_dep libssl-dev 2>/dev/null || check_dep openssl-devel 2>/dev/null || true
check_dep pkg-config

# --- Clone and build ---
info "Cloning rattunnel..."
TMPDIR=$(mktemp -d)
git clone --depth 1 "$REPO_URL" "$TMPDIR/rattunnel" 2>/dev/null

info "Building rathole agent (this may take a few minutes)..."
cd "$TMPDIR/rattunnel/rathole"
cargo build --release --locked 2>&1 | tail -3

BINARY="$TMPDIR/rattunnel/rathole/target/release/rathole"
[ -f "$BINARY" ] || err "Build failed — binary not found"

ok "Build complete"

# --- Install binary ---
mkdir -p "$INSTALL_DIR"
cp "$BINARY" "$INSTALL_DIR/rathole"
chmod +x "$INSTALL_DIR/rathole"
ok "Installed to $INSTALL_DIR/rathole"

# --- First-time setup ---
if [ -f "$CONFIG_FILE" ]; then
    warn "Config already exists at $CONFIG_FILE"
    echo -ne "${YELLOW}Overwrite and re-setup? [y/N]: ${NC}"
    read -r OVERWRITE
    if [ "$OVERWRITE" != "y" ] && [ "$OVERWRITE" != "Y" ]; then
        info "Keeping existing config"
    else
        rm -f "$CONFIG_FILE"
    fi
fi

if [ ! -f "$CONFIG_FILE" ]; then
    echo ""
    echo -ne "${CYAN}Enter setup code: ${NC}"
    read -r SETUP_CODE
    [ -z "$SETUP_CODE" ] && err "Setup code is required"

    info "Fetching config from server..."
    RESPONSE=$(curl -sf "http://$SERVER_API_ADDR/api/v1/setup/$SETUP_CODE" 2>/dev/null) || \
        err "Setup code invalid, expired, or server unreachable"

    # Parse JSON response
    REMOTE_ADDR=$(echo "$RESPONSE" | grep -o '"remote_addr":"[^"]*"' | cut -d'"' -f4)
    TOKEN=$(echo "$RESPONSE" | grep -o '"token":"[^"]*"' | cut -d'"' -f4)
    AGENT_ID=$(echo "$RESPONSE" | grep -o '"agent_id":"[^"]*"' | cut -d'"' -f4)

    [ -z "$REMOTE_ADDR" ] && err "Invalid response from server"

    cat > "$CONFIG_FILE" <<EOF
[client]
remote_addr = "$REMOTE_ADDR"
default_token = "$TOKEN"
agent_id = "$AGENT_ID"
EOF

    ok "Config written to $CONFIG_FILE"
fi

# --- Install systemd service ---
info "Installing systemd service..."

cat > "/etc/systemd/system/${SERVICE_NAME}.service" <<EOF
[Unit]
Description=RatTunnel Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$INSTALL_DIR/rathole $CONFIG_FILE
WorkingDirectory=$INSTALL_DIR
Restart=always
RestartSec=5

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$INSTALL_DIR

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable "$SERVICE_NAME" >/dev/null 2>&1
systemctl restart "$SERVICE_NAME"

ok "Service installed and started"

# --- Verify ---
sleep 2
if systemctl is-active --quiet "$SERVICE_NAME"; then
    ok "Agent is running"
else
    warn "Agent may not have started correctly. Check logs:"
    echo "  journalctl -u $SERVICE_NAME -f"
fi

# --- Cleanup ---
rm -rf "$TMPDIR"

# --- Summary ---
echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  RatTunnel Agent Installed${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "  Binary:  ${CYAN}$INSTALL_DIR/rathole${NC}"
echo -e "  Config:  ${CYAN}$CONFIG_FILE${NC}"
echo -e "  Service: ${CYAN}$SERVICE_NAME${NC}"
echo ""
echo -e "  ${YELLOW}Commands:${NC}"
echo -e "    systemctl status $SERVICE_NAME"
echo -e "    journalctl -u $SERVICE_NAME -f"
echo -e "    systemctl restart $SERVICE_NAME"
echo -e "    systemctl stop $SERVICE_NAME"
echo ""
