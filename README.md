# RatTunnel

Multi-agent reverse tunnel manager with Telegram bot control and connection approval.

Built on [rathole](https://github.com/rapiz1/rathole) with extensions for multi-agent support, runtime API, and Telegram-based management.

## Features

- **Multi-agent**: Multiple users, each with their own rathole client on different machines
- **Telegram bot**: Register agents, create/list/kill tunnels via chat commands
- **One-time setup codes**: Easy agent onboarding — no manual config editing
- **Connection approval**: Hold incoming connections, approve/deny via Telegram inline buttons
- **Per-IP caching**: Approved IPs skip future approval for the same service
- **DNS server**: Auto-resolves `<tunnel>.tun.yourdomain.com` to your server
- **Idle cleanup**: Auto-removes tunnels after configurable inactivity timeout

## Architecture

```
                        Server
                 +-----------------------+
                 |  rathole    (:2333)    |
                 |  API        (:9090)    |
                 |  qtun-controller       |
                 |    - Telegram bot      |
                 |    - DNS      (:53)    |
                 |    - Webhook  (:8090)  |
                 +-----------------------+
                    /         |         \
           Agent: home    Agent: lab   Agent: office
           (Alice)        (Alice)      (Bob)
```

## Quick Start

### 1. Configure

```bash
cp rathole/run/server.example.toml rathole/run/server.toml
cp qtun-controller/config.example.toml qtun-controller/config.toml
```

Edit both files with your values (server IP, Telegram bot token, domain, etc.)

### 2. Start the server

```bash
docker compose up -d --build
```

Both `rathole-server` and `qtun-controller` run with `network_mode: host`.

### 3. Register an agent

In Telegram, message your bot:

```
/register home-pc
```

The bot replies with a setup code:

```
✅ Agent registered!

📋 Name: home-pc
🔑 Setup code: ABCD-1234
⏳ Expires: 10 minutes

💻 On your machine, run:
rathole --setup 1.2.3.4:9090
Then enter the setup code when prompted.
```

### 4. Set up the agent client

On your machine:

```bash
./rathole --setup your-server-ip:9090
# Enter setup code: ABCD-1234
# Config saved, client starts automatically
```

### 5. Create tunnels

```
/create home-pc 192.168.1.100:22
```

```
✅ Tunnel created!

🏷 Name:   a1b2c3d4
📡 Agent:  home-pc
🌐 Domain: a1b2c3d4.tun.yourdomain.com
🎯 Target: 192.168.1.100:22
🚪 Port:   10521

🔗 Connect: a1b2c3d4.tun.yourdomain.com:10521
```

### 6. Connect (with approval)

When someone connects to the tunnel, you get a Telegram notification:

```
🔔 Connection Request

🏷 Service: a1b2c3d4
🌐 Source IP: 203.0.113.50

[✅ Approve]  [❌ Deny]
```

Once approved, that IP is remembered for future connections to the same service.

## Bot Commands

| Command | Description |
|---------|-------------|
| `/register <name>` | Register a new agent (one per machine) |
| `/agents` | List your agents with connection status |
| `/create <agent> <target:port> [port]` | Create a tunnel on a specific agent |
| `/list` | List your tunnels across all agents |
| `/kill <name>` | Kill one of your tunnels |
| `/unregister <name>` | Remove agent and all its tunnels |

## Documentation

- [Multi-Agent Guide](rathole/docs/multi-agent.md)
- [REST API Reference](rathole/docs/rest-api.md)
- [Connection Approval](rathole/docs/connection-approval.md)

## Project Structure

```
rattunnel/
├── docker-compose.yml          # Runs both services
├── rathole/                    # Modified rathole with multi-agent + approval
│   ├── src/
│   │   ├── server.rs           # Agent-aware push logic
│   │   ├── api.rs              # REST API + agent + setup endpoints
│   │   ├── pending.rs          # Connection approval + IP caching
│   │   ├── protocol.rs         # Gateway name helpers
│   │   └── client.rs           # Per-agent gateway + setup mode
│   ├── run/
│   │   └── server.example.toml # Server config template
│   └── docs/
└── qtun-controller/            # Telegram bot + DNS + cleanup
    ├── src/
    │   ├── bot.rs              # Telegram commands
    │   ├── webhook.rs          # Approval webhook server
    │   ├── dns.rs              # DNS server
    │   ├── db.rs               # SQLite (agents + tunnels)
    │   └── cleanup.rs          # Idle tunnel cleanup
    └── config.example.toml     # Controller config template
```

## License

Rathole is licensed under [Apache-2.0](rathole/LICENSE).
