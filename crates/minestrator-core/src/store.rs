//! Historique local de métriques (SQLite embarqué).
//!
//! L'API ne fournit AUCUN historique — le cœur l'accumule lui-même. C'est l'un des
//! avantages structurels d'une app persistante par rapport au panel web.
//!
//! La même base garde aussi :
//! - un **journal des alertes** (table `alerts`) : le service d'arrière-plan y consigne ce qu'il
//!   détecte pendant que l'app est fermée, et l'app le relit par IPC (voir [`crate::service`]) ;
//! - les **observations d'état** des serveurs (table `uptime`), base du suivi de disponibilité
//!   (voir [`crate::uptime`]) ;
//! - le **journal des automatisations** (table `automation_runs`) : exécutions des tâches
//!   planifiées et redémarrages automatiques (voir [`crate::automation`]).
//!
//! Les deux derniers sont gardés plus longtemps que les métriques (disponibilité sur 30 jours).

use crate::automation::AutomationRun;
use crate::error::{Error, Result};
use crate::events::Alert;
use crate::uptime::StateRow;
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Un échantillon de métriques (renvoyé au frontend pour les graphes).
///
/// `Deserialize` : l'historique collecté par le service d'arrière-plan transite par IPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MetricSample {
    pub ts: i64,
    pub cpu: f64,
    pub mem: i64,
    pub mem_limit: i64,
    pub disk: i64,
    pub state: String,
}

/// Une alerte consignée, avec son identifiant croissant (curseur de lecture côté app).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredAlert {
    pub id: i64,
    #[serde(flatten)]
    pub alert: Alert,
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
             CREATE INDEX IF NOT EXISTS idx_metrics_server_ts ON metrics(server_id, ts);
             CREATE TABLE IF NOT EXISTS alerts (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                server_id   INTEGER NOT NULL,
                server_name TEXT    NOT NULL,
                kind        TEXT    NOT NULL,
                severity    TEXT    NOT NULL,
                message     TEXT    NOT NULL,
                ts          INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS idx_alerts_ts ON alerts(ts);
             CREATE TABLE IF NOT EXISTS uptime (
                server_id INTEGER NOT NULL,
                ts        INTEGER NOT NULL,
                state     TEXT    NOT NULL,
                cause     TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_uptime_server_ts ON uptime(server_id, ts);
             CREATE TABLE IF NOT EXISTS automation_runs (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                account_id   INTEGER NOT NULL,
                server_id    INTEGER NOT NULL,
                task_id      TEXT,
                task_name    TEXT,
                kind         TEXT    NOT NULL,
                scheduled_ts INTEGER,
                started_ts   INTEGER NOT NULL,
                finished_ts  INTEGER,
                status       TEXT    NOT NULL,
                message      TEXT    NOT NULL,
                details      TEXT
             );
             CREATE INDEX IF NOT EXISTS idx_runs_server ON automation_runs(server_id, started_ts);
             CREATE INDEX IF NOT EXISTS idx_runs_task ON automation_runs(task_id);",
        );
        // Migration : colonne `details` des alertes (ajoutée en 0.7). Une base créée par une
        // version antérieure ne l'a pas, et `ALTER TABLE … ADD COLUMN` n'a pas de `IF NOT EXISTS`.
        let has_details = conn
            .prepare("SELECT 1 FROM pragma_table_info('alerts') WHERE name = 'details'")
            .and_then(|mut st| st.exists([]))
            .unwrap_or(true);
        if !has_details {
            if let Err(e) = conn.execute_batch("ALTER TABLE alerts ADD COLUMN details TEXT") {
                tracing::warn!("migration du journal d'alertes : {e}");
            }
        }
    }

    /// Consigne une alerte ; renvoie son identifiant (strictement croissant).
    pub fn insert_alert(&self, a: &Alert) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO alerts (server_id, server_name, kind, severity, message, ts, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                a.server_id,
                a.server_name,
                a.kind,
                a.severity,
                a.message,
                a.ts,
                a.details
            ],
        )
        .map_err(db_err)?;
        Ok(conn.last_insert_rowid())
    }

    /// Alertes consignées, par identifiant croissant : celles d'identifiant `> after_id` et de
    /// date `>= since_ts`, au plus `limit`. Renvoie aussi le plus grand identifiant existant
    /// (0 si aucun), dont l'app se sert comme curseur.
    pub fn alerts(
        &self,
        after_id: Option<i64>,
        since_ts: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<StoredAlert>, i64)> {
        let conn = self.conn.lock().unwrap();
        let last_id: i64 = conn
            .query_row("SELECT COALESCE(MAX(id), 0) FROM alerts", [], |r| r.get(0))
            .map_err(db_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT id, server_id, server_name, kind, severity, message, ts, details FROM alerts
                 WHERE id > ?1 AND ts >= ?2 ORDER BY id ASC LIMIT ?3",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(
                rusqlite::params![after_id.unwrap_or(0), since_ts.unwrap_or(i64::MIN), limit],
                |r| {
                    Ok(StoredAlert {
                        id: r.get(0)?,
                        alert: Alert {
                            server_id: r.get(1)?,
                            server_name: r.get(2)?,
                            kind: r.get(3)?,
                            severity: r.get(4)?,
                            message: r.get(5)?,
                            ts: r.get(6)?,
                            details: r.get(7)?,
                        },
                    })
                },
            )
            .map_err(db_err)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(db_err)?);
        }
        Ok((out, last_id))
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

    /// Supprime les échantillons et les alertes antérieurs à `older_than_ts`.
    pub fn prune(&self, older_than_ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM metrics WHERE ts < ?1",
            rusqlite::params![older_than_ts],
        )
        .map_err(db_err)?;
        conn.execute(
            "DELETE FROM alerts WHERE ts < ?1",
            rusqlite::params![older_than_ts],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Supprime les observations d'état et le journal des automatisations antérieurs à
    /// `older_than_ts` (rétention plus longue que les métriques : disponibilité sur 30 jours).
    pub fn prune_long(&self, older_than_ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM uptime WHERE ts < ?1", rusqlite::params![older_than_ts])
            .map_err(db_err)?;
        conn.execute(
            "DELETE FROM automation_runs WHERE started_ts < ?1",
            rusqlite::params![older_than_ts],
        )
        .map_err(db_err)?;
        Ok(())
    }

    // --- Suivi de disponibilité ---------------------------------------------------------

    /// Consigne une observation d'état d'un serveur. `cause` (`crash` | `stop`) : seulement pour
    /// une transition vers un état « hors service » dont on connaît l'origine.
    pub fn insert_state(&self, server_id: i64, ts: i64, state: &str, cause: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO uptime (server_id, ts, state, cause) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![server_id, ts, state, cause],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Observations d'état d'un serveur depuis `since_ts` (ordre chronologique).
    pub fn state_rows(&self, server_id: i64, since_ts: i64) -> Result<Vec<StateRow>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT ts, state, cause FROM uptime WHERE server_id = ?1 AND ts >= ?2
                 ORDER BY ts ASC, rowid ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(rusqlite::params![server_id, since_ts], |r| {
                Ok(StateRow { ts: r.get(0)?, state: r.get(1)?, cause: r.get(2)? })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    // --- Journal des automatisations ------------------------------------------------------

    /// Consigne une exécution ; renvoie son identifiant.
    pub fn insert_run(&self, run: &AutomationRun) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO automation_runs (account_id, server_id, task_id, task_name, kind,
                scheduled_ts, started_ts, finished_ts, status, message, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                run.account_id,
                run.server_id,
                run.task_id,
                run.task_name,
                run.kind,
                run.scheduled_ts,
                run.started_ts,
                run.finished_ts,
                run.status,
                run.message,
                run.details
            ],
        )
        .map_err(db_err)?;
        Ok(conn.last_insert_rowid())
    }

    /// Exécutions d'un compte (d'un seul serveur si `server_id`), des plus récentes aux plus
    /// anciennes, au plus `limit`.
    pub fn runs(&self, account_id: i64, server_id: Option<i64>, limit: u32) -> Result<Vec<AutomationRun>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, account_id, server_id, task_id, task_name, kind, scheduled_ts,
                        started_ts, finished_ts, status, message, details
                 FROM automation_runs
                 WHERE account_id = ?1 AND (?2 IS NULL OR server_id = ?2)
                 ORDER BY started_ts DESC, id DESC LIMIT ?3",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(rusqlite::params![account_id, server_id, limit], |r| {
                Ok(AutomationRun {
                    id: r.get(0)?,
                    account_id: r.get(1)?,
                    server_id: r.get(2)?,
                    task_id: r.get(3)?,
                    task_name: r.get(4)?,
                    kind: r.get(5)?,
                    scheduled_ts: r.get(6)?,
                    started_ts: r.get(7)?,
                    finished_ts: r.get(8)?,
                    status: r.get(9)?,
                    message: r.get(10)?,
                    details: r.get(11)?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }

    /// Dernière occurrence planifiée déjà traitée (exécutée, manquée ou ignorée) d'une tâche.
    pub fn last_scheduled(&self, task_id: &str) -> Result<Option<i64>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT MAX(scheduled_ts) FROM automation_runs WHERE task_id = ?1",
            rusqlite::params![task_id],
            |r| r.get::<_, Option<i64>>(0),
        )
        .optional()
        .map(Option::flatten)
        .map_err(db_err)
    }

    /// Horodatages des redémarrages automatiques **lancés** pour un serveur depuis `since_ts`
    /// (limite horaire), du plus ancien au plus récent.
    pub fn auto_restarts_since(&self, server_id: i64, since_ts: i64) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT started_ts FROM automation_runs
                 WHERE server_id = ?1 AND kind = 'auto_restart' AND status = 'ok'
                   AND started_ts >= ?2
                 ORDER BY started_ts ASC",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(rusqlite::params![server_id, since_ts], |r| r.get(0))
            .map_err(db_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>().map_err(db_err)
    }
}

pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Dossier temporaire unique, supprimé à la fin du test (après fermeture de la base).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("mst-test-store-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample(store: &MetricsStore, server_id: i64, ts: i64) {
        store
            .insert(server_id, ts, ts as f64 / 10.0, ts * 1000, 4096, ts * 10, "running")
            .unwrap();
    }

    fn timestamps(samples: &[MetricSample]) -> Vec<i64> {
        samples.iter().map(|s| s.ts).collect()
    }

    #[test]
    fn query_is_chronological_and_filtered_by_server_and_time() {
        let dir = TempDir::new();
        let store = MetricsStore::open(&dir.0);
        for ts in [300, 100, 200] {
            sample(&store, 1, ts);
        }
        sample(&store, 2, 150);

        let all = store.query(1, 0).unwrap();
        assert_eq!(timestamps(&all), [100, 200, 300]);
        let first = &all[0];
        assert_eq!(first.cpu, 10.0);
        assert_eq!((first.mem, first.mem_limit, first.disk), (100_000, 4096, 1000));
        assert_eq!(first.state, "running");

        // Borne `since` incluse.
        assert_eq!(timestamps(&store.query(1, 200).unwrap()), [200, 300]);
        assert_eq!(timestamps(&store.query(2, 0).unwrap()), [150]);
        assert!(store.query(3, 0).unwrap().is_empty());
        assert!(dir.0.join("metrics.db").exists());
    }

    #[test]
    fn prune_drops_only_older_samples_of_every_server() {
        let dir = TempDir::new();
        let store = MetricsStore::open(&dir.0);
        for ts in [100, 200, 300] {
            sample(&store, 1, ts);
            sample(&store, 2, ts);
        }
        store.prune(200).unwrap();
        // Borne stricte : l'échantillon à 200 est conservé.
        assert_eq!(timestamps(&store.query(1, 0).unwrap()), [200, 300]);
        assert_eq!(timestamps(&store.query(2, 0).unwrap()), [200, 300]);
    }

    #[test]
    fn history_survives_a_reopen() {
        let dir = TempDir::new();
        {
            let store = MetricsStore::open(&dir.0);
            sample(&store, 1, 100);
        }
        let store = MetricsStore::open(&dir.0);
        assert_eq!(timestamps(&store.query(1, 0).unwrap()), [100]);
    }

    #[test]
    fn falls_back_to_memory_when_the_file_cannot_be_opened() {
        let dir = TempDir::new();
        // Dossier inexistant : `metrics.db` ne peut pas y être créé.
        let missing = dir.0.join("absent");
        let store = MetricsStore::open(&missing);
        sample(&store, 1, 100);
        assert_eq!(timestamps(&store.query(1, 0).unwrap()), [100]);
        assert!(!missing.exists());
    }

    #[test]
    fn metric_sample_serializes_with_front_keys() {
        let s = MetricSample {
            ts: 1,
            cpu: 2.5,
            mem: 3,
            mem_limit: 4,
            disk: 5,
            state: "offline".into(),
        };
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            serde_json::json!({
                "ts": 1, "cpu": 2.5, "mem": 3, "mem_limit": 4, "disk": 5, "state": "offline"
            })
        );
    }

    #[test]
    fn now_secs_is_a_unix_timestamp() {
        let now = now_secs();
        assert!(now > 1_700_000_000, "{now}");
        assert!(now_secs() >= now);
    }


    fn temp_store(tag: &str) -> (MetricsStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "mnstr-store-{tag}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (MetricsStore::open(&dir), dir)
    }

    fn alert(id: i64, kind: &str, ts: i64) -> Alert {
        Alert {
            server_id: id,
            server_name: format!("srv{id}"),
            kind: kind.into(),
            severity: "critical".into(),
            message: "test".into(),
            ts,
            details: None,
        }
    }

    #[test]
    fn alertes_curseur_et_fenetre() {
        let (store, dir) = temp_store("alerts");
        assert_eq!(store.alerts(None, None, 10).unwrap().1, 0);
        let a = store.insert_alert(&alert(1, "crash", 100)).unwrap();
        let b = store.insert_alert(&alert(2, "cpu", 200)).unwrap();
        let c = store.insert_alert(&alert(3, "ram", 300)).unwrap();
        assert!(a < b && b < c);

        // Curseur : seulement les alertes postérieures, dans l'ordre.
        let (after, last) = store.alerts(Some(a), None, 10).unwrap();
        assert_eq!(last, c);
        assert_eq!(after.iter().map(|s| s.id).collect::<Vec<_>>(), vec![b, c]);
        assert_eq!(after[0].alert.kind, "cpu");

        // Fenêtre temporelle + limite.
        let (since, _) = store.alerts(None, Some(250), 10).unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[0].alert.server_id, 3);
        let (limited, _) = store.alerts(None, None, 2).unwrap();
        assert_eq!(limited.len(), 2);

        // `limit = 0` sert à ne lire que le curseur.
        let (none, last) = store.alerts(None, None, 0).unwrap();
        assert!(none.is_empty());
        assert_eq!(last, c);

        // La rétention purge aussi les alertes.
        store.prune(250).unwrap();
        let (left, _) = store.alerts(None, None, 10).unwrap();
        assert_eq!(left.len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn alerte_stockee_serialisee_a_plat() {
        let s = StoredAlert { id: 7, alert: alert(42, "crash", 10) };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["server_id"], 42);
        assert_eq!(v["kind"], "crash");
        // Sans détail : le champ est absent (forme identique aux versions antérieures).
        assert!(v.get("details").is_none());
        let back: StoredAlert = serde_json::from_value(v).unwrap();
        assert_eq!(back.alert.server_name, "srv42");
        assert_eq!(back.alert.details, None);
    }

    #[test]
    fn alerte_avec_detail_conservee() {
        let dir = TempDir::new();
        let store = MetricsStore::open(&dir.0);
        let mut a = alert(1, "crash", 100);
        a.details = Some("[ERROR] java.lang.OutOfMemoryError".into());
        store.insert_alert(&a).unwrap();
        let (items, _) = store.alerts(None, None, 10).unwrap();
        assert_eq!(items[0].alert.details.as_deref(), Some("[ERROR] java.lang.OutOfMemoryError"));
    }

    #[test]
    fn migration_ajoute_la_colonne_details() {
        let dir = TempDir::new();
        // Base créée par une version antérieure : table `alerts` sans `details`.
        {
            let conn = Connection::open(dir.0.join("metrics.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE alerts (
                    id INTEGER PRIMARY KEY AUTOINCREMENT, server_id INTEGER NOT NULL,
                    server_name TEXT NOT NULL, kind TEXT NOT NULL, severity TEXT NOT NULL,
                    message TEXT NOT NULL, ts INTEGER NOT NULL);
                 INSERT INTO alerts (server_id, server_name, kind, severity, message, ts)
                 VALUES (1, 'ancien', 'crash', 'critical', 'avant migration', 5);",
            )
            .unwrap();
        }
        let store = MetricsStore::open(&dir.0);
        let (items, _) = store.alerts(None, None, 10).unwrap();
        assert_eq!(items.len(), 1, "l'ancienne alerte reste lisible");
        assert_eq!(items[0].alert.details, None);
        let mut a = alert(2, "crash", 6);
        a.details = Some("logs".into());
        store.insert_alert(&a).unwrap();
        assert_eq!(store.alerts(None, None, 10).unwrap().0[1].alert.details.as_deref(), Some("logs"));
        drop(store);
        // Réouverture : la migration ne se rejoue pas (et n'échoue pas).
        let store = MetricsStore::open(&dir.0);
        assert_eq!(store.alerts(None, None, 10).unwrap().0.len(), 2);
    }

    #[test]
    fn observations_detat_par_serveur_et_retention_longue() {
        let dir = TempDir::new();
        let store = MetricsStore::open(&dir.0);
        store.insert_state(1, 100, "running", None).unwrap();
        store.insert_state(1, 160, "offline", Some("crash")).unwrap();
        store.insert_state(2, 120, "running", None).unwrap();
        let rows = store.state_rows(1, 0).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], StateRow { ts: 160, state: "offline".into(), cause: Some("crash".into()) });
        assert_eq!(store.state_rows(1, 150).unwrap().len(), 1);
        // La purge des métriques ne touche pas les observations (rétention plus longue).
        store.prune(1_000).unwrap();
        assert_eq!(store.state_rows(1, 0).unwrap().len(), 2);
        store.prune_long(150).unwrap();
        assert_eq!(store.state_rows(1, 0).unwrap().len(), 1);
        assert!(store.state_rows(2, 0).unwrap().is_empty());
    }

    fn run(account_id: i64, server_id: i64, task: Option<&str>, kind: &str, started: i64) -> AutomationRun {
        AutomationRun {
            id: 0,
            account_id,
            server_id,
            task_id: task.map(str::to_string),
            task_name: task.map(|t| format!("tâche {t}")),
            kind: kind.into(),
            scheduled_ts: task.map(|_| started),
            started_ts: started,
            finished_ts: Some(started + 5),
            status: "ok".into(),
            message: "fait".into(),
            details: None,
        }
    }

    #[test]
    fn journal_des_automatisations() {
        let dir = TempDir::new();
        let store = MetricsStore::open(&dir.0);
        store.insert_run(&run(7, 1, Some("t1"), "restart", 100)).unwrap();
        store.insert_run(&run(7, 1, Some("t1"), "restart", 200)).unwrap();
        store.insert_run(&run(7, 2, None, "auto_restart", 150)).unwrap();
        store.insert_run(&run(8, 1, Some("t9"), "snapshot", 300)).unwrap();

        // Filtré par compte, puis par serveur ; du plus récent au plus ancien.
        let all = store.runs(7, None, 10).unwrap();
        assert_eq!(all.iter().map(|r| r.started_ts).collect::<Vec<_>>(), vec![200, 150, 100]);
        let srv1 = store.runs(7, Some(1), 10).unwrap();
        assert_eq!(srv1.len(), 2);
        assert_eq!(srv1[0].task_name.as_deref(), Some("tâche t1"));
        assert_eq!(store.runs(7, None, 1).unwrap().len(), 1);
        assert_eq!(store.runs(8, None, 10).unwrap().len(), 1, "un autre compte ne voit rien");

        assert_eq!(store.last_scheduled("t1").unwrap(), Some(200));
        assert_eq!(store.last_scheduled("absente").unwrap(), None);

        assert_eq!(store.auto_restarts_since(2, 0).unwrap(), vec![150]);
        assert!(store.auto_restarts_since(2, 151).unwrap().is_empty());
        let mut failed = run(7, 2, None, "auto_restart", 160);
        failed.status = "failed".into();
        store.insert_run(&failed).unwrap();
        assert_eq!(store.auto_restarts_since(2, 0).unwrap(), vec![150], "seuls les lancés comptent");

        store.prune_long(160).unwrap();
        assert_eq!(store.runs(7, None, 10).unwrap().len(), 2);
    }
}
