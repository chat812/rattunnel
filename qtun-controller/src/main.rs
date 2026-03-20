use std::sync::Arc;

mod bot;
mod cleanup;
mod config;
mod db;
mod dns;
mod port;
mod rathole;
mod webhook;

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

    // DNS server in background
    {
        let dns_db = db.clone();
        let dns_cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = dns::run_dns_server(dns_db, dns_cfg).await {
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
    bot::run_bot(db, rathole, cfg).await?;

    Ok(())
}
