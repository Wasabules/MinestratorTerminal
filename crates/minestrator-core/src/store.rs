//! Historique local de métriques (SQLite embarqué).
//!
//! L'API ne fournit AUCUN historique — le cœur l'accumule lui-même. C'est l'un des
//! avantages structurels d'une app persistante par rapport au panel web.

use crate::error::{Error, Result};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// Un échantillon de métriques (renvoyé au frontend pour les graphes).
#[derive(Debug, Clone, Serialize)]
pub struct MetricSample {
    pub ts: i64,
    pub cpu: f64,
    pub mem: i64,
    pub mem_limit: i64,
    pub disk: i64,
    pub state: String,
}

pub struct MetricsStore {
    conn: Mutex<Connection>,
}

fn db_err<E: std::fmt::Display>(e: E) -> Error {
    Error::Unexpected(format!("stockage métriques: {e}"))
}

impl MetricsStore {
    /// Ouvre `dir/metrics.db` ; repli en mémoire si le fichier est inaccessible.
    pub fn open(dir: &Path) -> Self {
        let conn = Connection::open(dir.join("metrics.db"))
            .or_else(|e| {
                tracing::warn!("historique métriques en mémoire (fichier indisponible: {e})");
                Connection::open_in_memory()
            })
            .expect("connexion SQLite (mémoire)");
        let store = Self {
            conn: Mutex::new(conn),
        };
        store.init();
        store
    }

    fn init(&self) {
        let conn = self.conn.lock().unwrap();
        // WAL + busy_timeout : lecture/écriture concurrente entre l'app desktop (écrit)
        // et le binaire MCP (lit) sur le même fichier.
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metrics (
                server_id INTEGER NOT NULL,
                ts        INTEGER NOT NULL,
                cpu       REAL    NOT NULL,
                mem       INTEGER NOT NULL,
                mem_limit INTEGER NOT NULL,
                disk      INTEGER NOT NULL,
                state     TEXT    NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_metrics_server_ts ON metrics(server_id, ts);",
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert(
        &self,
        server_id: i64,
        ts: i64,
        cpu: f64,
        mem: i64,
        mem_limit: i64,
        disk: i64,
        state: &str,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO metrics (server_id, ts, cpu, mem, mem_limit, disk, state)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![server_id, ts, cpu, mem, mem_limit, disk, state],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Échantillons d'un serveur depuis `since_ts` (ordre chronologique).
    pub fn query(&self, server_id: i64, since_ts: i64) -> Result<Vec<MetricSample>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT ts, cpu, mem, mem_limit, disk, state FROM metrics
                 WHERE server_id = ?1 AND ts >= ?2 ORDER BY ts ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(rusqlite::params![server_id, since_ts], |r| {
                Ok(MetricSample {
                    ts: r.get(0)?,
                    cpu: r.get(1)?,
                    mem: r.get(2)?,
                    mem_limit: r.get(3)?,
                    disk: r.get(4)?,
                    state: r.get(5)?,
                })
            })
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(db_err)?);
        }
        Ok(out)
    }

    /// Supprime les échantillons antérieurs à `older_than_ts`.
    pub fn prune(&self, older_than_ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM metrics WHERE ts < ?1",
            rusqlite::params![older_than_ts],
        )
        .map_err(db_err)?;
        Ok(())
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
