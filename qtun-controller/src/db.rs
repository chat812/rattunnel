use rusqlite::{params, Connection, Result};
use std::sync::Mutex;

pub struct Db {
    conn: Mutex<Connection>,
}

pub struct Tunnel {
    pub id: i64,
    pub name: String,
    pub subdomain: String,
    pub target: String,
    pub listen_port: u16,
    pub creator_chat_id: i64,
    pub agent_id: Option<String>,
    pub created_at: String,
    pub last_active_at: Option<String>,
    pub persistent: bool,
    pub status: String, // "active" or "idle"
}

pub struct Agent {
    pub id: i64,
    pub agent_id: String,
    pub token: String,
    pub chat_id: i64,
    pub name: String,
    pub created_at: String,
}

impl Db {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tunnels (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT UNIQUE NOT NULL,
                subdomain TEXT UNIQUE NOT NULL,
                target TEXT NOT NULL,
                listen_port INTEGER UNIQUE NOT NULL,
                creator_chat_id INTEGER NOT NULL DEFAULT 0,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                last_active_at DATETIME
            );",
        )?;
        // Agents table
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agents (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent_id TEXT UNIQUE NOT NULL,
                token TEXT NOT NULL,
                chat_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );",
        )?;

        // Migrate existing DBs that lack columns
        let _ = conn.execute(
            "ALTER TABLE tunnels ADD COLUMN last_active_at DATETIME",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tunnels ADD COLUMN creator_chat_id INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tunnels ADD COLUMN agent_id TEXT DEFAULT NULL",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tunnels ADD COLUMN persistent INTEGER NOT NULL DEFAULT 0",
            [],
        );
        let _ = conn.execute(
            "ALTER TABLE tunnels ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn insert(&self, name: &str, subdomain: &str, target: &str, port: u16, creator_chat_id: i64, agent_id: Option<&str>, persistent: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO tunnels (name, subdomain, target, listen_port, creator_chat_id, agent_id, persistent, status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'active')",
            params![name, subdomain, target, port, creator_chat_id, agent_id, persistent as i32],
        )?;
        Ok(())
    }

    fn row_to_tunnel(row: &rusqlite::Row) -> rusqlite::Result<Tunnel> {
        let persistent_int: i32 = row.get(9)?;
        Ok(Tunnel {
            id: row.get(0)?,
            name: row.get(1)?,
            subdomain: row.get(2)?,
            target: row.get(3)?,
            listen_port: row.get(4)?,
            creator_chat_id: row.get(5)?,
            agent_id: row.get(6)?,
            created_at: row.get(7)?,
            last_active_at: row.get(8)?,
            persistent: persistent_int != 0,
            status: row.get(10)?,
        })
    }

    pub fn all(&self) -> Result<Vec<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels ORDER BY id",
        )?;
        let rows = stmt.query_map([], Self::row_to_tunnel)?;
        rows.collect()
    }

    pub fn delete(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM tunnels WHERE name = ?1", params![name])?;
        Ok(n > 0)
    }

    pub fn port_in_use(&self, port: u16) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tunnels WHERE listen_port = ?1",
            params![port],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn name_in_use(&self, name: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tunnels WHERE name = ?1",
            params![name],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn subdomain_exists(&self, subdomain: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM tunnels WHERE subdomain = ?1",
            params![subdomain],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn find_by_name(&self, name: &str) -> Result<Option<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels WHERE name = ?1",
            params![name],
            Self::row_to_tunnel,
        );
        match result {
            Ok(t) => Ok(Some(t)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Get the creator's chat ID for a tunnel by name.
    pub fn creator_chat_id(&self, name: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT creator_chat_id FROM tunnels WHERE name = ?1",
            params![name],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Mark a tunnel as seen active right now.
    pub fn touch_active(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tunnels SET last_active_at = CURRENT_TIMESTAMP WHERE name = ?1",
            params![name],
        )?;
        Ok(())
    }

    /// List tunnels owned by a specific chat_id (user).
    pub fn tunnels_by_chat_id(&self, chat_id: i64) -> Result<Vec<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels WHERE creator_chat_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![chat_id], Self::row_to_tunnel)?;
        rows.collect()
    }

    // --- Agent methods ---

    pub fn insert_agent(&self, agent_id: &str, token: &str, chat_id: i64, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agents (agent_id, token, chat_id, name) VALUES (?1, ?2, ?3, ?4)",
            params![agent_id, token, chat_id, name],
        )?;
        Ok(())
    }

    pub fn all_agents(&self) -> Result<Vec<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, token, chat_id, name, created_at FROM agents ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Agent {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                token: row.get(2)?,
                chat_id: row.get(3)?,
                name: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn agents_by_chat_id(&self, chat_id: i64) -> Result<Vec<Agent>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, agent_id, token, chat_id, name, created_at FROM agents WHERE chat_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![chat_id], |row| {
            Ok(Agent {
                id: row.get(0)?,
                agent_id: row.get(1)?,
                token: row.get(2)?,
                chat_id: row.get(3)?,
                name: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect()
    }

    pub fn find_agent_by_name(&self, chat_id: i64, name: &str) -> Result<Option<Agent>> {
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT id, agent_id, token, chat_id, name, created_at FROM agents WHERE chat_id = ?1 AND name = ?2",
            params![chat_id, name],
            |row| {
                Ok(Agent {
                    id: row.get(0)?,
                    agent_id: row.get(1)?,
                    token: row.get(2)?,
                    chat_id: row.get(3)?,
                    name: row.get(4)?,
                    created_at: row.get(5)?,
                })
            },
        );
        match result {
            Ok(a) => Ok(Some(a)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn delete_agent(&self, agent_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM agents WHERE agent_id = ?1", params![agent_id])?;
        Ok(n > 0)
    }

    pub fn tunnels_by_agent(&self, agent_id: &str) -> Result<Vec<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels WHERE agent_id = ?1 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![agent_id], Self::row_to_tunnel)?;
        rows.collect()
    }

    /// Return tunnels that have been idle (non-active) for more than `threshold_secs` seconds.
    /// Idle = never seen active and created more than threshold ago,
    ///        OR last_active_at is older than threshold ago.
    /// Return non-persistent tunnels that have been idle for more than `threshold_secs`.
    pub fn idle_tunnels(&self, threshold_secs: u64) -> Result<Vec<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels
             WHERE persistent = 0 AND status = 'active'
             AND (
                 (last_active_at IS NULL
                  AND (strftime('%s', 'now') - strftime('%s', created_at)) > ?1)
              OR
                 (last_active_at IS NOT NULL
                  AND (strftime('%s', 'now') - strftime('%s', last_active_at)) > ?1)
             )",
        )?;
        let rows = stmt.query_map(params![threshold_secs as i64], Self::row_to_tunnel)?;
        rows.collect()
    }

    /// Return persistent tunnels that are active but idle for more than `threshold_secs`.
    /// These should be put to sleep (idle mode), not deleted.
    pub fn idle_persistent_tunnels(&self, threshold_secs: u64) -> Result<Vec<Tunnel>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, name, subdomain, target, listen_port, creator_chat_id, agent_id, created_at, last_active_at, persistent, status
             FROM tunnels
             WHERE persistent = 1 AND status = 'active'
             AND (
                 (last_active_at IS NULL
                  AND (strftime('%s', 'now') - strftime('%s', created_at)) > ?1)
              OR
                 (last_active_at IS NOT NULL
                  AND (strftime('%s', 'now') - strftime('%s', last_active_at)) > ?1)
             )",
        )?;
        let rows = stmt.query_map(params![threshold_secs as i64], Self::row_to_tunnel)?;
        rows.collect()
    }

    /// Set a tunnel's status to idle.
    pub fn set_idle(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tunnels SET status = 'idle', last_active_at = NULL WHERE name = ?1",
            params![name],
        )?;
        Ok(())
    }

    /// Set a tunnel's status to active.
    pub fn set_active(&self, name: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE tunnels SET status = 'active' WHERE name = ?1",
            params![name],
        )?;
        Ok(())
    }
}
