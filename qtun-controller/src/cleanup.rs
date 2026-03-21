use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{interval, Duration};

use crate::config::Config;
use crate::db::Db;
use crate::port::{is_port_listening, kill_listener};
use crate::rathole::RatholeClient;

/// Check /proc/net/tcp and /proc/net/tcp6 for any ESTABLISHED connection
/// on the given local port. This is actual traffic — not just "client connected".
fn has_active_connections(port: u16) -> bool {
    let port_hex = format!("{:04X}", port);

    for path in &["/proc/net/tcp", "/proc/net/tcp6"] {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for line in content.lines().skip(1) {
            let mut fields = line.split_ascii_whitespace();
            let _sl = fields.next();
            let local_addr = match fields.next() { Some(v) => v, None => continue };
            let _rem_addr = fields.next();
            let state = match fields.next() { Some(v) => v, None => continue };

            // 01 = ESTABLISHED
            if state != "01" {
                continue;
            }

            // local_addr format: XXXXXXXX:PPPP
            if let Some(p) = local_addr.split(':').nth(1) {
                if p.eq_ignore_ascii_case(&port_hex) {
                    return true;
                }
            }
        }
    }

    false
}

/// Every 60 seconds:
///   - For each tunnel: if its listen port has ESTABLISHED TCP connections → touch last_active_at
///   - Remove tunnels idle longer than idle_timeout_secs
pub async fn run_cleanup(db: Arc<Db>, rathole: Arc<RatholeClient>, cfg: Arc<Config>) {
    let mut ticker = interval(Duration::from_secs(60));
    ticker.tick().await; // skip immediate first tick

    loop {
        ticker.tick().await;

        let tunnels = match db.all() {
            Ok(t) => t,
            Err(e) => {
                log::warn!("cleanup: db.all() failed: {}", e);
                continue;
            }
        };

        // Update last_active_at for tunnels with real traffic
        for t in &tunnels {
            if has_active_connections(t.listen_port) {
                log::debug!("cleanup: tunnel '{}' port {} has active connections", t.name, t.listen_port);
                if let Err(e) = db.touch_active(&t.name) {
                    log::warn!("cleanup: touch_active('{}') failed: {}", t.name, e);
                }
            }
        }

        // Remove idle non-persistent tunnels
        let idle = match db.idle_tunnels(cfg.idle_timeout_secs) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("cleanup: idle_tunnels() failed: {}", e);
                continue;
            }
        };

        for t in idle {
            log::info!(
                "cleanup: removing idle tunnel '{}' ({}) — no traffic for >{}s",
                t.name, t.target, cfg.idle_timeout_secs,
            );
            if let Err(e) = rathole.remove(&t.name).await {
                log::warn!("cleanup: rathole remove '{}' failed: {}", t.name, e);
            }
            if let Err(e) = db.delete(&t.name) {
                log::warn!("cleanup: db delete '{}' failed: {}", t.name, e);
            }
            kill_listener(t.listen_port);
        }

        // Put idle persistent tunnels to sleep (30 min idle threshold)
        let persistent_idle_secs = 1800; // 30 minutes
        let idle_persistent = match db.idle_persistent_tunnels(persistent_idle_secs) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("cleanup: idle_persistent_tunnels() failed: {}", e);
                continue;
            }
        };

        for t in idle_persistent {
            log::info!(
                "cleanup: idling persistent tunnel '{}' ({}) — no traffic for >{}s",
                t.name, t.target, persistent_idle_secs,
            );
            // Remove from rathole (stop listening) but keep in DB
            if let Err(e) = rathole.remove(&t.name).await {
                log::warn!("cleanup: rathole remove '{}' failed: {}", t.name, e);
            }
            // Clear approved IPs
            if let Err(e) = rathole.clear_approved(&t.name).await {
                log::warn!("cleanup: clear_approved '{}' failed: {}", t.name, e);
            }
            if let Err(e) = db.set_idle(&t.name) {
                log::warn!("cleanup: set_idle '{}' failed: {}", t.name, e);
            }
            kill_listener(t.listen_port);
        }
    }
}

/// Connection watcher: every 15s, check active tunnels whose ports are not listening.
/// Try to re-register them up to 3 times. If all retries fail, set status to "error".
pub async fn run_connection_watcher(db: Arc<Db>, rathole: Arc<RatholeClient>) {
    let mut ticker = interval(Duration::from_secs(15));
    ticker.tick().await; // skip immediate first tick

    // Track retry counts: tunnel name -> attempts so far
    let mut retries: HashMap<String, u8> = HashMap::new();

    loop {
        ticker.tick().await;

        let tunnels = match db.all() {
            Ok(t) => t,
            Err(e) => {
                log::warn!("watcher: db.all() failed: {}", e);
                continue;
            }
        };

        // Get live service states from rathole
        let live_states: HashMap<String, String> = rathole
            .list()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.name, s.state))
            .collect();

        // Clean up retries for tunnels that no longer exist or are idle
        retries.retain(|name, _| tunnels.iter().any(|t| t.name == *name && t.status == "active"));

        for t in &tunnels {
            // Only watch active tunnels
            if t.status != "active" {
                continue;
            }

            // Check if rathole knows about this service and it's active
            let rathole_state = live_states.get(&t.name).map(|s| s.as_str());

            // If the port is listening, everything is fine — reset retries
            if is_port_listening(t.listen_port) {
                retries.remove(&t.name);
                continue;
            }

            // Port not listening — this is a problem for an active tunnel
            let attempt = retries.entry(t.name.clone()).or_insert(0);
            *attempt += 1;

            if *attempt > 3 {
                // Already exceeded retries, set to error
                log::error!(
                    "watcher: tunnel '{}' port {} not listening after 3 retries — marking as error",
                    t.name, t.listen_port,
                );
                if let Err(e) = db.set_error(&t.name) {
                    log::warn!("watcher: set_error('{}') failed: {}", t.name, e);
                }
                retries.remove(&t.name);
                continue;
            }

            log::warn!(
                "watcher: tunnel '{}' port {} not listening (attempt {}/3) — re-registering",
                t.name, t.listen_port, attempt,
            );

            // Try to re-register on rathole
            // First remove if it exists in a bad state
            if rathole_state.is_some() {
                let _ = rathole.remove(&t.name).await;
            }

            let bind_addr = format!("0.0.0.0:{}", t.listen_port);
            if let Err(e) = rathole.add(&t.name, &bind_addr, &t.target, true, t.agent_id.as_deref()).await {
                log::warn!("watcher: re-register '{}' failed: {}", t.name, e);
            }
        }
    }
}
