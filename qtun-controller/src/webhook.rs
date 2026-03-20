use axum::{
    extract::{ConnectInfo, Path, State},
    http::{StatusCode, HeaderMap, header},
    response::IntoResponse,
    Json, Router,
    routing::{get, post},
};
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup, ParseMode};

use crate::db::Db;
use crate::download::{AccessResult, DownloadTokenStore};

#[derive(Deserialize)]
pub struct WebhookPayload {
    pub id: String,
    pub service_name: String,
    pub visitor_addr: String,
}

pub struct WebhookState {
    pub bot: Bot,
    pub db: Arc<Db>,
    pub agent_binaries_dir: Option<String>,
    pub dl_tokens: Arc<DownloadTokenStore>,
    pub domain: String,
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

/// Map user-facing arch names to binary filenames in the agents directory.
fn resolve_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "x64" | "amd64" => Some("x86_64"),
        "i686" | "x86" | "i386" => Some("i686"),
        "aarch64" | "arm64" => Some("aarch64"),
        "armv7" | "arm" | "arm32" | "armhf" => Some("armv7"),
        _ => None,
    }
}

/// Extract the download token from the Host header subdomain.
/// e.g. Host: "a1b2c3d4.tun.example.com:8090" with domain "tun.example.com"
///      → Some("a1b2c3d4")
fn extract_token_from_host(headers: &HeaderMap, domain: &str) -> Option<String> {
    let host = headers.get(header::HOST)?.to_str().ok()?;
    // Strip port if present
    let host_no_port = host.split(':').next().unwrap_or(host);
    // Expect "{token}.{domain}"
    let suffix = format!(".{}", domain);
    if host_no_port.ends_with(&suffix) {
        let token = &host_no_port[..host_no_port.len() - suffix.len()];
        if !token.is_empty() && !token.contains('.') {
            return Some(token.to_string());
        }
    }
    None
}

async fn serve_binary(state: &WebhookState, arch: &str) -> Result<([(header::HeaderName, String); 2], Vec<u8>), StatusCode> {
    let dir = state.agent_binaries_dir.as_deref().ok_or(StatusCode::NOT_FOUND)?;
    let filename = resolve_arch(arch).ok_or(StatusCode::BAD_REQUEST)?;
    let path = std::path::Path::new(dir).join(filename);
    let bytes = tokio::fs::read(&path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream".to_string()),
        (header::CONTENT_DISPOSITION, format!("attachment; filename=\"rathole-agent-{}\"", filename)),
    ];
    Ok((headers, bytes))
}

async fn handle_download(
    State(state): State<Arc<WebhookState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Path(arch): Path<String>,
) -> impl IntoResponse {
    let token = match extract_token_from_host(&headers, &state.domain) {
        Some(t) => t,
        None => return Err(StatusCode::BAD_REQUEST),
    };

    let ip = addr.ip();

    match state.dl_tokens.check_access(&token, ip) {
        AccessResult::Approved => {
            serve_binary(&state, &arch).await
        }
        AccessResult::NeedApproval(chat_id) => {
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                InlineKeyboardButton::callback(
                    "\u{2705} Approve",
                    format!("dl_approve:{}:{}", token, ip),
                ),
                InlineKeyboardButton::callback(
                    "\u{274c} Deny",
                    format!("dl_deny:{}:{}", token, ip),
                ),
            ]]);

            let text = format!(
                "\u{1f4e5} <b>Download Request</b>\n\n\
                 \u{1f310} <b>Source IP:</b> <code>{}</code>\n\
                 \u{1f4c2} <b>File:</b> <code>rathole-agent-{}</code>\n",
                ip, arch,
            );

            let _ = state.bot
                .send_message(ChatId(chat_id), text)
                .parse_mode(ParseMode::Html)
                .reply_markup(keyboard)
                .await;

            Err(StatusCode::FORBIDDEN)
        }
        AccessResult::Pending => {
            Err(StatusCode::FORBIDDEN)
        }
        AccessResult::Denied => {
            Err(StatusCode::FORBIDDEN)
        }
    }
}

pub async fn run_webhook_server(
    bind_addr: &str,
    state: Arc<WebhookState>,
) -> anyhow::Result<()> {
    let app = Router::new()
        .route("/webhook/connection", post(handle_webhook))
        .route("/:arch", get(handle_download));

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    log::info!("Webhook server listening at {}", bind_addr);
    axum::serve(
        listener,
        app.with_state(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
