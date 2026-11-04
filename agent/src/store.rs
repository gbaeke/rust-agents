//! Where chat sessions are kept: the history (rig's session store), the
//! workspace (plan and files) and a lock that lets a session run one turn at
//! a time. With `DATABASE_URL` all three live in Postgres and every replica
//! shares them, so any pod can serve any message. Without it they live in
//! this process's memory, as for `cargo run`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError};

use rig::memory::MemoryStore;
use rig::postgres::PostgresSessionStore;
use sqlx::{Connection, PgConnection, PgPool};

use crate::tools::Workspace;

/// The workspace table, also the list of sessions: every turn touches its
/// row first, so `updated_at` is the session's last activity and expiry
/// goes by it. Made at startup under an advisory lock, so replicas that
/// start together make it once.
const SCHEMA: &str = "
SELECT pg_advisory_xact_lock(hashtext('agent_workspaces_schema'));
CREATE TABLE IF NOT EXISTS agent_workspaces (
    session_id TEXT PRIMARY KEY,
    workspace JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
";

/// The conversation store rig's agent loads before a turn and saves after it.
pub enum History {
    Postgres(PostgresSessionStore),
    Memory(MemoryStore),
}

pub struct Store {
    pub history: History,
    backend: Backend,
}

enum Backend {
    Postgres {
        url: String,
        pool: PgPool,
    },
    Memory {
        workspaces: Mutex<HashMap<String, Workspace>>,
        busy: Arc<Mutex<HashSet<String>>>,
    },
}

/// Held for the length of one turn; dropping it lets the session's next turn start.
pub enum TurnLock {
    /// A session-level advisory lock, released when this connection closes,
    /// so also when the stream is dropped or the pod dies.
    Postgres(#[allow(dead_code)] PgConnection),
    Memory { busy: Arc<Mutex<HashSet<String>>>, session: String },
}

impl Drop for TurnLock {
    fn drop(&mut self) {
        if let TurnLock::Memory { busy, session } = self {
            busy.lock().unwrap_or_else(PoisonError::into_inner).remove(session);
        }
    }
}

impl Store {
    pub async fn open(database_url: Option<String>) -> anyhow::Result<Self> {
        let Some(url) = database_url else {
            return Ok(Self {
                history: History::Memory(MemoryStore::default()),
                backend: Backend::Memory { workspaces: Mutex::default(), busy: Arc::default() },
            });
        };
        let pool = PgPool::connect(&url).await?;
        let history = History::Postgres(PostgresSessionStore::open(pool.clone()).await?);
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Self { history, backend: Backend::Postgres { url, pool } })
    }

    pub fn kind(&self) -> &'static str {
        match self.backend {
            Backend::Postgres { .. } => "postgres",
            Backend::Memory { .. } => "memory",
        }
    }

    /// The session's turn lock, or `None` while another turn of it runs, on any replica.
    pub async fn lock(&self, session: &str) -> anyhow::Result<Option<TurnLock>> {
        match &self.backend {
            Backend::Postgres { url, .. } => {
                // Its own connection, not a pooled one: a pooled connection
                // would go back to the pool still holding the lock.
                let mut connection = PgConnection::connect(url).await?;
                let (locked,): (bool,) =
                    sqlx::query_as("SELECT pg_try_advisory_lock(hashtext('agent_turn'), hashtext($1))")
                        .bind(session)
                        .fetch_one(&mut connection)
                        .await?;
                Ok(locked.then_some(TurnLock::Postgres(connection)))
            }
            Backend::Memory { busy, .. } => {
                let fresh = busy.lock().unwrap_or_else(PoisonError::into_inner).insert(session.to_owned());
                Ok(fresh.then(|| TurnLock::Memory { busy: busy.clone(), session: session.to_owned() }))
            }
        }
    }

    /// Marks the session as used now, so it expires counting from its last turn.
    pub async fn touch(&self, session: &str) -> anyhow::Result<()> {
        if let Backend::Postgres { pool, .. } = &self.backend {
            sqlx::query(
                "INSERT INTO agent_workspaces (session_id, workspace) VALUES ($1, $2)
                 ON CONFLICT (session_id) DO UPDATE SET updated_at = now()",
            )
            .bind(session)
            .bind(sqlx::types::Json(Workspace::default()))
            .execute(pool)
            .await?;
        }
        Ok(())
    }

    /// Deletes the sessions idle for longer than `idle`, with their history,
    /// and returns how many went. One replica at a time does it; the others
    /// return 0. In memory nothing expires: the process does not live that long.
    pub async fn expire(&self, idle: std::time::Duration) -> anyhow::Result<u64> {
        let Backend::Postgres { pool, .. } = &self.backend else { return Ok(0) };
        let mut tx = pool.begin().await?;
        let (ours,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_xact_lock(hashtext('agent_expiry'))")
            .fetch_one(&mut *tx)
            .await?;
        if !ours {
            return Ok(0);
        }
        sqlx::query("DELETE FROM agent_workspaces WHERE updated_at < now() - make_interval(secs => $1)")
            .bind(idle.as_secs_f64())
            .execute(&mut *tx)
            .await?;
        // A conversation whose session row is gone: just expired, or from
        // before sessions were touched. A running turn touched its row
        // before rig saves its log, so it is never caught here.
        sqlx::query("DELETE FROM rig_memory_log_chunks WHERE name NOT IN (SELECT session_id FROM agent_workspaces)")
            .execute(&mut *tx)
            .await?;
        let gone = sqlx::query("DELETE FROM rig_memory_logs WHERE name NOT IN (SELECT session_id FROM agent_workspaces)")
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(gone)
    }

    pub async fn load_workspace(&self, session: &str) -> anyhow::Result<Workspace> {
        match &self.backend {
            Backend::Postgres { pool, .. } => {
                let row: Option<(sqlx::types::Json<Workspace>,)> =
                    sqlx::query_as("SELECT workspace FROM agent_workspaces WHERE session_id = $1")
                        .bind(session)
                        .fetch_optional(pool)
                        .await?;
                Ok(row.map(|(workspace,)| workspace.0).unwrap_or_default())
            }
            Backend::Memory { workspaces, .. } => Ok(workspaces
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(session)
                .cloned()
                .unwrap_or_default()),
        }
    }

    pub async fn save_workspace(&self, session: &str, workspace: &Workspace) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Postgres { pool, .. } => {
                sqlx::query(
                    "INSERT INTO agent_workspaces (session_id, workspace) VALUES ($1, $2)
                     ON CONFLICT (session_id) DO UPDATE SET workspace = EXCLUDED.workspace, updated_at = now()",
                )
                .bind(session)
                .bind(sqlx::types::Json(workspace))
                .execute(pool)
                .await?;
            }
            Backend::Memory { workspaces, .. } => {
                workspaces
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(session.to_owned(), workspace.clone());
            }
        }
        Ok(())
    }

    /// Forgets the session's history and workspace.
    pub async fn reset(&self, session: &str) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Postgres { pool, .. } => {
                // rig's store has no delete, so its rows go directly: a
                // conversation is the log named after the session.
                let mut tx = pool.begin().await?;
                for statement in [
                    "DELETE FROM rig_memory_log_chunks WHERE name = $1",
                    "DELETE FROM rig_memory_logs WHERE name = $1",
                    "DELETE FROM agent_workspaces WHERE session_id = $1",
                ] {
                    sqlx::query(statement).bind(session).execute(&mut *tx).await?;
                }
                tx.commit().await?;
            }
            Backend::Memory { workspaces, .. } => {
                // MemoryStore cannot delete one log either; the UI starts a new
                // session id after a reset, so the old history is just unused.
                workspaces.lock().unwrap_or_else(PoisonError::into_inner).remove(session);
            }
        }
        Ok(())
    }

    /// Whether the database answers; always true in memory.
    pub async fn ready(&self) -> bool {
        match &self.backend {
            Backend::Postgres { pool, .. } => sqlx::query("SELECT 1").execute(pool).await.is_ok(),
            Backend::Memory { .. } => true,
        }
    }
}
