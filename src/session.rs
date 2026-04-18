//! SQLite-backed session store. Persists messages, large tool outputs (so
//! they can be referenced from the conversation by handle), OAuth tokens,
//! and MCP credentials. Per-session mutual exclusion is provided by an
//! OS-level file lock ([`SessionLock`]) so the kernel reclaims it whenever
//! the holder dies — no PID-aliveness check, no risk of stale locks.

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fd_lock::{RwLock as FdRwLock, RwLockWriteGuard as FdRwLockWriteGuard};
use serde::{Deserialize, Serialize};
use tokio_rusqlite::Connection;
use uuid::Uuid;

use crate::error::{AgshError, Result};
use crate::provider::AuthCredential;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: Uuid,
    pub updated_at: String,
    pub preview: String,
}

#[derive(Debug, Clone)]
pub struct ToolOutputSummary {
    pub name: String,
    pub size: usize,
    pub created_at: String,
}

#[derive(Clone)]
pub struct SessionManager {
    connection: Arc<Connection>,
    lock_dir: PathBuf,
}

/// RAII handle for an exclusive per-session OS file lock. Holding this value
/// keeps the underlying lock file descriptor open; dropping it (including
/// when the process exits or panics) closes the FD, which causes the kernel
/// to release the `flock`/`LockFileEx` lock automatically. There is no
/// "stale lock" failure mode — even `SIGKILL` is safe.
///
/// Internally this is a self-referential struct: `_guard` borrows from
/// `*_lock` (a `Box` for stable heap address). Field declaration order
/// guarantees `_guard` is dropped before `_lock`, which is the safety
/// invariant of the lifetime transmute used during construction.
pub struct SessionLock {
    _guard: FdRwLockWriteGuard<'static, File>,
    _lock: Box<FdRwLock<File>>,
}

fn default_database_path() -> Result<PathBuf> {
    // `dirs::data_dir()` honors XDG_DATA_HOME on Linux and the platform's
    // standard data directory elsewhere. Fall back to `$HOME/.local/share`
    // so a tilde never reaches the filesystem (which doesn't expand it).
    let base = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .ok_or_else(|| {
            AgshError::Config(
                "could not determine a data directory; set XDG_DATA_HOME or HOME, \
                 or pass an explicit session database path"
                    .into(),
            )
        })?;
    Ok(base.join("agsh").join("sessions.db"))
}

impl SessionManager {
    pub async fn open(path: Option<&Path>) -> Result<Self> {
        let database_path = match path {
            Some(path) => path.to_path_buf(),
            None => default_database_path()?,
        };

        // In-memory SQLite databases (used by tests) have no on-disk parent;
        // give each `open()` call its own ephemeral lock dir under the system
        // temp directory so concurrent tests don't share lock files.
        let is_in_memory = database_path == Path::new(":memory:");
        let lock_dir = if is_in_memory {
            std::env::temp_dir().join(format!("agsh-test-locks-{}", Uuid::new_v4()))
        } else {
            if let Some(parent) = database_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            database_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("locks")
        };
        std::fs::create_dir_all(&lock_dir)?;

        let connection = Connection::open(&database_path)
            .await
            .map_err(|error| AgshError::Database(format!("failed to open database: {}", error)))?;

        let manager = Self {
            connection: Arc::new(connection),
            lock_dir,
        };
        manager.initialize_schema().await?;
        Ok(manager)
    }

    async fn initialize_schema(&self) -> Result<()> {
        self.connection
            .call(|connection| {
                connection.execute_batch(
                    "CREATE TABLE IF NOT EXISTS sessions (
                        id TEXT PRIMARY KEY,
                        created_at TEXT NOT NULL,
                        updated_at TEXT NOT NULL,
                        metadata TEXT
                    );

                    CREATE TABLE IF NOT EXISTS messages (
                        id INTEGER PRIMARY KEY AUTOINCREMENT,
                        session_id TEXT NOT NULL,
                        role TEXT NOT NULL,
                        content TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        FOREIGN KEY (session_id) REFERENCES sessions(id)
                    );

                    CREATE INDEX IF NOT EXISTS idx_messages_session_id
                        ON messages(session_id);

                    CREATE TABLE IF NOT EXISTS oauth_tokens (
                        provider TEXT PRIMARY KEY,
                        access_token TEXT NOT NULL,
                        refresh_token TEXT,
                        expires_at INTEGER,
                        updated_at TEXT NOT NULL
                    );

                    CREATE TABLE IF NOT EXISTS mcp_oauth_credentials (
                        server_name TEXT PRIMARY KEY,
                        credentials_json TEXT NOT NULL,
                        updated_at TEXT NOT NULL
                    );

                    ",
                )?;

                // Migration: recreate tool_outputs if it has the old integer-ID schema.
                let has_old_schema: bool = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('tool_outputs') WHERE name = 'id'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    > 0;
                if has_old_schema {
                    connection.execute_batch("DROP TABLE tool_outputs")?;
                }

                // Migration: drop the legacy `sessions.locked_by` column.
                // Locks are now OS file locks managed via `SessionLock`, so
                // any value left in this column is meaningless and a stale
                // PID can permanently lock a session if the column survives.
                let has_locked_by: bool = connection
                    .query_row(
                        "SELECT COUNT(*) FROM pragma_table_info('sessions') WHERE name = 'locked_by'",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap_or(0)
                    > 0;
                if has_locked_by {
                    connection.execute_batch("ALTER TABLE sessions DROP COLUMN locked_by")?;
                }

                connection.execute_batch(
                    "CREATE TABLE IF NOT EXISTS tool_outputs (
                        session_id TEXT NOT NULL,
                        name TEXT NOT NULL,
                        content TEXT NOT NULL,
                        created_at TEXT NOT NULL,
                        PRIMARY KEY (session_id, name),
                        FOREIGN KEY (session_id) REFERENCES sessions(id)
                    );",
                )?;

                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to initialize schema: {}", error)))
    }

    pub async fn create_session(&self) -> Result<Uuid> {
        let session_id = Uuid::new_v4();
        let now = chrono::Utc::now().to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "INSERT INTO sessions (id, created_at, updated_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![session_id.to_string(), now, now],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to create session: {}", error)))?;

        Ok(session_id)
    }

    /// Acquire an exclusive OS file lock on the session. Returns a
    /// [`SessionLock`] handle whose lifetime owns the lock; drop it (or let
    /// the process exit) to release.
    ///
    /// The session must already exist in the database. Returns
    /// [`AgshError::SessionLocked`] if another live process holds the lock.
    pub fn lock_session(&self, session_id: Uuid) -> Result<SessionLock> {
        let path = self.lock_dir.join(format!("{}.lock", session_id));
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                AgshError::Database(format!(
                    "failed to open session lock file '{}': {}",
                    path.display(),
                    error
                ))
            })?;

        let mut lock = Box::new(FdRwLock::new(file));
        let guard = match lock.try_write() {
            Ok(guard) => guard,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(AgshError::SessionLocked(session_id));
            }
            Err(error) => {
                return Err(AgshError::Database(format!(
                    "failed to acquire session lock '{}': {}",
                    path.display(),
                    error
                )));
            }
        };

        // SAFETY: `guard` borrows from `*lock`. We move the box (not the
        // RwLock inside it) into the returned `SessionLock`, so the RwLock's
        // heap address is stable for as long as the box lives. Field
        // declaration order in `SessionLock` ensures `_guard` is dropped
        // before `_lock`, so the borrow never outlives the borrowee.
        let guard: FdRwLockWriteGuard<'static, File> = unsafe { std::mem::transmute(guard) };

        Ok(SessionLock {
            _guard: guard,
            _lock: lock,
        })
    }

    pub async fn save_message(&self, session_id: Uuid, role: &str, content: &str) -> Result<()> {
        let role = role.to_string();
        let content = content.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "INSERT INTO messages (session_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![session_id.to_string(), role, content, now],
                )?;

                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![
                        chrono::Utc::now().to_rfc3339(),
                        session_id.to_string()
                    ],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to save message: {}", error)))
    }

    pub async fn load_messages(&self, session_id: Uuid) -> Result<Vec<StoredMessage>> {
        self.connection
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT role, content, created_at FROM messages WHERE session_id = ?1 ORDER BY id ASC",
                )?;

                let messages = statement
                    .query_map(rusqlite::params![session_id.to_string()], |row| {
                        Ok(StoredMessage {
                            role: row.get(0)?,
                            content: row.get(1)?,
                            created_at: row.get(2)?,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok(messages)
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to load messages: {}", error)))
    }

    pub async fn last_session_id(&self) -> Result<Option<Uuid>> {
        self.connection
            .call(|connection| {
                let result: std::result::Result<String, _> = connection.query_row(
                    "SELECT id FROM sessions ORDER BY updated_at DESC LIMIT 1",
                    [],
                    |row| row.get(0),
                );

                match result {
                    Ok(id_str) => {
                        let uuid = Uuid::parse_str(&id_str).map_err(|error| {
                            rusqlite::Error::InvalidParameterName(error.to_string())
                        })?;
                        Ok(Some(uuid))
                    }
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to get last session: {}", error)))
    }

    pub async fn session_exists(&self, session_id: Uuid) -> Result<bool> {
        self.connection
            .call(move |connection| {
                let count: i64 = connection.query_row(
                    "SELECT COUNT(*) FROM sessions WHERE id = ?1",
                    rusqlite::params![session_id.to_string()],
                    |row| row.get(0),
                )?;
                Ok(count > 0)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to check session existence: {}", error))
            })
    }

    pub async fn list_sessions(&self, limit: u32) -> Result<Vec<SessionSummary>> {
        self.connection
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT s.id, s.updated_at,
                            COALESCE(
                              (SELECT content FROM messages
                               WHERE session_id = s.id AND role = 'user'
                               ORDER BY id ASC LIMIT 1),
                              ''
                            ) AS preview
                     FROM sessions s
                     ORDER BY s.updated_at DESC
                     LIMIT ?1",
                )?;

                let rows = statement.query_map(rusqlite::params![limit], |row| {
                    let id_str: String = row.get(0)?;
                    let updated_at: String = row.get(1)?;
                    let preview: String = row.get(2)?;
                    Ok((id_str, updated_at, preview))
                })?;

                let mut summaries = Vec::new();
                for row in rows {
                    let (id_str, updated_at, preview) = row?;
                    let id = Uuid::parse_str(&id_str).map_err(|error| {
                        rusqlite::Error::InvalidParameterName(error.to_string())
                    })?;

                    let preview = truncate_preview(&preview, 80);

                    summaries.push(SessionSummary {
                        id,
                        updated_at,
                        preview,
                    });
                }
                Ok(summaries)
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to list sessions: {}", error)))
    }

    pub async fn delete_expired_sessions(&self, retention_days: u64) -> Result<u64> {
        let cutoff = chrono::Utc::now() - chrono::TimeDelta::days(retention_days as i64);
        let cutoff_str = cutoff.to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "DELETE FROM tool_outputs WHERE session_id IN (
                        SELECT id FROM sessions WHERE updated_at < ?1
                    )",
                    rusqlite::params![cutoff_str],
                )?;

                connection.execute(
                    "DELETE FROM messages WHERE session_id IN (
                        SELECT id FROM sessions WHERE updated_at < ?1
                    )",
                    rusqlite::params![cutoff_str],
                )?;

                let deleted = connection.execute(
                    "DELETE FROM sessions WHERE updated_at < ?1",
                    rusqlite::params![cutoff_str],
                )?;

                Ok(deleted as u64)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to delete expired sessions: {}", error))
            })
    }

    #[cfg(test)]
    pub async fn clear_messages(&self, session_id: Uuid) -> Result<()> {
        self.connection
            .call(move |connection| {
                connection.execute(
                    "DELETE FROM tool_outputs WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                connection.execute(
                    "DELETE FROM messages WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![chrono::Utc::now().to_rfc3339(), session_id.to_string()],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to clear messages: {}", error)))
    }

    /// Clear messages but preserve scratchpad (tool_outputs). Used by compaction.
    pub async fn clear_messages_only(&self, session_id: Uuid) -> Result<()> {
        self.connection
            .call(move |connection| {
                connection.execute(
                    "DELETE FROM messages WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![chrono::Utc::now().to_rfc3339(), session_id.to_string()],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to clear messages: {}", error)))
    }

    pub async fn delete_session(&self, session_id: Uuid) -> Result<bool> {
        self.connection
            .call(move |connection| {
                connection.execute(
                    "DELETE FROM tool_outputs WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                connection.execute(
                    "DELETE FROM messages WHERE session_id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                let deleted = connection.execute(
                    "DELETE FROM sessions WHERE id = ?1",
                    rusqlite::params![session_id.to_string()],
                )?;

                Ok(deleted > 0)
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to delete session: {}", error)))
    }

    pub async fn delete_all_sessions(&self) -> Result<u64> {
        self.connection
            .call(move |connection| {
                connection.execute("DELETE FROM tool_outputs", [])?;
                connection.execute("DELETE FROM messages", [])?;
                let deleted = connection.execute("DELETE FROM sessions", [])?;
                Ok(deleted as u64)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to delete all sessions: {}", error))
            })
    }

    pub async fn save_tool_output(
        &self,
        session_id: Uuid,
        name: &str,
        content: &str,
    ) -> Result<()> {
        let name = name.to_string();
        let content = content.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "INSERT OR REPLACE INTO tool_outputs (session_id, name, content, created_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![session_id.to_string(), name, content, now],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to save tool output: {}", error)))
    }

    pub async fn update_tool_output(
        &self,
        session_id: Uuid,
        name: &str,
        content: &str,
    ) -> Result<bool> {
        let name = name.to_string();
        let content = content.to_string();

        self.connection
            .call(move |connection| {
                let updated = connection.execute(
                    "UPDATE tool_outputs SET content = ?1 \
                     WHERE session_id = ?2 AND name = ?3",
                    rusqlite::params![content, session_id.to_string(), name],
                )?;
                Ok(updated > 0)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to update tool output: {}", error))
            })
    }

    pub async fn delete_tool_output(&self, session_id: Uuid, name: &str) -> Result<bool> {
        let name = name.to_string();

        self.connection
            .call(move |connection| {
                let deleted = connection.execute(
                    "DELETE FROM tool_outputs WHERE session_id = ?1 AND name = ?2",
                    rusqlite::params![session_id.to_string(), name],
                )?;
                Ok(deleted > 0)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to delete tool output: {}", error))
            })
    }

    pub async fn list_tool_outputs(&self, session_id: Uuid) -> Result<Vec<ToolOutputSummary>> {
        self.connection
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT name, LENGTH(content), created_at \
                     FROM tool_outputs WHERE session_id = ?1 ORDER BY created_at ASC",
                )?;

                let rows = statement
                    .query_map(rusqlite::params![session_id.to_string()], |row| {
                        Ok(ToolOutputSummary {
                            name: row.get(0)?,
                            size: row.get::<_, i64>(1)? as usize,
                            created_at: row.get(2)?,
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok(rows)
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to list tool outputs: {}", error)))
    }

    pub async fn load_tool_output(&self, session_id: Uuid, name: &str) -> Result<Option<String>> {
        let name = name.to_string();

        self.connection
            .call(move |connection| {
                let result = connection.query_row(
                    "SELECT content FROM tool_outputs \
                     WHERE session_id = ?1 AND name = ?2",
                    rusqlite::params![session_id.to_string(), name],
                    |row| row.get::<_, String>(0),
                );

                match result {
                    Ok(content) => Ok(Some(content)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to load tool output: {}", error)))
    }

    pub async fn load_all_tool_outputs(&self, session_id: Uuid) -> Result<Vec<(String, String)>> {
        self.connection
            .call(move |connection| {
                let mut statement = connection.prepare(
                    "SELECT name, content FROM tool_outputs \
                     WHERE session_id = ?1 ORDER BY created_at ASC",
                )?;

                let rows = statement
                    .query_map(rusqlite::params![session_id.to_string()], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                Ok(rows)
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to load tool outputs: {}", error)))
    }

    pub async fn enforce_storage_limit(&self, max_bytes: u64) -> Result<u64> {
        self.connection
            .call(move |connection| {
                let mut deleted: u64 = 0;

                loop {
                    let total_bytes: i64 = connection.query_row(
                        "SELECT COALESCE(SUM(LENGTH(content)), 0) FROM messages",
                        [],
                        |row| row.get(0),
                    )?;

                    if (total_bytes as u64) <= max_bytes {
                        break;
                    }

                    let oldest_id: std::result::Result<String, _> = connection.query_row(
                        "SELECT id FROM sessions ORDER BY updated_at ASC LIMIT 1",
                        [],
                        |row| row.get(0),
                    );

                    match oldest_id {
                        Ok(session_id) => {
                            connection.execute(
                                "DELETE FROM tool_outputs WHERE session_id = ?1",
                                rusqlite::params![session_id],
                            )?;
                            connection.execute(
                                "DELETE FROM messages WHERE session_id = ?1",
                                rusqlite::params![session_id],
                            )?;
                            connection.execute(
                                "DELETE FROM sessions WHERE id = ?1",
                                rusqlite::params![session_id],
                            )?;
                            deleted += 1;
                        }
                        Err(rusqlite::Error::QueryReturnedNoRows) => break,
                        Err(error) => return Err(error.into()),
                    }
                }

                Ok(deleted)
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to enforce storage limit: {}", error))
            })
    }
}

#[derive(Clone)]
pub struct TokenStore {
    connection: Arc<Connection>,
}

impl TokenStore {
    pub async fn load_oauth_token(&self, provider: &str) -> Result<Option<AuthCredential>> {
        let provider = provider.to_string();
        self.connection
            .call(move |connection| {
                let result = connection.query_row(
                    "SELECT access_token, refresh_token, expires_at FROM oauth_tokens WHERE provider = ?1",
                    rusqlite::params![provider],
                    |row| {
                        let access_token: String = row.get(0)?;
                        let refresh_token: Option<String> = row.get(1)?;
                        let expires_at: Option<i64> = row.get(2)?;
                        Ok(AuthCredential::OAuthToken {
                            access_token,
                            refresh_token,
                            expires_at,
                        })
                    },
                );

                match result {
                    Ok(credential) => Ok(Some(credential)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to load OAuth token: {}", error)))
    }

    pub async fn load_mcp_credentials(&self, server_name: &str) -> Result<Option<String>> {
        let server_name = server_name.to_string();
        self.connection
            .call(move |connection| {
                let result = connection.query_row(
                    "SELECT credentials_json FROM mcp_oauth_credentials WHERE server_name = ?1",
                    rusqlite::params![server_name],
                    |row| row.get::<_, String>(0),
                );

                match result {
                    Ok(json) => Ok(Some(json)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to load MCP credentials: {}", error))
            })
    }

    pub async fn save_mcp_credentials(&self, server_name: &str, json: &str) -> Result<()> {
        let server_name = server_name.to_string();
        let json = json.to_string();
        let now = chrono::Utc::now().to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "INSERT INTO mcp_oauth_credentials (server_name, credentials_json, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(server_name) DO UPDATE SET
                         credentials_json = excluded.credentials_json,
                         updated_at = excluded.updated_at",
                    rusqlite::params![server_name, json, now],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to save MCP credentials: {}", error))
            })
    }

    pub async fn clear_mcp_credentials(&self, server_name: &str) -> Result<()> {
        let server_name = server_name.to_string();
        self.connection
            .call(move |connection| {
                connection.execute(
                    "DELETE FROM mcp_oauth_credentials WHERE server_name = ?1",
                    rusqlite::params![server_name],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| {
                AgshError::Database(format!("failed to clear MCP credentials: {}", error))
            })
    }

    pub async fn save_oauth_token(
        &self,
        provider: &str,
        credential: &AuthCredential,
    ) -> Result<()> {
        let AuthCredential::OAuthToken {
            access_token,
            refresh_token,
            expires_at,
        } = credential
        else {
            return Ok(());
        };

        let provider = provider.to_string();
        let access_token = access_token.clone();
        let refresh_token = refresh_token.clone();
        let expires_at = *expires_at;
        let now = chrono::Utc::now().to_rfc3339();

        self.connection
            .call(move |connection| {
                connection.execute(
                    "INSERT INTO oauth_tokens (provider, access_token, refresh_token, expires_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(provider) DO UPDATE SET
                         access_token = excluded.access_token,
                         refresh_token = excluded.refresh_token,
                         expires_at = excluded.expires_at,
                         updated_at = excluded.updated_at",
                    rusqlite::params![provider, access_token, refresh_token, expires_at, now],
                )?;
                Ok(())
            })
            .await
            .map_err(|error| AgshError::Database(format!("failed to save OAuth token: {}", error)))
    }
}

impl SessionManager {
    pub fn token_store(&self) -> TokenStore {
        TokenStore {
            connection: Arc::clone(&self.connection),
        }
    }
}

/// Strip `<context>...</context>` tags from a stored user message,
/// returning only the actual user input.
pub fn strip_context_tags(text: &str) -> &str {
    const CLOSING_TAG: &str = "</context>";
    if let Some(end) = text.find(CLOSING_TAG) {
        let after = &text[end + CLOSING_TAG.len()..];
        after.trim_start_matches('\n')
    } else {
        text
    }
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let text = strip_context_tags(text);
    let first_line = text.lines().next().unwrap_or("");
    if first_line.chars().count() <= max_chars {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(max_chars).collect();
        format!("{}…", truncated.trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_manager() -> SessionManager {
        SessionManager::open(Some(Path::new(":memory:")))
            .await
            .expect("failed to open in-memory database")
    }

    #[tokio::test]
    async fn test_create_session() {
        let manager = test_manager().await;
        let session_id = manager
            .create_session()
            .await
            .expect("failed to create session");
        assert!(
            manager
                .session_exists(session_id)
                .await
                .expect("failed to check")
        );
    }

    #[tokio::test]
    async fn test_save_and_load_messages() {
        let manager = test_manager().await;
        let session_id = manager
            .create_session()
            .await
            .expect("failed to create session");

        manager
            .save_message(session_id, "user", "hello")
            .await
            .expect("failed to save message");
        manager
            .save_message(session_id, "assistant", "hi there")
            .await
            .expect("failed to save message");

        let messages = manager
            .load_messages(session_id)
            .await
            .expect("failed to load messages");

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].content, "hello");
        assert_eq!(messages[1].role, "assistant");
        assert_eq!(messages[1].content, "hi there");
    }

    #[tokio::test]
    async fn test_last_session_id() {
        let manager = test_manager().await;
        assert!(manager.last_session_id().await.expect("failed").is_none());

        let session_id = manager
            .create_session()
            .await
            .expect("failed to create session");
        let last = manager
            .last_session_id()
            .await
            .expect("failed to get last session");
        assert_eq!(last, Some(session_id));
    }

    #[tokio::test]
    async fn test_session_locking_acquire_and_release() {
        let manager = test_manager().await;
        let session_id = manager
            .create_session()
            .await
            .expect("failed to create session");

        let lock = manager
            .lock_session(session_id)
            .expect("failed to lock session");

        // While the lock handle is alive, a second attempt must fail.
        match manager.lock_session(session_id) {
            Err(AgshError::SessionLocked(id)) => assert_eq!(id, session_id),
            other => panic!("expected SessionLocked, got {:?}", other.map(|_| "Ok(_)")),
        }

        // Dropping the handle releases the OS lock; re-acquisition succeeds.
        drop(lock);
        let _lock2 = manager
            .lock_session(session_id)
            .expect("failed to re-acquire session lock after drop");
    }

    #[tokio::test]
    async fn test_session_not_found() {
        let manager = test_manager().await;
        let fake_id = Uuid::new_v4();
        assert!(
            !manager
                .session_exists(fake_id)
                .await
                .expect("failed to check")
        );
    }

    #[tokio::test]
    async fn test_multiple_sessions() {
        let manager = test_manager().await;
        let session1 = manager.create_session().await.expect("failed");
        let session2 = manager.create_session().await.expect("failed");

        manager
            .save_message(session1, "user", "msg1")
            .await
            .expect("failed");
        manager
            .save_message(session2, "user", "msg2")
            .await
            .expect("failed");

        let messages1 = manager.load_messages(session1).await.expect("failed");
        let messages2 = manager.load_messages(session2).await.expect("failed");

        assert_eq!(messages1.len(), 1);
        assert_eq!(messages1[0].content, "msg1");
        assert_eq!(messages2.len(), 1);
        assert_eq!(messages2[0].content, "msg2");
    }

    #[tokio::test]
    async fn test_delete_expired_sessions() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("failed");
        manager
            .save_message(session_id, "user", "hello")
            .await
            .expect("failed");

        // Backdate the session to 100 days ago
        let old_date = (chrono::Utc::now() - chrono::TimeDelta::days(100)).to_rfc3339();
        manager
            .connection
            .call(move |connection| {
                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![old_date, session_id.to_string()],
                )?;
                Ok(())
            })
            .await
            .expect("failed to backdate");

        let deleted = manager
            .delete_expired_sessions(30)
            .await
            .expect("failed to delete");
        assert_eq!(deleted, 1);
        assert!(!manager.session_exists(session_id).await.expect("failed"));

        let messages = manager.load_messages(session_id).await.expect("failed");
        assert!(messages.is_empty());
    }

    #[tokio::test]
    async fn test_delete_expired_sessions_keeps_recent() {
        let manager = test_manager().await;
        let old_session = manager.create_session().await.expect("failed");
        let new_session = manager.create_session().await.expect("failed");

        manager
            .save_message(old_session, "user", "old")
            .await
            .expect("failed");
        manager
            .save_message(new_session, "user", "new")
            .await
            .expect("failed");

        // Backdate only the old session
        let old_date = (chrono::Utc::now() - chrono::TimeDelta::days(100)).to_rfc3339();
        manager
            .connection
            .call(move |connection| {
                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![old_date, old_session.to_string()],
                )?;
                Ok(())
            })
            .await
            .expect("failed to backdate");

        let deleted = manager
            .delete_expired_sessions(30)
            .await
            .expect("failed to delete");
        assert_eq!(deleted, 1);
        assert!(!manager.session_exists(old_session).await.expect("failed"));
        assert!(manager.session_exists(new_session).await.expect("failed"));
    }

    #[tokio::test]
    async fn test_enforce_storage_limit() {
        let manager = test_manager().await;
        let session1 = manager.create_session().await.expect("failed");

        // Add enough content to exceed a small limit
        let large_content = "x".repeat(1000);
        manager
            .save_message(session1, "user", &large_content)
            .await
            .expect("failed");

        // Backdate session1 so it's the oldest
        let old_date = (chrono::Utc::now() - chrono::TimeDelta::days(10)).to_rfc3339();
        manager
            .connection
            .call(move |connection| {
                connection.execute(
                    "UPDATE sessions SET updated_at = ?1 WHERE id = ?2",
                    rusqlite::params![old_date, session1.to_string()],
                )?;
                Ok(())
            })
            .await
            .expect("failed to backdate");

        let session2 = manager.create_session().await.expect("failed");
        manager
            .save_message(session2, "user", "small")
            .await
            .expect("failed");

        // Set a limit smaller than the total, but larger than session2 alone
        let deleted = manager
            .enforce_storage_limit(500)
            .await
            .expect("failed to enforce");
        assert_eq!(deleted, 1);
        assert!(!manager.session_exists(session1).await.expect("failed"));
        assert!(manager.session_exists(session2).await.expect("failed"));
    }

    #[tokio::test]
    async fn test_clear_messages() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("failed");

        manager
            .save_message(session_id, "user", "hello")
            .await
            .expect("failed");
        manager
            .save_message(session_id, "assistant", "hi")
            .await
            .expect("failed");

        let messages = manager.load_messages(session_id).await.expect("failed");
        assert_eq!(messages.len(), 2);

        manager
            .clear_messages(session_id)
            .await
            .expect("failed to clear");

        let messages = manager.load_messages(session_id).await.expect("failed");
        assert!(messages.is_empty());

        // Session itself should still exist
        assert!(manager.session_exists(session_id).await.expect("failed"));
    }

    #[test]
    fn test_strip_context_tags_with_context() {
        let input = "<context>\n[Environment context]\nWorking directory: /tmp\nDate: Mon\n</context>\n\nhello world";
        assert_eq!(strip_context_tags(input), "hello world");
    }

    #[test]
    fn test_strip_context_tags_without_context() {
        let input = "hello world";
        assert_eq!(strip_context_tags(input), "hello world");
    }

    #[test]
    fn test_strip_context_tags_empty_after_context() {
        let input = "<context>\nstuff\n</context>\n\n";
        assert_eq!(strip_context_tags(input), "");
    }

    #[test]
    fn test_truncate_preview_with_context_tags() {
        let input = "<context>\n[Environment context]\nWorking directory: /tmp\n</context>\n\nfind all Rust files";
        assert_eq!(truncate_preview(input, 80), "find all Rust files");
    }

    #[test]
    fn test_truncate_preview_without_context_tags() {
        let input = "find all Rust files";
        assert_eq!(truncate_preview(input, 80), "find all Rust files");
    }

    #[test]
    fn test_truncate_preview_old_format_backward_compat() {
        let input = "[Environment context]\nWorking directory: /tmp\n\nfind all Rust files";
        assert_eq!(truncate_preview(input, 80), "[Environment context]");
    }

    #[test]
    fn test_truncate_preview_with_context_tags_long_input() {
        let long_input = format!("<context>\nstuff\n</context>\n\n{}", "x".repeat(100));
        let preview = truncate_preview(&long_input, 80);
        assert!(preview.ends_with('…'));
        assert!(preview.len() <= 84); // 80 chars + "…"
    }

    #[tokio::test]
    async fn test_enforce_storage_limit_no_deletion_needed() {
        let manager = test_manager().await;
        let session_id = manager.create_session().await.expect("failed");
        manager
            .save_message(session_id, "user", "small")
            .await
            .expect("failed");

        let deleted = manager
            .enforce_storage_limit(1_000_000)
            .await
            .expect("failed to enforce");
        assert_eq!(deleted, 0);
        assert!(manager.session_exists(session_id).await.expect("failed"));
    }
}
