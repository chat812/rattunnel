use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rand::Rng;

const TOKEN_TTL: Duration = Duration::from_secs(600); // 10 minutes
const TOKEN_LEN: usize = 16;

// Rate-limit: expire token if >RATE_LIMIT_MAX requests in RATE_LIMIT_WINDOW
const RATE_LIMIT_MAX: usize = 10;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(5);

pub struct DownloadToken {
    pub chat_id: i64,
    pub created_at: Instant,
    /// IPs that have been approved by the Telegram user.
    pub approved_ips: HashSet<IpAddr>,
    /// IPs currently pending approval (avoid duplicate prompts).
    pub pending_ips: HashSet<IpAddr>,
    /// Timestamps of recent download requests (for rate-limiting).
    recent_hits: Vec<Instant>,
}

impl DownloadToken {
    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > TOKEN_TTL
    }

    pub fn remaining_secs(&self) -> u64 {
        TOKEN_TTL
            .checked_sub(self.created_at.elapsed())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// Result of checking access for a download request.
pub enum AccessResult {
    /// IP is approved — serve the file.
    Approved,
    /// IP needs approval — send Telegram prompt. Returns (chat_id).
    NeedApproval(i64),
    /// IP is already pending approval — don't send another prompt.
    Pending,
    /// Token invalid, expired, or rate-limited.
    Denied,
}

pub struct DownloadTokenStore {
    tokens: Mutex<HashMap<String, DownloadToken>>,
}

impl DownloadTokenStore {
    pub fn new() -> Self {
        Self {
            tokens: Mutex::new(HashMap::new()),
        }
    }

    /// Find an existing non-expired token for this chat_id.
    pub fn find_active(&self, chat_id: i64) -> Option<(String, u64)> {
        let tokens = self.tokens.lock().unwrap();
        for (tok, dt) in tokens.iter() {
            if dt.chat_id == chat_id && !dt.is_expired() {
                return Some((tok.clone(), dt.remaining_secs()));
            }
        }
        None
    }

    /// Create a new token for the given chat_id. Returns the token string.
    pub fn create(&self, chat_id: i64) -> String {
        let mut rng = rand::thread_rng();
        let token: String = (0..TOKEN_LEN)
            .map(|_| {
                let idx = rng.gen_range(0u8..36);
                if idx < 10 {
                    (b'0' + idx) as char
                } else {
                    (b'a' + idx - 10) as char
                }
            })
            .collect();

        let mut tokens = self.tokens.lock().unwrap();
        tokens.retain(|_, dt| dt.is_expired().not());
        tokens.insert(
            token.clone(),
            DownloadToken {
                chat_id,
                created_at: Instant::now(),
                approved_ips: HashSet::new(),
                pending_ips: HashSet::new(),
                recent_hits: Vec::new(),
            },
        );
        token
    }

    /// Check access for a download request. Performs rate-limit check and
    /// returns what action the caller should take.
    pub fn check_access(&self, token: &str, ip: IpAddr) -> AccessResult {
        let mut tokens = self.tokens.lock().unwrap();
        let dt = match tokens.get_mut(token) {
            Some(dt) if !dt.is_expired() => dt,
            _ => return AccessResult::Denied,
        };

        // Rate-limit check
        let now = Instant::now();
        dt.recent_hits.retain(|t| now.duration_since(*t) < RATE_LIMIT_WINDOW);
        dt.recent_hits.push(now);
        if dt.recent_hits.len() > RATE_LIMIT_MAX {
            log::warn!("Download token {} rate-limited, expiring immediately", token);
            // Force-expire by setting created_at far in the past
            dt.created_at = Instant::now() - TOKEN_TTL - Duration::from_secs(1);
            return AccessResult::Denied;
        }

        if dt.approved_ips.contains(&ip) {
            return AccessResult::Approved;
        }

        if dt.pending_ips.contains(&ip) {
            return AccessResult::Pending;
        }

        // Mark as pending
        dt.pending_ips.insert(ip);
        let chat_id = dt.chat_id;
        AccessResult::NeedApproval(chat_id)
    }

    /// Approve an IP for a token.
    pub fn approve_ip(&self, token: &str, ip: IpAddr) {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(dt) = tokens.get_mut(token) {
            dt.pending_ips.remove(&ip);
            dt.approved_ips.insert(ip);
        }
    }

    /// Deny an IP for a token.
    pub fn deny_ip(&self, token: &str, ip: IpAddr) {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(dt) = tokens.get_mut(token) {
            dt.pending_ips.remove(&ip);
        }
    }
}

// Helper to use ! with retain
trait Not {
    fn not(self) -> bool;
}
impl Not for bool {
    fn not(self) -> bool {
        !self
    }
}
