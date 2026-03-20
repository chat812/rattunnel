use std::collections::HashMap;
use std::sync::Arc;

use rand::Rng;
use teloxide::{prelude::*, utils::command::BotCommands};

use crate::config::Config;
use crate::db::Db;
use crate::port::kill_listener;
use crate::rathole::RatholeClient;

#[derive(BotCommands, Clone)]
#[command(rename_rule = "lowercase", description = "QuickTun — tunnel controller")]
pub enum Cmd {
    #[command(description = "<name> — Register a new agent")]
    Register(String),
    #[command(description = "List your registered agents")]
    Agents,
    #[command(description = "<name> — Unregister an agent")]
    Unregister(String),
    #[command(description = "<agent> <target:port> [listen_port] — Create a tunnel")]
    Create(String),
    #[command(description = "List your tunnels")]
    List,
    #[command(description = "<name> — Kill a tunnel")]
    Kill(String),
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

async fn answer(
    bot: Bot,
    msg: Message,
    cmd: Cmd,
    db: Arc<Db>,
    rathole: Arc<RatholeClient>,
    cfg: Arc<Config>,
) -> ResponseResult<()> {
    match cmd {
        Cmd::Register(args) => {
            let agent_name = args.trim().to_string();
            if agent_name.is_empty() {
                bot.send_message(msg.chat.id, "Usage: /register &lt;name&gt;")
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
                bot.send_message(msg.chat.id, "Usage: /unregister &lt;name&gt;")
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
            let parts: Vec<&str> = args.trim().splitn(3, ' ').collect();
            if parts.len() < 2 || parts[0].is_empty() || parts[1].is_empty() {
                bot.send_message(
                    msg.chat.id,
                    "Usage: /create &lt;agent_name&gt; &lt;target_ip:port&gt; [listen_port]",
                )
                .parse_mode(teloxide::types::ParseMode::Html)
                .await?;
                return Ok(());
            }

            let agent_name = parts[0];
            let target = parts[1];

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

            let listen_port = if parts.len() > 2 && !parts[2].trim().is_empty() {
                match parts[2].trim().parse::<u16>() {
                    Ok(p) => p,
                    Err(_) => {
                        bot.send_message(msg.chat.id, "Invalid listen port number.")
                            .await?;
                        return Ok(());
                    }
                }
            } else {
                gen_port(&db, &cfg)
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

            if let Err(e) = db.insert(&name, &subdomain, target, listen_port, msg.chat.id.0, Some(&agent.agent_id)) {
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

            let text = format!(
                "\u{2705} <b>Tunnel created!</b>\n\n\
                 \u{1f3f7} <b>Name:</b>   <code>{name}</code>\n\
                 \u{1f4e1} <b>Agent:</b>  <code>{agent_name}</code>\n\
                 \u{1f310} <b>Domain:</b> <code>{subdomain}</code>\n\
                 \u{1f3af} <b>Target:</b> <code>{target}</code>\n\
                 \u{1f6aa} <b>Port:</b>   <code>{listen_port}</code>\n\n\
                 \u{1f517} <b>Connect:</b> <code>{subdomain}:{listen_port}</code>",
                name = escape_html(&name),
                agent_name = escape_html(agent_name),
                subdomain = escape_html(&subdomain),
                target = escape_html(target),
                listen_port = listen_port,
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
                let state = states.get(&t.name).map(|s| s.as_str()).unwrap_or("Unknown");
                let icon = match state {
                    "Active" => "\u{1f7e2}",
                    "Registered" => "\u{1f7e1}",
                    _ => "\u{1f534}",
                };
                let agent_label = t.agent_id.as_ref()
                    .and_then(|id| agent_names.get(id).map(|n| n.as_str()))
                    .unwrap_or("-");
                text.push_str(&format!(
                    "{} <b>{}</b>  <i>{}</i>\n\
                     \u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{1f4e1} {} \u{2192} \u{1f3af} <code>{}</code>\n\
                     \u{00a0}\u{00a0}\u{00a0}\u{00a0}\u{1f517} <code>{}:{}</code>\n\n",
                    icon,
                    escape_html(&t.name),
                    escape_html(state),
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
                bot.send_message(msg.chat.id, "Usage: /kill &lt;name&gt;")
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
    }

    Ok(())
}

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    rathole: Arc<RatholeClient>,
) -> ResponseResult<()> {
    let data = match q.data {
        Some(d) => d,
        None => return Ok(()),
    };

    let (action, id) = match data.split_once(':') {
        Some(pair) => pair,
        None => return Ok(()),
    };

    let result = match action {
        "approve" => rathole.approve_connection(id).await,
        "deny" => rathole.deny_connection(id).await,
        _ => return Ok(()),
    };

    // Answer the callback query (removes loading spinner)
    let answer_text = match &result {
        Ok(()) if action == "approve" => "Connection approved",
        Ok(()) => "Connection denied",
        Err(_) => "Failed (connection may have timed out)",
    };
    let _ = bot.answer_callback_query(&q.id).text(answer_text).await;

    // Edit the original message to show the decision
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
            .parse_mode(teloxide::types::ParseMode::Html)
            .await;
        // Remove the inline keyboard
        let _ = bot
            .edit_message_reply_markup(msg.chat.id, msg.id)
            .await;
    }

    Ok(())
}

pub async fn run_bot(
    db: Arc<Db>,
    rathole: Arc<RatholeClient>,
    cfg: Arc<Config>,
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
        .dependencies(dptree::deps![db, rathole, cfg])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
