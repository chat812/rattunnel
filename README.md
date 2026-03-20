# RatTunnel

Multi-agent reverse tunnel manager with Telegram bot control and connection approval.

Built on a [rathole](https://github.com/rapiz1/rathole) fork with extensions for multi-agent support, runtime API, and Telegram-based management.

## Features

- **Multi-agent**: Multiple users, each with their own rathole client on different machines
- **Telegram bot**: Register agents, create/list/kill tunnels via chat commands
- **One-time setup codes**: Easy agent onboarding — no manual config editing
- **Connection approval**: Hold incoming connections, approve/deny via Telegram inline buttons
- **Per-IP caching**: Approved IPs skip future approval for the same service
- **DNS server**: Auto-resolves `<tunnel>.tun.yourdomain.com` to your server
- **Idle cleanup**: Auto-removes tunnels after configurable inactivity timeout

## Architecture

RatTunnel consists of two services running on the server, plus lightweight agents on user machines.

### Components

| Component | Role |
|-----------|------|
| **rathole** (forked) | Reverse proxy server handling tunnel connections. Binds tunnel ports, manages agent gateways, holds pending connections for approval, and exposes a REST API for runtime management. |
| **qtun-controller** | Telegram bot + DNS server + webhook handler. Orchestrates tunnel lifecycle: receives user commands via Telegram, calls the rathole API, resolves DNS for tunnel subdomains, and handles connection approval webhooks. |

### How They Fit Together

```
                        Server
                 +-----------------------+
                 |  rathole    (:2333)    |   <-- agents connect here
                 |  API        (:9090)    |   <-- qtun-controller calls this
                 |  qtun-controller       |
                 |    - Telegram bot      |   <-- users interact here
                 |    - DNS      (:53)    |   <-- resolves tunnel subdomains
                 |    - Webhook  (:8090)  |   <-- rathole sends approval requests here
                 +-----------------------+
                    /         |         \
           Agent: home    Agent: lab   Agent: office
           (Alice)        (Alice)      (Bob)
```

- Both services run with `network_mode: host` in Docker Compose.
- qtun-controller talks to rathole's REST API on `127.0.0.1:9090`.
- rathole sends connection approval webhooks to qtun-controller on `127.0.0.1:8090`.
- Agents (rathole clients) connect to the server on port `2333`.

## User Flow

### 1. Register an Agent

User messages the Telegram bot:

```
/register home-pc
```

Behind the scenes:
- Bot generates a unique `agent_id` and `token`
- Bot calls rathole API to register the agent gateway (`PUT /api/v1/agents/:agent_id`)
- Bot creates a one-time setup code (`POST /api/v1/setup`)
- Bot replies with the setup code and instructions

### 2. Set Up the Agent

User runs the rathole client on their machine:

```bash
rathole --setup server-ip:9090
```

When prompted, they enter the setup code. The client:
- Calls `GET /api/v1/setup/:code` to claim the code (no auth required)
- Receives the server address, token, and agent ID
- Writes a local config file and starts connecting

### 3. Create Tunnels

User creates tunnels via Telegram:

```
/create home-pc 192.168.1.100:22
```

The bot calls the rathole API to add a service. The server binds the port and pushes the tunnel config to the agent in real time — no agent restart needed.

### 4. Connection Approval

When someone connects to a tunnel port:
1. Rathole holds the TCP connection
2. Rathole sends a webhook to qtun-controller with the visitor's IP
3. qtun-controller sends a Telegram notification with **Approve** / **Deny** buttons
4. User taps a button; qtun-controller calls the rathole approval API
5. The connection is forwarded (approved) or dropped (denied)

### 5. Per-IP Caching

Once an IP is approved for a service, subsequent connections from the same IP are auto-approved. The cache is cleared when the service is removed.

## Telegram Bot Commands

| Command | Description |
|---------|-------------|
| `/register <name>` | Register a new agent (one per machine, multiple per user) |
| `/agents` | List your registered agents with connection status |
| `/unregister <name>` | Remove an agent and all its tunnels |
| `/create <agent> <target:port> [port]` | Create a tunnel on a specific agent. Optional `port` specifies the server-side listen port (auto-assigned from range if omitted). |
| `/list` | List your tunnels across all agents |
| `/kill <name>` | Kill (remove) a tunnel |

## REST API Reference (rathole)

The rathole server exposes a REST API on the address configured in `[api].bind_addr`.

### Authentication

Two-tier authorization model:

| Role | Token Source | Access |
|------|-------------|--------|
| **Admin** | `[api].token` in server.toml | Full access to all endpoints and all agents' resources |
| **Agent** | Generated during `/register`, stored in rathole memory | Scoped to own resources only — can only see/modify own services and pending connections |

All authenticated requests require the header:
```
Authorization: Bearer <token>
```

### Endpoints

#### Anonymous (no auth required)

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/setup/:code` | Claim a setup code. Returns client config (server address, token, agent_id). Single-use; code is deleted after claim. Returns `404` if invalid/expired. |

#### Admin Only

| Method | Path | Description |
|--------|------|-------------|
| `PUT` | `/api/v1/agents/:agent_id` | Register an agent. Body: `{"token": "..."}`. Creates the agent's gateway service. |
| `DELETE` | `/api/v1/agents/:agent_id` | Unregister an agent and delete all services owned by it. |
| `GET` | `/api/v1/agents` | List all registered agents with connection state. |
| `POST` | `/api/v1/setup` | Create a one-time setup code. Body: `{"agent_id": "...", "token": "...", "setup_code": "..."}`. Expires after 10 minutes. |

#### Admin or Agent-Scoped

Agents authenticate with their own token and can only see/modify their own resources. Admin token grants access to all resources.

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/services` | List services. Agent sees only own services. |
| `GET` | `/api/v1/services/:name` | Get a single service by name. |
| `PUT` | `/api/v1/services/:name` | Add or update a service. Body: `{"bind_addr": "...", "local_addr": "...", ...}`. |
| `DELETE` | `/api/v1/services/:name` | Remove a service. Port is released, client tears down forwarding. |
| `GET` | `/api/v1/pending` | List pending (held) connections awaiting approval. |
| `POST` | `/api/v1/pending/:id/approve` | Approve a pending connection. Visitor's TCP stream is forwarded immediately. |
| `POST` | `/api/v1/pending/:id/deny` | Deny a pending connection. Visitor's TCP stream is dropped. |

#### PUT /api/v1/services/:name Body Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `bind_addr` | string | yes | Server-side listen address (e.g. `"0.0.0.0:5022"`) |
| `local_addr` | string | yes | Client-side forward target (e.g. `"192.168.1.100:22"`) |
| `token` | string | no | Per-service auth token. Uses `default_token` if omitted |
| `type` | string | no | `"tcp"` (default) or `"udp"` |
| `nodelay` | bool | no | Enable `TCP_NODELAY` |
| `require_approval` | bool | no | Hold incoming connections until approved via webhook |
| `agent_id` | string | no | Assign service to a specific agent |

#### Error Responses

| Status | Body | Cause |
|--------|------|-------|
| `400` | `{"error": "..."}` | Invalid JSON, missing fields, or invalid parameters |
| `401` | `{"error": "unauthorized"}` | Missing or invalid bearer token |
| `404` | `{"error": "not found"}` | Unknown endpoint, service, or pending connection |

#### Service States

| State | Meaning |
|-------|---------|
| `Registered` | Config added, waiting for client to connect |
| `Active` | Client connected, tunnel is live |
| `Disconnected` | Client was connected but dropped off |

## Authorization Model

- **Admin token**: Configured in `[api].token` in `server.toml`. Has full access to all API endpoints and can manage any agent's resources.
- **Agent tokens**: Generated by qtun-controller during `/register` and passed to rathole via `PUT /api/v1/agents/:agent_id`. Stored in rathole's in-memory agent registry.
- **Scope isolation**: An agent token only grants access to services and pending connections that belong to that agent. Agents cannot see or modify other agents' resources.
- **Setup codes**: Ephemeral, single-use, unauthenticated. Allow a new agent to claim its config without needing the admin token. Expire after 10 minutes.

## Server Setup

### Prerequisites

- Docker and Docker Compose
- A domain with DNS pointing to your server (for tunnel subdomains)
- A Telegram bot token (from [@BotFather](https://t.me/BotFather))

### 1. Clone and Configure

```bash
git clone https://github.com/chat812/rattunnel.git
cd rattunnel

# Create config files from examples
cp rathole/run/server.example.toml rathole/run/server.toml
cp qtun-controller/config.example.toml qtun-controller/config.toml
```

Edit both config files with your values (see [Configuration Reference](#configuration-reference) below).

### 2. Start Services

```bash
docker compose up -d --build
```

Both `rathole-server` and `qtun-controller` run with `network_mode: host`, so they bind directly to the host's network interfaces.

### 3. DNS Setup

Point a wildcard DNS record to your server:

```
*.tun.yourdomain.com  →  your-server-ip
```

Or let qtun-controller's built-in DNS server handle it (listens on port 53). In that case, set an NS record for `tun.yourdomain.com` pointing to your server.

## Agent Setup

### Option A: One-liner Install

Builds from source, runs setup, and installs as a systemd service:

```bash
sudo bash -c "$(curl -sSL https://raw.githubusercontent.com/chat812/rattunnel/main/agent-install.sh)" -- your-server-ip:9090
```

You will be prompted for the setup code from `/register`.

### Option B: Manual Setup

1. Build or download the rathole binary
2. Run setup:

```bash
./rathole --setup your-server-ip:9090
# Enter setup code when prompted
```

3. The client auto-configures and connects.

### systemd Service

The install script creates a systemd service. If setting up manually:

```ini
[Unit]
Description=RatTunnel Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/rattunnel/rathole /opt/rattunnel/client.toml
WorkingDirectory=/opt/rattunnel
Restart=always
RestartSec=5
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/rattunnel

[Install]
WantedBy=multi-user.target
```

Management commands:

```bash
systemctl status rattunnel-agent
systemctl restart rattunnel-agent
journalctl -u rattunnel-agent -f
```

## Configuration Reference

### rathole server.toml

```toml
[server]
bind_addr = "0.0.0.0:2333"        # Port agents connect to
default_token = "your-token"       # Shared secret for tunnel auth

[api]
bind_addr = "0.0.0.0:9090"        # REST API listen address
token = "admin-api-token"          # Admin bearer token for API auth
port_range_min = 10000             # Minimum port for auto-assigned tunnels
port_range_max = 14000             # Maximum port for auto-assigned tunnels
approval_webhook = "http://127.0.0.1:8090/webhook/connection"  # Where to send approval requests
approval_timeout = 60              # Seconds to wait for approval before dropping connection
```

### qtun-controller config.toml

```toml
telegram_bot_token = "BOT_TOKEN"           # From @BotFather
server_ip = "1.2.3.4"                      # Public IP of this server
domain = "tun.example.com"                 # Base domain for tunnel subdomains
rathole_api = "http://127.0.0.1:9090"      # rathole API address
rathole_api_token = "admin-api-token"       # Must match [api].token in server.toml
db_path = "tunnels.db"                      # SQLite database path
port_min = 10000                            # Must match rathole port_range_min
port_max = 14000                            # Must match rathole port_range_max
log_level = "info"                          # Log level: debug, info, warn, error
idle_timeout_secs = 3600                    # Remove tunnels after N seconds of inactivity
webhook_listen_addr = "0.0.0.0:8090"        # Webhook server for connection approval
```

## Project Structure

```
rattunnel/
├── docker-compose.yml          # Runs both services with network_mode: host
├── agent-install.sh            # One-liner agent installer
├── build.sh                    # Build script for all binaries
├── rathole/                    # Forked rathole with multi-agent + approval
│   ├── src/
│   │   ├── server.rs           # Agent-aware push logic
│   │   ├── api.rs              # REST API + agent + setup endpoints
│   │   ├── pending.rs          # Connection approval + IP caching
│   │   ├── protocol.rs         # Gateway name helpers
│   │   └── client.rs           # Per-agent gateway + setup mode
│   ├── run/
│   │   └── server.example.toml # Server config template
│   ├── docs/                   # Detailed documentation
│   └── Dockerfile
└── qtun-controller/            # Telegram bot + DNS + cleanup
    ├── src/
    │   ├── bot.rs              # Telegram commands
    │   ├── webhook.rs          # Approval webhook server
    │   ├── dns.rs              # DNS server
    │   ├── db.rs               # SQLite (agents + tunnels)
    │   └── cleanup.rs          # Idle tunnel cleanup
    ├── config.example.toml     # Controller config template
    └── Dockerfile
```

## Further Documentation

- [Multi-Agent Guide](rathole/docs/multi-agent.md)
- [REST API Reference](rathole/docs/rest-api.md)
- [Connection Approval](rathole/docs/connection-approval.md)

## License

Rathole is licensed under [Apache-2.0](rathole/LICENSE).
