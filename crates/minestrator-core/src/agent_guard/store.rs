//! Stockage de la garde : `agent_guard.db` (SQLite, mode WAL) dans le dossier de données de
//! l'app, partagé par l'app et les processus MCP (`minestrator-mcp`, `<app> --mcp`) : l'app voit
//! les actions d'un client MCP externe et peut annuler ses modifications.
//!
//! Tables :
//! - `agent_actions` : journal des appels reçus par le protocole MCP (arguments anonymisés) ;
//! - `agent_changesets` / `agent_steps` : modifications regroupées par session, avec leurs
//!   opérations inverses (JSON, sans contenu) ;
//! - `agent_blobs` : copies des fichiers nécessaires à l'annulation (fichiers du serveur de
//!   l'utilisateur), plafonnées et effacées avec leur changeset.
//!
//! Aucune écriture ici ne doit faire échouer un outil : les erreurs sont journalisées (tracing)
//! et l'étape devient non annulable.

use super::undo::UndoOp;
use super::{
    ActionOutcome, AgentAction, AgentActionQuery, ChangeOrigin, ChangeStep, Changeset, StepState,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

const DAY: i64 = 86_400;
/// Rétention du journal : écritures 30 jours, lectures 7 jours, 20 000 lignes au plus.
pub(crate) const WRITE_RETENTION_DAYS: i64 = 30;
pub(crate) const READ_RETENTION_DAYS: i64 = 7;
pub(crate) const JOURNAL_MAX_ROWS: i64 = 20_000;
/// Rétention des changesets (et de leurs copies) : 14 jours après leur dernière étape.
pub(crate) const CHANGESET_RETENTION_DAYS: i64 = 14;
/// Plafond de toutes les copies gardées ; au-delà, les plus anciennes sont effacées et leurs
/// étapes deviennent non annulables.
pub(crate) const STORE_CAP: u64 = 256 * super::undo::MIB;
/// Note posée sur une étape dont les copies ont été effacées pour tenir le plafond.
const EXPIRED: &str = "copies effacées pour tenir le plafond de stockage : plus annulable";

/// Une ligne du journal à écrire.
pub(crate) struct NewAction<'a> {
    pub ts: i64,
    pub session: &'a str,
    pub client: Option<&'a str>,
    pub tool: &'a str,
    pub write: bool,
    pub servers: &'a [i64],
    pub args: &'a str,
    pub outcome: ActionOutcome,
    pub detail: Option<&'a str>,
    pub duration_ms: i64,
    pub step: Option<(&'a str, i64)>,
}

/// Une étape à enregistrer.
pub(crate) struct NewStep<'a> {
    pub changeset: &'a str,
    pub origin: ChangeOrigin,
    pub client: Option<&'a str>,
    pub ts: i64,
    pub tool: &'a str,
    pub server_id: Option<i64>,
    pub target: &'a str,
    /// `None` : étape non annulable.
    pub undo: Option<&'a [UndoOp]>,
    pub note: Option<&'a str>,
    pub failed: bool,
    pub blobs: &'a [Vec<u8>],
}

/// Étape relue pour l'annulation.
pub(crate) struct StoredStep {
    pub seq: i64,
    pub tool: String,
    pub server_id: Option<i64>,
    pub target: String,
    pub undo: Option<Vec<UndoOp>>,
    pub note: Option<String>,
}

pub(crate) struct GuardStore {
    conn: Mutex<Connection>,
    last_prune: AtomicI64,
}

fn db_err<E: std::fmt::Display>(e: E) -> String {
    format!("journal des agents : {e}")
}

/// Serveurs d'une ligne du journal, encadrés de virgules pour un filtre `LIKE '%,id,%'`.
fn servers_col(ids: &[i64]) -> String {
    let mut s = String::from(",");
    for id in ids {
        s.push_str(&id.to_string());
        s.push(',');
    }
    s
}

fn parse_servers(s: &str) -> Vec<i64> {
    s.split(',').filter_map(|t| t.parse().ok()).collect()
}

impl GuardStore {
    /// Ouvre `dir/agent_guard.db` ; repli en mémoire si le fichier est inaccessible.
    #[cfg_attr(test, allow(dead_code))] // les tests travaillent en mémoire
    pub(crate) fn open(dir: &Path) -> Self {
        let conn = Connection::open(dir.join("agent_guard.db")).unwrap_or_else(|e| {
            tracing::warn!("journal des agents en mémoire (fichier indisponible : {e})");
            Connection::open_in_memory().expect("connexion SQLite (mémoire)")
        });
        Self::with(conn)
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self::with(Connection::open_in_memory().expect("SQLite en mémoire"))
    }

    fn with(conn: Connection) -> Self {
        // WAL + attente : l'app et les processus MCP écrivent dans le même fichier.
        let _ = conn.busy_timeout(std::time::Duration::from_secs(5));
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        if let Err(e) = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_actions (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                ts          INTEGER NOT NULL,
                session     TEXT    NOT NULL,
                client      TEXT,
                tool        TEXT    NOT NULL,
                write       INTEGER NOT NULL,
                servers     TEXT    NOT NULL,
                args        TEXT    NOT NULL,
                outcome     TEXT    NOT NULL,
                detail      TEXT,
                duration_ms INTEGER NOT NULL,
                changeset   TEXT,
                step        INTEGER
             );
             CREATE INDEX IF NOT EXISTS idx_agent_actions_ts ON agent_actions(ts);
             CREATE INDEX IF NOT EXISTS idx_agent_actions_session ON agent_actions(session, ts);
             CREATE TABLE IF NOT EXISTS agent_changesets (
                id      TEXT    PRIMARY KEY,
                origin  TEXT    NOT NULL,
                client  TEXT,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_agent_changesets_updated ON agent_changesets(updated);
             CREATE TABLE IF NOT EXISTS agent_steps (
                changeset TEXT    NOT NULL,
                seq       INTEGER NOT NULL,
                ts        INTEGER NOT NULL,
                tool      TEXT    NOT NULL,
                server_id INTEGER,
                target    TEXT    NOT NULL,
                undo      TEXT,
                note      TEXT,
                failed    INTEGER NOT NULL DEFAULT 0,
                bytes     INTEGER NOT NULL DEFAULT 0,
                state     TEXT    NOT NULL DEFAULT 'active',
                undone_at INTEGER,
                PRIMARY KEY (changeset, seq)
             );
             CREATE TABLE IF NOT EXISTS agent_blobs (
                changeset TEXT    NOT NULL,
                seq       INTEGER NOT NULL,
                idx       INTEGER NOT NULL,
                data      BLOB    NOT NULL,
                PRIMARY KEY (changeset, seq, idx)
             );",
        ) {
            tracing::warn!("journal des agents : création des tables : {e}");
        }
        let store = Self { conn: Mutex::new(conn), last_prune: AtomicI64::new(0) };
        store.prune(crate::store::now_secs());
        store
    }

    // --- Journal -------------------------------------------------------------------------------

    pub(crate) fn insert_action(&self, a: &NewAction) -> Result<i64, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO agent_actions (ts, session, client, tool, write, servers, args, outcome, detail,
                                        duration_ms, changeset, step)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                a.ts,
                a.session,
                a.client,
                a.tool,
                a.write,
                servers_col(a.servers),
                a.args,
                a.outcome.as_str(),
                a.detail,
                a.duration_ms,
                a.step.map(|(c, _)| c),
                a.step.map(|(_, s)| s),
            ],
        )
        .map_err(db_err)?;
        Ok(conn.last_insert_rowid())
    }

    /// Journal filtré, du plus récent au plus ancien.
    pub(crate) fn actions(&self, q: &AgentActionQuery) -> Result<Vec<AgentAction>, String> {
        let limit = q.limit.unwrap_or(50).clamp(1, 500);
        let like = q.server_id.map(|id| format!("%,{id},%"));
        let conn = self.conn.lock().unwrap();
        let mut st = conn
            .prepare(
                "SELECT id, ts, session, client, tool, write, servers, args, outcome, detail, duration_ms,
                        changeset, step
                   FROM agent_actions
                  WHERE (?1 IS NULL OR servers LIKE ?1)
                    AND (?2 IS NULL OR session = ?2)
                    AND (?3 IS NULL OR ts >= ?3)
                    AND (?4 = 0 OR write = 1)
                  ORDER BY id DESC
                  LIMIT ?5",
            )
            .map_err(db_err)?;
        let rows = st
            .query_map(params![like, q.session, q.since, q.only_writes, limit], |r| {
                let args: String = r.get(7)?;
                let outcome: String = r.get(8)?;
                Ok(AgentAction {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    session: r.get(2)?,
                    client: r.get(3)?,
                    tool: r.get(4)?,
                    write: r.get(5)?,
                    servers: parse_servers(&r.get::<_, String>(6)?),
                    args: serde_json::from_str(&args).unwrap_or(serde_json::Value::String(args)),
                    outcome: ActionOutcome::parse(&outcome),
                    detail: r.get(9)?,
                    duration_ms: r.get(10)?,
                    changeset: r.get(11)?,
                    step: r.get(12)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    // --- Changesets ----------------------------------------------------------------------------

    /// Octets de copies déjà gardés pour un changeset.
    pub(crate) fn changeset_bytes(&self, id: &str) -> i64 {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COALESCE(SUM(bytes), 0) FROM agent_steps WHERE changeset = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap_or(0)
    }

    /// Ajoute une étape (et ses copies) ; renvoie son numéro dans le changeset.
    pub(crate) fn add_step(&self, s: &NewStep) -> Result<i64, String> {
        let undo = match s.undo {
            Some(ops) => Some(serde_json::to_string(ops).map_err(db_err)?),
            None => None,
        };
        let bytes: i64 = s.blobs.iter().map(|b| b.len() as i64).sum();
        let mut conn = self.conn.lock().unwrap();
        // Transaction immédiate : deux processus (app et agent CLI du Copilote) peuvent écrire dans
        // le même changeset ; le numéro d'étape doit rester unique.
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        tx.execute(
            "INSERT INTO agent_changesets (id, origin, client, created, updated) VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET updated = excluded.updated,
                                           client = COALESCE(agent_changesets.client, excluded.client)",
            params![s.changeset, s.origin.as_str(), s.client, s.ts],
        )
        .map_err(db_err)?;
        let seq: i64 = tx
            .query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_steps WHERE changeset = ?1", [s.changeset], |r| r.get(0))
            .map_err(db_err)?;
        tx.execute(
            "INSERT INTO agent_steps (changeset, seq, ts, tool, server_id, target, undo, note, failed, bytes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![s.changeset, seq, s.ts, s.tool, s.server_id, s.target, undo, s.note, s.failed, bytes],
        )
        .map_err(db_err)?;
        for (idx, data) in s.blobs.iter().enumerate() {
            tx.execute(
                "INSERT INTO agent_blobs (changeset, seq, idx, data) VALUES (?1, ?2, ?3, ?4)",
                params![s.changeset, seq, idx as i64, data],
            )
            .map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        drop(conn);
        if bytes > 0 {
            self.prune_if_due(s.ts);
        }
        Ok(seq)
    }

    /// Changesets récents (le plus récemment modifié d'abord), avec leurs étapes. `server` :
    /// seulement ceux qui touchent ce serveur, et seulement leurs étapes sur ce serveur.
    pub(crate) fn changesets(&self, limit: u32, server: Option<i64>, id: Option<&str>) -> Result<Vec<Changeset>, String> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn
            .prepare(
                "SELECT c.id, c.origin, c.client, c.created, c.updated FROM agent_changesets c
                  WHERE (?1 IS NULL OR c.id = ?1)
                    AND (?2 IS NULL OR EXISTS (SELECT 1 FROM agent_steps s
                                               WHERE s.changeset = c.id AND s.server_id = ?2))
                  ORDER BY c.updated DESC, c.id
                  LIMIT ?3",
            )
            .map_err(db_err)?;
        let heads = st
            .query_map(params![id, server, limit.clamp(1, 200)], |r| {
                Ok(Changeset {
                    id: r.get(0)?,
                    origin: ChangeOrigin::parse(&r.get::<_, String>(1)?),
                    client: r.get(2)?,
                    created: r.get(3)?,
                    updated: r.get(4)?,
                    servers: Vec::new(),
                    bytes: 0,
                    steps: Vec::new(),
                })
            })
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        let mut steps_st = conn
            .prepare(
                "SELECT seq, ts, tool, server_id, target, undo IS NOT NULL, note, failed, state, undone_at, bytes
                   FROM agent_steps WHERE changeset = ?1 AND (?2 IS NULL OR server_id = ?2) ORDER BY seq",
            )
            .map_err(db_err)?;
        let mut out = Vec::with_capacity(heads.len());
        for mut cs in heads {
            let rows = steps_st
                .query_map(params![cs.id, server], |r| {
                    Ok((
                        ChangeStep {
                            seq: r.get(0)?,
                            ts: r.get(1)?,
                            tool: r.get(2)?,
                            server_id: r.get(3)?,
                            target: r.get(4)?,
                            undoable: r.get(5)?,
                            note: r.get(6)?,
                            failed: r.get(7)?,
                            state: StepState::parse(&r.get::<_, String>(8)?),
                            undone_at: r.get(9)?,
                        },
                        r.get::<_, i64>(10)?,
                    ))
                })
                .map_err(db_err)?;
            for row in rows {
                let (step, bytes) = row.map_err(db_err)?;
                cs.bytes += bytes;
                if let Some(id) = step.server_id {
                    if !cs.servers.contains(&id) {
                        cs.servers.push(id);
                    }
                }
                cs.steps.push(step);
            }
            cs.servers.sort_unstable();
            out.push(cs);
        }
        Ok(out)
    }

    /// Serveurs touchés par les étapes encore actives d'un changeset (`None` : changeset inconnu).
    pub(crate) fn changeset_servers(&self, id: &str) -> Result<Option<Vec<i64>>, String> {
        let conn = self.conn.lock().unwrap();
        let known: Option<i64> = conn
            .query_row("SELECT 1 FROM agent_changesets WHERE id = ?1", [id], |r| r.get(0))
            .optional()
            .map_err(db_err)?;
        if known.is_none() {
            return Ok(None);
        }
        let mut st = conn
            .prepare(
                "SELECT DISTINCT server_id FROM agent_steps
                  WHERE changeset = ?1 AND state = 'active' AND server_id IS NOT NULL ORDER BY server_id",
            )
            .map_err(db_err)?;
        let ids = st
            .query_map([id], |r| r.get(0))
            .map_err(db_err)?
            .collect::<Result<Vec<i64>, _>>()
            .map_err(db_err)?;
        Ok(Some(ids))
    }

    /// Étapes encore actives d'un changeset, de la plus récente à la plus ancienne.
    pub(crate) fn active_steps(&self, id: &str) -> Result<Vec<StoredStep>, String> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn
            .prepare(
                "SELECT seq, tool, server_id, target, undo, note FROM agent_steps
                  WHERE changeset = ?1 AND state = 'active' ORDER BY seq DESC",
            )
            .map_err(db_err)?;
        let rows = st
            .query_map([id], |r| {
                let undo: Option<String> = r.get(4)?;
                Ok(StoredStep {
                    seq: r.get(0)?,
                    tool: r.get(1)?,
                    server_id: r.get(2)?,
                    target: r.get(3)?,
                    // JSON illisible (version future ?) : traité comme non annulable.
                    undo: undo.and_then(|u| serde_json::from_str(&u).ok()),
                    note: r.get(5)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Copies d'une étape, dans l'ordre de leurs index.
    pub(crate) fn blobs(&self, id: &str, seq: i64) -> Result<Vec<Vec<u8>>, String> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn
            .prepare("SELECT data FROM agent_blobs WHERE changeset = ?1 AND seq = ?2 ORDER BY idx")
            .map_err(db_err)?;
        let rows = st.query_map(params![id, seq], |r| r.get(0)).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Marque une étape annulée ou passée ; les copies d'une étape annulée ne servent plus.
    pub(crate) fn set_state(&self, id: &str, seq: i64, state: StepState, now: i64) -> Result<(), String> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE agent_steps SET state = ?3, undone_at = ?4 WHERE changeset = ?1 AND seq = ?2",
            params![id, seq, state.as_str(), now],
        )
        .map_err(db_err)?;
        if state == StepState::Undone {
            conn.execute("DELETE FROM agent_blobs WHERE changeset = ?1 AND seq = ?2", params![id, seq])
                .map_err(db_err)?;
            conn.execute("UPDATE agent_steps SET bytes = 0 WHERE changeset = ?1 AND seq = ?2", params![id, seq])
                .map_err(db_err)?;
        }
        Ok(())
    }

    // --- Rétention -----------------------------------------------------------------------------

    /// Purge au plus une fois par heure (et à l'ouverture).
    pub(crate) fn prune_if_due(&self, now: i64) {
        let last = self.last_prune.load(Ordering::Relaxed);
        if now - last >= 3600 {
            self.prune(now);
        }
    }

    pub(crate) fn prune(&self, now: i64) {
        self.prune_with(now, STORE_CAP);
    }

    /// Purge avec un plafond de copies donné (`STORE_CAP` hors tests).
    fn prune_with(&self, now: i64, cap: u64) {
        self.last_prune.store(now, Ordering::Relaxed);
        if let Err(e) = self.prune_inner(now, cap) {
            tracing::warn!("journal des agents : purge : {e}");
        }
    }

    fn prune_inner(&self, now: i64, cap: u64) -> Result<(), String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        tx.execute(
            "DELETE FROM agent_actions WHERE (write = 1 AND ts < ?1) OR (write = 0 AND ts < ?2)",
            params![now - WRITE_RETENTION_DAYS * DAY, now - READ_RETENTION_DAYS * DAY],
        )
        .map_err(db_err)?;
        tx.execute(
            "DELETE FROM agent_actions WHERE id <= (SELECT id FROM agent_actions ORDER BY id DESC LIMIT 1 OFFSET ?1)",
            [JOURNAL_MAX_ROWS],
        )
        .map_err(db_err)?;
        let old = now - CHANGESET_RETENTION_DAYS * DAY;
        for table in ["agent_blobs", "agent_steps"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE changeset IN (SELECT id FROM agent_changesets WHERE updated < ?1)"),
                [old],
            )
            .map_err(db_err)?;
        }
        tx.execute("DELETE FROM agent_changesets WHERE updated < ?1", [old]).map_err(db_err)?;
        // Plafond des copies : on efface celles des changesets les plus anciens d'abord.
        let mut total: i64 = tx
            .query_row("SELECT COALESCE(SUM(bytes), 0) FROM agent_steps", [], |r| r.get(0))
            .map_err(db_err)?;
        if total as u64 > cap {
            let ids: Vec<(String, i64)> = {
                let mut st = tx
                    .prepare(
                        "SELECT c.id, SUM(s.bytes) FROM agent_changesets c JOIN agent_steps s ON s.changeset = c.id
                          GROUP BY c.id HAVING SUM(s.bytes) > 0 ORDER BY c.updated, c.id",
                    )
                    .map_err(db_err)?;
                let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(db_err)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
            };
            for (id, bytes) in ids {
                if total as u64 <= cap {
                    break;
                }
                tx.execute("DELETE FROM agent_blobs WHERE changeset = ?1", [&id]).map_err(db_err)?;
                tx.execute(
                    "UPDATE agent_steps SET undo = NULL, note = ?2, bytes = 0
                      WHERE changeset = ?1 AND bytes > 0 AND state = 'active'",
                    params![id, EXPIRED],
                )
                .map_err(db_err)?;
                tx.execute("UPDATE agent_steps SET bytes = 0 WHERE changeset = ?1", [&id]).map_err(db_err)?;
                total -= bytes;
            }
        }
        tx.commit().map_err(db_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action<'a>(ts: i64, session: &'a str, tool: &'a str, write: bool, servers: &'a [i64]) -> NewAction<'a> {
        NewAction {
            ts,
            session,
            client: Some("claude-code"),
            tool,
            write,
            servers,
            args: r#"{"path":"/a"}"#,
            outcome: ActionOutcome::Ok,
            detail: None,
            duration_ms: 12,
            step: None,
        }
    }

    fn step<'a>(cs: &'a str, ts: i64, server: i64, blobs: &'a [Vec<u8>], undo: Option<&'a [UndoOp]>) -> NewStep<'a> {
        NewStep {
            changeset: cs,
            origin: ChangeOrigin::Mcp,
            client: Some("claude-code"),
            ts,
            tool: "write_file",
            server_id: Some(server),
            target: "/a",
            undo,
            note: None,
            failed: false,
            blobs,
        }
    }

    #[test]
    fn journal_filters_and_order() {
        let s = GuardStore::in_memory();
        let now = crate::store::now_secs();
        s.insert_action(&action(now - 100, "mcp:a", "read_file", false, &[1])).unwrap();
        s.insert_action(&action(now - 50, "mcp:a", "write_file", true, &[1, 12])).unwrap();
        s.insert_action(&action(now - 10, "mcp:b", "write_file", true, &[2])).unwrap();
        let all = s.actions(&AgentActionQuery::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all[0].ts > all[1].ts, "plus récent d'abord");
        assert_eq!(all[1].servers, vec![1, 12]);
        assert_eq!(all[1].args["path"], "/a");
        let q = |f: fn(&mut AgentActionQuery)| {
            let mut q = AgentActionQuery::default();
            f(&mut q);
            s.actions(&q).unwrap().len()
        };
        assert_eq!(q(|q| q.server_id = Some(1)), 2, "pas de faux positif sur 12");
        assert_eq!(q(|q| q.server_id = Some(2)), 1);
        assert_eq!(q(|q| q.only_writes = true), 2);
        assert_eq!(q(|q| q.session = Some("mcp:b".into())), 1);
        assert_eq!(q(|q| q.since = Some(crate::store::now_secs() - 60)), 2);
        assert_eq!(q(|q| q.limit = Some(1)), 1);
    }

    #[test]
    fn retention_bounds_the_journal_and_changesets() {
        let s = GuardStore::in_memory();
        let now = crate::store::now_secs();
        s.insert_action(&action(now - 8 * DAY, "x", "read_file", false, &[])).unwrap();
        s.insert_action(&action(now - 8 * DAY, "x", "write_file", true, &[])).unwrap();
        s.insert_action(&action(now - 31 * DAY, "x", "write_file", true, &[])).unwrap();
        let blobs = vec![b"ancien".to_vec()];
        s.add_step(&step("vieux", now - 15 * DAY, 1, &blobs, Some(&[]))).unwrap();
        s.add_step(&step("récent", now, 1, &blobs, Some(&[]))).unwrap();
        s.prune(now);
        let left = s.actions(&AgentActionQuery::default()).unwrap();
        assert_eq!(left.len(), 1, "lecture de 8 jours et écriture de 31 jours purgées");
        assert!(left[0].write);
        let cs = s.changesets(10, None, None).unwrap();
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].id, "récent");
        assert!(s.blobs("vieux", 1).unwrap().is_empty());
    }

    #[test]
    fn steps_are_numbered_and_listed_by_server() {
        let s = GuardStore::in_memory();
        let now = crate::store::now_secs();
        let blobs = vec![b"v1".to_vec(), b"v2".to_vec()];
        assert_eq!(s.add_step(&step("cs", now, 1, &blobs, Some(&[]))).unwrap(), 1);
        assert_eq!(s.add_step(&step("cs", now, 2, &[], None)).unwrap(), 2);
        assert_eq!(s.changeset_bytes("cs"), 4);
        let cs = &s.changesets(10, None, None).unwrap()[0];
        assert_eq!((cs.servers.clone(), cs.steps.len(), cs.bytes), (vec![1, 2], 2, 4));
        assert!(cs.steps[0].undoable && !cs.steps[1].undoable);
        let only2 = &s.changesets(10, Some(2), None).unwrap()[0];
        assert_eq!(only2.steps.len(), 1);
        assert!(s.changesets(10, Some(3), None).unwrap().is_empty());
        assert_eq!(s.changeset_servers("cs").unwrap(), Some(vec![1, 2]));
        assert_eq!(s.changeset_servers("inconnu").unwrap(), None);
        assert_eq!(s.blobs("cs", 1).unwrap(), blobs);
        let active = s.active_steps("cs").unwrap();
        assert_eq!(active.iter().map(|a| a.seq).collect::<Vec<_>>(), vec![2, 1]);
        s.set_state("cs", 1, StepState::Undone, now).unwrap();
        assert!(s.blobs("cs", 1).unwrap().is_empty(), "copies inutiles après annulation");
        assert_eq!(s.active_steps("cs").unwrap().len(), 1);
        assert_eq!(s.changeset_servers("cs").unwrap(), Some(vec![2]));
    }

    #[test]
    fn storage_cap_expires_oldest_copies_first() {
        let s = GuardStore::in_memory();
        let now = crate::store::now_secs();
        let big = vec![vec![0u8; 600]];
        s.add_step(&step("ancien", now - 100, 1, &big, Some(&[]))).unwrap();
        s.add_step(&step("nouveau", now, 1, &big, Some(&[]))).unwrap();
        s.prune_with(now, 1000);
        let cs = s.changesets(10, None, None).unwrap();
        let old = cs.iter().find(|c| c.id == "ancien").unwrap();
        let new = cs.iter().find(|c| c.id == "nouveau").unwrap();
        assert!(!old.steps[0].undoable && old.steps[0].note.as_deref() == Some(EXPIRED));
        assert!(new.steps[0].undoable);
        assert!(s.blobs("ancien", 1).unwrap().is_empty());
    }
}
