use std::sync::Arc;

mod bot;
mod cleanup;
mod config;
mod db;
mod dns;
mod download;
mod port;
mod rathole;
mod webhook;

async fn restore_state(db: &db::Db, rathole: &rathole::RatholeClient) {
    // Re-register all agents
    let agents = db.all_agents().unwrap_or_default();
    for agent in &agents {
        if let Err(e) = rathole.register_agent(&agent.agent_id, &agent.token).await {
            log::warn!("Failed to restore agent '{}' ({}): {}", agent.name, agent.agent_id, e);
        }
    }
    if !agents.is_empty() {
        log::info!("Restored {} agent(s)", agents.len());
    }

    // Re-register active tunnels (skip idle persistent ones)
    let tunnels = db.all().unwrap_or_default();
    let mut restored = 0;
    let mut skipped_idle = 0;
    for t in &tunnels {
        if t.status == "idle" {
            skipped_idle += 1;
            continue;
        }
        let bind_addr = format!("0.0.0.0:{}", t.listen_port);
        if let Err(e) = rathole.add(&t.name, &bind_addr, &t.target, true, t.agent_id.as_deref()).await {
            log::warn!("Failed to restore tunnel '{}': {}", t.name, e);
        }
        restored += 1;
    }
    if restored > 0 || skipped_idle > 0 {
        log::info!("Restored {} tunnel(s), {} idle persistent tunnel(s) skipped", restored, skipped_idle);
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // --config <path>  (default: config.toml next to the binary)
    let config_path = {
        let mut args = std::env::args().skip(1);
        let mut path = std::path::PathBuf::from("config.toml");
        while let Some(arg) = args.next() {
            if arg == "--config" {
                if let Some(p) = args.next() {
                    path = std::path::PathBuf::from(p);
                }
            }
        }
        path
    };

    let cfg = config::Config::load(&config_path)?;

    // Set log level from config (RUST_LOG env takes precedence if set)
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", &cfg.log_level);
    }
    pretty_env_logger::init();

    log::info!("Loaded config from {}", config_path.display());
    log::info!("Domain: {} → {}", cfg.domain, cfg.server_ip);
    log::info!("Port range: {}–{}", cfg.port_min, cfg.port_max);

    let cfg = Arc::new(cfg);
    let db = Arc::new(db::Db::new(&cfg.db_path)?);
    let rathole = Arc::new(rathole::RatholeClient::new(&cfg.rathole_api));
    let dl_tokens = Arc::new(download::DownloadTokenStore::new());

    // Restore agents and tunnels from DB into rathole
    restore_state(&db, &rathole).await;

    // DNS server in background
    {
        let dns_cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = dns::run_dns_server(dns_cfg).await {
                log::error!("DNS server stopped: {}", e);
            }
        });
    }

    // Idle tunnel cleanup in background
    {
        let c_db = db.clone();
        let c_rat = rathole.clone();
        let c_cfg = cfg.clone();
        tokio::spawn(async move {
            cleanup::run_cleanup(c_db, c_rat, c_cfg).await;
        });
    }

    log::info!("Idle timeout: {}s — checking every 60s", cfg.idle_timeout_secs);

    // Webhook server for connection approval notifications
    if let Some(ref webhook_addr) = cfg.webhook_listen_addr {
        let webhook_state = Arc::new(webhook::WebhookState {
            bot: teloxide::Bot::new(&cfg.telegram_bot_token),
            db: db.clone(),
            agent_binaries_dir: cfg.agent_binaries_dir.clone(),
            dl_tokens: dl_tokens.clone(),
            domain: cfg.domain.clone(),
        });
        let addr = webhook_addr.clone();
        tokio::spawn(async move {
            if let Err(e) = webhook::run_webhook_server(&addr, webhook_state).await {
                log::error!("Webhook server stopped: {}", e);
            }
        });
        log::info!("Approval webhook enabled");
    }

    // Telegram bot (blocks until Ctrl-C)
    bot::run_bot(db, rathole, cfg, dl_tokens).await?;

    Ok(())
}
