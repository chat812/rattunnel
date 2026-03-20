use axum::{extract::State, http::StatusCode, Json, Router, routing::post};
use serde::Deserialize;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use crate::db::Db;

#[derive(Deserialize)]
pub struct WebhookPayload {
    pub id: String,
    pub service_name: String,
    pub visitor_addr: String,
}

pub struct WebhookState {
    pub bot: Bot,
    pub db: Arc<Db>,
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn handle_webhook(
    State(state): State<Arc<WebhookState>>,
    Json(payload): Json<WebhookPayload>,
) -> StatusCode {
    // Look up the tunnel creator's chat ID
    let chat_id = match state.db.creator_chat_id(&payload.service_name) {
        Ok(Some(id)) if id != 0 => ChatId(id),
        _ => {
            log::warn!(
                "No creator found for service '{}', skipping approval notification",
                payload.service_name
            );
            return StatusCode::OK;
        }
    };

    // Extract just the IP (without port) for cleaner display
    let ip_only = payload.visitor_addr.rsplit_once(':')
        .map(|(ip, _)| ip)
        .unwrap_or(&payload.visitor_addr);

    let keyboard = InlineKeyboardMarkup::new(vec![vec![
        InlineKeyboardButton::callback("\u{2705} Approve", format!("approve:{}", payload.id)),
        InlineKeyboardButton::callback("\u{274c} Deny", format!("deny:{}", payload.id)),
    ]]);

    let text = format!(
        "\u{1f514} <b>Connection Request</b>\n\n\
         \u{1f3f7} <b>Service:</b> <code>{}</code>\n\
         \u{1f310} <b>Source IP:</b> <code>{}</code>\n",
        escape_html(&payload.service_name),
        escape_html(ip_only),
    );

    if let Err(e) = state
        .bot
        .send_message(chat_id, text)
        .parse_mode(ParseMode::Html)
        .reply_markup(keyboard)
        .await
    {
        log::error!("Failed to send Telegram approval message: {}", e);
    }

    StatusCode::OK
}

pub async fn run_webhook_server(
    bind_addr: &str,
    state: Arc<WebhookState>,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/webhook/connection", post(handle_webhook))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    log::info!("Webhook server listening at {}", bind_addr);
    axum::serve(listener, app).await?;
    Ok(())
}
