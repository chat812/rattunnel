use std::collections::HashMap;
use std::sync::Arc;

use rand::Rng;
use teloxide::{prelude::*, types::ParseMode, utils::command::BotCommands};

use crate::config::Config;
use crate::db::Db;
use crate::download::DownloadTokenStore;
use crate::port::kill_listener;
use crate::rathole::RatholeClient;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "QuickTun — tunnel controller")]
pub enum Cmd {
    #[command(description = "<agent_name> — Register a new agent")]
    Register(String),
    #[command(description = "List your registered agents")]
    Agents,
    #[command(description = "<agent_name> — Unregister an agent and its tunnels")]
    Unregister(String),
    #[command(description = "<agent> <ip:port> [port] [persist] — Create tunnel")]
    Create(String),
    #[command(description = "List your tunnels with status")]
    List,
    #[command(description = "<tunnel_name> — Remove a tunnel")]
    Kill(String),
    #[command(description = "<tunnel_name> — Resume an idle persistent tunnel")]
    Activate(String),
    #[command(description = "Get time-limited agent download links")]
    Download,
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn gen_name(db: &Db) -> String {
    let mut rng = rand::thread_rng();
    loop {
        let name: String = (0..8)
            .map(|_| {
                let idx = rng.gen_range(0u8..36);
                if idx < 10 {
                    (b'0' + idx) as char
                } else {
                    (b'a' + idx - 10) as char
                }
            })
            .collect();
        if !db.name_in_use(&name).unwrap_or(true) {
            return name;
        }
    }
}

fn gen_port(db: &Db, cfg: &Config) -> u16 {
    let mut rng = rand::thread_rng();
    loop {
        let port = rng.gen_range(cfg.port_min..=cfg.port_max);
        if !db.port_in_use(port).unwrap_or(true) {
            return port;
        }
    }
}

fn gen_token() -> String {
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| {
            let idx = rng.gen_range(0u8..36);
            if idx < 10 { (b'0' + idx) as char } else { (b'a' + idx - 10) as char }
        })
        .collect()
}

fn gen_setup_code() -> String {
    let mut rng = rand::thread_rng();
    let part1: String = (0..4).map(|_| (b'A' + rng.gen_range(0u8..26)) as char).collect();
    let part2: String = (0..4).map(|_| (b'0' + rng.gen_range(0u8..10)) as char).collect();
    format!("{}-{}", part1, part2)
}

fn format_download_links(domain: &str, token: &str, binaries_dir: Option<&str>) -> String {
    let base_url = format!("http://{}.{}:8090", token, domain);
    let archs = [
        ("x86_64", "x86_64 (64-bit Intel/AMD)"),
        ("i686", "x86 (32-bit Intel/AMD)"),
        ("aarch64", "ARM64 (aarch64)"),
        ("armv7", "ARM32 (armv7/armhf)"),
    ];

    let mut lines = Vec::new();
    for (arch, label) in &archs {
        let available = binaries_dir
            .map(|d| std::path::Path::new(d).join(arch).exists())
            .unwrap_or(false);
        if available {
            lines.push(format!(
                "\u{2705} <b>{}</b>\n<code>curl -sSL {}/{} -o rathole &amp;&amp; chmod +x rathole</code>",
                label, base_url, arch,
            ));
        }
    }

    if lines.is_empty() {
        "No agent binaries available. Run <code>./build.sh</code> on the server first.".to_string()
    } else {
        format!(
            "\u{1f4e6} <b>Agent Download Links</b>\n\n{}\n\n\
             \u{1f680} <b>Quick install (auto-detect arch):</b>\n\
             <code>curl -sSL {}/$(uname -m) -o rathole &amp;&amp; chmod +x rathole</code>",
            lines.join("\n\n"),
            base_url,
        )
    }
}

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: Cmd,
    db: Arc<Db>,
    rathole: Arc<RatholeClient>,
    cfg: Arc<Config>,
    dl_tokens: Arc<DownloadTokenStore>,
) -> ResponseResult<()> {
    match cmd {
        Cmd::Register(args) => {
            let agent_name = args.trim().to_string();
            if agent_name.is_empty() {
                bot.send_message(msg.chat.id, "Usage: /register &lt;agent_name&gt;")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            // Check if user already has an agent with this name
            if let Ok(Some(_)) = db.find_agent_by_name(msg.chat.id.0, &agent_name) {
                bot.send_message(
                    msg.chat.id,
                    format!("You already have an agent named <code>{}</code>.", escape_html(&agent_name)),
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
                return Ok(());
            }

            let agent_id = gen_name(&db); // reuse 8-char random generator
            let token = gen_token();
            let setup_code = gen_setup_code();

            // Register gateway service on rathole
            if let Err(e) = rathole.register_agent(&agent_id, &token).await {
                bot.send_message(msg.chat.id, format!("Failed to register agent: {}", escape_html(&e.to_string())))
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            // Create setup code on rathole
            let remote_addr = format!("{}:2333", cfg.server_ip);
            if let Err(e) = rathole.create_setup_code(&agent_id, &token, &setup_code, &remote_addr).await {
                bot.send_message(msg.chat.id, format!("Failed to create setup code: {}", escape_html(&e.to_string())))
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            // Store in DB
            if let Err(e) = db.insert_agent(&agent_id, &token, msg.chat.id.0, &agent_name) {
                bot.send_message(msg.chat.id, format!("Database error: {}", escape_html(&e.to_string())))
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            let text = format!(
                "\u{2705} <b>Agent registered!</b>\n\n\
                 \u{1f4cb} <b>Name:</b> <code>{}</code>\n\
                 \u{1f511} <b>Setup code:</b> <code>{}</code>\n\
                 \u{23f3} <b>Expires:</b> 10 minutes\n\n\
                 \u{1f4bb} On your machine, run:\n\
                 <code>rathole --setup {}:9090</code>\n\
                 Then enter the setup code when prompted.",
                escape_html(&agent_name),
                escape_html(&setup_code),
                escape_html(&cfg.server_ip),
            );
            bot.send_message(msg.chat.id, text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }

        Cmd::Agents => {
            let agents = db.agents_by_chat_id(msg.chat.id.0).unwrap_or_default();
            if agents.is_empty() {
                bot.send_message(msg.chat.id, "No agents registered.\nUse /register &lt;name&gt; to add one.")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            let states: HashMap<String, String> = rathole
                .list_agents()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| (s.name, s.state))
                .collect();

            let mut text = String::from("\u{1f4e1} <b>Your Agents</b>\n\n");
            for a in &agents {
                let gw_name = format!("__gw_{}__", a.agent_id);
                let state = states.get(&gw_name).map(|s| s.as_str()).unwrap_or("Offline");
                let icon = match state {
                    "Active" => "\u{1f7e2}",
                    "Registered" => "\u{1f7e1}",
                    _ => "\u{1f534}",
                };
                let tunnel_count = db.tunnels_by_agent(&a.agent_id).unwrap_or_default().len();
                text.push_str(&format!(
                    "{} <b>{}</b>  <i>{}</i>\n    Tunnels: {}\n\n",
                    icon,
                    escape_html(&a.name),
                    escape_html(state),
                    tunnel_count,
                ));
            }
            bot.send_message(msg.chat.id, text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }

        Cmd::Unregister(args) => {
            let agent_name = args.trim().to_string();
            if agent_name.is_empty() {
                bot.send_message(msg.chat.id, "Usage: /unregister &lt;agent_name&gt;")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            match db.find_agent_by_name(msg.chat.id.0, &agent_name) {
                Ok(Some(agent)) => {
                    // Kill all tunnels for this agent
                    let tunnels = db.tunnels_by_agent(&agent.agent_id).unwrap_or_default();
                    for t in &tunnels {
                        let _ = rathole.remove(&t.name).await;
                        let _ = db.delete(&t.name);
                        kill_listener(t.listen_port);
                    }

                    // Unregister from rathole
                    let _ = rathole.unregister_agent(&agent.agent_id).await;
                    let _ = db.delete_agent(&agent.agent_id);

                    let text = format!(
                        "\u{274c} <b>Agent unregistered</b>\n\n\
                         \u{1f4cb} <b>Name:</b> <code>{}</code>\n\
                         \u{1f5d1} <b>Tunnels removed:</b> {}",
                        escape_html(&agent_name),
                        tunnels.len()
                    );
                    bot.send_message(msg.chat.id, text)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }
                Ok(None) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("No agent named <code>{}</code> found.", escape_html(&agent_name)),
                    )
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                }
                Err(e) => {
                    bot.send_message(msg.chat.id, format!("Database error: {}", escape_html(&e.to_string())))
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }
            }
        }

        Cmd::Create(args) => {
            let parts: Vec<&str> = args.trim().split_whitespace().collect();
            if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
                bot.send_message(
                    msg.chat.id,
                    "Usage: /create &lt;agent_name&gt; &lt;ip:port&gt; [listen_port] [persist]\n\nExample: /create myagent 192.168.1.5:22\nExample: /create myagent 10.0.0.1:3389 5022 persist",
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
                return Ok(());
            }

            let agent_name = parts[0];
            let target = parts[1];

            // Check for "persist" flag anywhere in remaining args
            let persistent = parts[2..].iter().any(|p| p.eq_ignore_ascii_case("persist"));

            // Look up agent
            let agent = match db.find_agent_by_name(msg.chat.id.0, agent_name) {
                Ok(Some(a)) => a,
                Ok(None) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("No agent named <code>{}</code>. Use /register first.", escape_html(agent_name)),
                    )
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                    return Ok(());
                }
                Err(e) => {
                    bot.send_message(msg.chat.id, format!("Database error: {}", escape_html(&e.to_string())))
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                    return Ok(());
                }
            };

            if !target.contains(':') {
                bot.send_message(msg.chat.id, "Target must be in format <code>ip:port</code>")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            // Find a numeric port in remaining args (skip "persist" keyword)
            let listen_port = parts[2..].iter()
                .filter(|p| !p.eq_ignore_ascii_case("persist"))
                .next()
                .and_then(|p| p.parse::<u16>().ok());
            let listen_port = match listen_port {
                Some(p) => p,
                None => gen_port(&db, &cfg),
            };

            if db.port_in_use(listen_port).unwrap_or(false) {
                bot.send_message(
                    msg.chat.id,
                    format!("Port {} is already in use by another tunnel.", listen_port),
                )
                .await?;
                return Ok(());
            }

            let name = gen_name(&db);
            let subdomain = format!("{}.{}", name, cfg.domain);
            let bind_addr = format!("0.0.0.0:{}", listen_port);

            if let Err(e) = rathole.add(&name, &bind_addr, target, true, Some(&agent.agent_id)).await {
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "Failed to create tunnel in rathole: {}",
                        escape_html(&e.to_string())
                    ),
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
                return Ok(());
            }

            if let Err(e) = db.insert(&name, &subdomain, target, listen_port, msg.chat.id.0, Some(&agent.agent_id), persistent) {
                let _ = rathole.remove(&name).await;
                bot.send_message(
                    msg.chat.id,
                    format!(
                        "Database error (tunnel rolled back): {}",
                        escape_html(&e.to_string())
                    ),
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
                return Ok(());
            }

            let persist_label = if persistent { "\n\u{1f4cc} <b>Mode:</b>   Persistent (idles after 30m, use /activate to resume)" } else { "" };
            let text = format!(
                "\u{2705} <b>Tunnel created!</b>\n\n\
                 \u{1f3f7} <b>Name:</b>   <code>{name}</code>\n\
                 \u{1f4e1} <b>Agent:</b>  <code>{agent_name}</code>\n\
                 \u{1f310} <b>Domain:</b> <code>{subdomain}</code>\n\
                 \u{1f3af} <b>Target:</b> <code>{target}</code>\n\
                 \u{1f6aa} <b>Port:</b>   <code>{listen_port}</code>{persist_label}\n\n\
                 \u{1f517} <b>Connect:</b> <code>{subdomain}:{listen_port}</code>",
                name = escape_html(&name),
                agent_name = escape_html(agent_name),
                subdomain = escape_html(&subdomain),
                target = escape_html(target),
                listen_port = listen_port,
                persist_label = persist_label,
            );
            bot.send_message(msg.chat.id, text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }

        Cmd::List => {
            let tunnels = db.tunnels_by_chat_id(msg.chat.id.0).unwrap_or_default();
            if tunnels.is_empty() {
                bot.send_message(msg.chat.id, "No tunnels.\nUse /create &lt;agent&gt; &lt;target:port&gt; to add one.")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            let states: HashMap<String, String> = rathole
                .list()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|s| (s.name, s.state))
                .collect();

            // Resolve agent names
            let agents = db.agents_by_chat_id(msg.chat.id.0).unwrap_or_default();
            let agent_names: HashMap<String, String> = agents.into_iter()
                .map(|a| (a.agent_id, a.name))
                .collect();

            let mut text = String::from("\u{1f310} <b>Your Tunnels</b>\n\n");
            for t in &tunnels {
                let (icon, state_label) = if t.status == "idle" {
                    ("\u{1f4a4}", "Idle")
                } else {
                    let state = states.get(&t.name).map(|s| s.as_str()).unwrap_or("Unknown");
                    let icon = match state {
                        "Active" => "\u{1f7e2}",
                        "Registered" => "\u{1f7e1}",
                        _ => "\u{1f534}",
                    };
                    (icon, state)
                };
                let agent_label = t.agent_id.as_ref()
                    .and_then(|id| agent_names.get(id).map(|n| n.as_str()))
                    .unwrap_or("-");
                let persist_tag = if t.persistent { " \u{1f4cc}" } else { "" };
                text.push_str(&format!(
                    "{} <b>{}</b>  <i>{}</i>{}\n\
                     \u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{1f4e1} {} \u{2192} \u{1f3af} <code>{}</code>\n\
                     \u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{1f517} <code>{}:{}</code>\n\n",
                    icon,
                    escape_html(&t.name),
                    escape_html(state_label),
                    persist_tag,
                    escape_html(agent_label),
                    escape_html(&t.target),
                    escape_html(&t.subdomain),
                    t.listen_port,
                ));
            }
            bot.send_message(msg.chat.id, text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }

        Cmd::Kill(arg) => {
            let name = arg.trim().to_string();
            if name.is_empty() {
                bot.send_message(msg.chat.id, "Usage: /kill &lt;tunnel_name&gt;")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            match db.find_by_name(&name) {
                Ok(Some(t)) => {
                    // Only allow killing own tunnels
                    if t.creator_chat_id != msg.chat.id.0 {
                        bot.send_message(msg.chat.id, "\u{26d4} You can only kill your own tunnels.")
                            .await?;
                        return Ok(());
                    }
                    if let Err(e) = rathole.remove(&name).await {
                        log::warn!("rathole remove error for {}: {}", name, e);
                    }
                    let _ = db.delete(&name);
                    kill_listener(t.listen_port);
                    let text = format!(
                        "\u{274c} <b>Tunnel killed</b>\n\n\
                         \u{1f3f7} <b>Name:</b> <code>{}</code>\n\
                         \u{1f3af} <b>Target was:</b> <code>{}</code>",
                        escape_html(&t.name),
                        escape_html(&t.target),
                    );
                    bot.send_message(msg.chat.id, text)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }
                Ok(None) => {
                    bot.send_message(
                        msg.chat.id,
                        format!(
                            "No tunnel named <code>{}</code> found.",
                            escape_html(&name)
                        ),
                    )
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                }
                Err(e) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("Database error: {}", escape_html(&e.to_string())),
                    )
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                }
            }
        }

        Cmd::Activate(arg) => {
            let name = arg.trim().to_string();
            if name.is_empty() {
                bot.send_message(msg.chat.id, "Usage: /activate &lt;tunnel_name&gt;")
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                return Ok(());
            }

            match db.find_by_name(&name) {
                Ok(Some(t)) => {
                    if t.creator_chat_id != msg.chat.id.0 {
                        bot.send_message(msg.chat.id, "\u{26d4} You can only activate your own tunnels.")
                            .await?;
                        return Ok(());
                    }
                    if !t.persistent {
                        bot.send_message(msg.chat.id, "This tunnel is not persistent. Only persistent tunnels can be activated.")
                            .await?;
                        return Ok(());
                    }
                    if t.status == "active" {
                        bot.send_message(msg.chat.id, "This tunnel is already active.")
                            .await?;
                        return Ok(());
                    }

                    // Re-register on rathole
                    let bind_addr = format!("0.0.0.0:{}", t.listen_port);
                    if let Err(e) = rathole.add(&t.name, &bind_addr, &t.target, true, t.agent_id.as_deref()).await {
                        bot.send_message(
                            msg.chat.id,
                            format!("Failed to reactivate tunnel: {}", escape_html(&e.to_string())),
                        )
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                        return Ok(());
                    }

                    if let Err(e) = db.set_active(&t.name) {
                        log::warn!("set_active('{}') failed: {}", t.name, e);
                    }

                    let text = format!(
                        "\u{26a1} <b>Tunnel activated!</b>\n\n\
                         \u{1f3f7} <b>Name:</b>   <code>{}</code>\n\
                         \u{1f3af} <b>Target:</b> <code>{}</code>\n\
                         \u{1f517} <b>Connect:</b> <code>{}:{}</code>",
                        escape_html(&t.name),
                        escape_html(&t.target),
                        escape_html(&t.subdomain),
                        t.listen_port,
                    );
                    bot.send_message(msg.chat.id, text)
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }
                Ok(None) => {
                    bot.send_message(
                        msg.chat.id,
                        format!("No tunnel named <code>{}</code> found.", escape_html(&name)),
                    )
                    .parse_mode(teloxide::types::ParseMode::Html)
                    .await?;
                }
                Err(e) => {
                    bot.send_message(msg.chat.id, format!("Database error: {}", escape_html(&e.to_string())))
                        .parse_mode(teloxide::types::ParseMode::Html)
                        .await?;
                }
            }
        }

        Cmd::Download => {
            let (token, remaining) = if let Some(active) = dl_tokens.find_active(msg.chat.id.0) {
                active
            } else {
                let token = dl_tokens.create(msg.chat.id.0);
                (token, 600)
            };

            let mins = remaining / 60;
            let secs = remaining % 60;
            let text = format_download_links(&cfg.domain, &token, cfg.agent_binaries_dir.as_deref());
            let text = format!(
                "{}\n\n\u{23f3} <b>Expires in:</b> {}m {}s\n\
                 \u{1f512} Each new IP will require your approval before downloading.",
                text, mins, secs,
            );
            bot.send_message(msg.chat.id, text)
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
        }
    }

    Ok(())
}

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    rathole: Arc<RatholeClient>,
    _cfg: Arc<Config>,
    dl_tokens: Arc<DownloadTokenStore>,
) -> ResponseResult<()> {
    let data = match q.data {
        Some(d) => d,
        None => return Ok(()),
    };

    let (action, id) = match data.split_once(':') {
        Some(pair) => pair,
        None => return Ok(()),
    };

    match action {
        "approve" | "deny" => {
            let result = match action {
                "approve" => rathole.approve_connection(id).await,
                _ => rathole.deny_connection(id).await,
            };

            let answer_text = match &result {
                Ok(()) if action == "approve" => "Connection approved",
                Ok(()) => "Connection denied",
                Err(_) => "Failed (connection may have timed out)",
            };
            let _ = bot.answer_callback_query(&q.id).text(answer_text).await;

            if let Some(msg) = q.message {
                let orig_text = msg.text().unwrap_or("");
                let status = if result.is_ok() {
                    if action == "approve" { "APPROVED" } else { "DENIED" }
                } else {
                    "EXPIRED"
                };
                let new_text = format!(
                    "{}\n\n<b>Status:</b> {}",
                    escape_html(orig_text),
                    status
                );
                let _ = bot
                    .edit_message_text(msg.chat.id, msg.id, new_text)
                    .parse_mode(ParseMode::Html)
                    .await;
                let _ = bot
                    .edit_message_reply_markup(msg.chat.id, msg.id)
                    .await;
            }
        }

        "dl_approve" | "dl_deny" => {
            // id format: "token:ip"
            let (token, ip_str) = match id.split_once(':') {
                Some(pair) => pair,
                None => return Ok(()),
            };
            let ip: std::net::IpAddr = match ip_str.parse() {
                Ok(ip) => ip,
                Err(_) => return Ok(()),
            };

            if action == "dl_approve" {
                dl_tokens.approve_ip(token, ip);
                let _ = bot.answer_callback_query(&q.id).text("Download approved").await;
            } else {
                dl_tokens.deny_ip(token, ip);
                let _ = bot.answer_callback_query(&q.id).text("Download denied").await;
            }

            if let Some(msg) = q.message {
                let orig_text = msg.text().unwrap_or("");
                let status = if action == "dl_approve" { "APPROVED" } else { "DENIED" };
                let new_text = format!(
                    "{}\n\n<b>Status:</b> {}",
                    escape_html(orig_text),
                    status,
                );
                let _ = bot
                    .edit_message_text(msg.chat.id, msg.id, new_text)
                    .parse_mode(ParseMode::Html)
                    .await;
                let _ = bot
                    .edit_message_reply_markup(msg.chat.id, msg.id)
                    .await;
            }
        }

        _ => {}
    }

    Ok(())
}

pub async fn run_bot(
    db: Arc<Db>,
    rathole: Arc<RatholeClient>,
    cfg: Arc<Config>,
    dl_tokens: Arc<DownloadTokenStore>,
) -> anyhow::Result<()> {
    let bot = Bot::new(&cfg.telegram_bot_token);

    bot.set_my_commands(Cmd::bot_commands()).await?;

    let command_handler = Update::filter_message()
        .filter_command::<Cmd>()
        .endpoint(answer);

    let callback_handler = Update::filter_callback_query()
        .endpoint(handle_callback);

    let handler = dptree::entry()
        .branch(command_handler)
        .branch(callback_handler);

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![db, rathole, cfg, dl_tokens])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
