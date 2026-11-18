//! Journal des alertes du superviseur, lu par le centre d'alertes de l'app mobile.
//!
//! Le superviseur du core émet ses alertes (crash, seuils CPU/RAM/disque, échéance MyBox) sur le
//! bus d'events sans les conserver : le desktop les accumule dans son webview, qui vit aussi
//! longtemps que l'app. Sur Android, ce n'est pas suffisant. Quand la surveillance en arrière-plan
//! est active, le process survit à l'activité (service au premier plan) mais le webview, lui, peut
//! être détruit : une alerte reçue à ce moment-là n'aurait laissé qu'une notification système. Le
//! pont d'events (`forward`, dans `lib.rs`) les consigne donc ici, dans un petit fichier JSON du
//! dossier privé de l'app, que l'écran Alertes relit à l'ouverture.
//!
//! Volume borné ([`MAX_ITEMS`], [`RETENTION_S`]) : le superviseur espace ses alertes (délais de
//! récidive), si bien que réécrire tout le fichier à chaque ajout reste négligeable. Le fichier ne
//! contient aucun secret : noms de serveurs et messages, déjà montrés dans les notifications.

use minestrator_core::Alert;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;

/// Nombre maximal d'alertes conservées (les plus anciennes sont écartées au-delà).
const MAX_ITEMS: usize = 200;
/// Une alerte plus vieille que 30 jours n'apprend plus rien : elle est oubliée.
const RETENTION_S: i64 = 30 * 24 * 3600;

/// Une alerte consignée, telle que l'écran Alertes l'affiche.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AlertEntry {
    /// Identifiant local strictement croissant (clé de liste côté front).
    pub id: i64,
    /// Déjà vue dans le centre d'alertes (pastille de la liste des serveurs).
    pub read: bool,
    #[serde(flatten)]
    pub alert: Alert,
}

/// Contenu du fichier : les alertes, les plus récentes d'abord.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Journal {
    next_id: i64,
    items: Vec<AlertEntry>,
}

/// Journal partagé entre le pont d'events (écriture) et les commandes (lecture, marquage).
pub struct AlertJournal {
    /// `None` : dossier de données inconnu, journal tenu en mémoire seulement.
    path: Option<PathBuf>,
    inner: Mutex<Journal>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl AlertJournal {
    /// Charge le journal depuis `path`. Fichier absent ou illisible : journal vide (on ne bloque
    /// jamais le démarrage pour un historique).
    pub fn open(path: Option<PathBuf>) -> Self {
        let journal = path
            .as_deref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice::<Journal>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            inner: Mutex::new(journal),
        }
    }

    /// Un verrou empoisonné (panique d'un autre fil) ne doit pas faire perdre le journal.
    fn lock(&self) -> MutexGuard<'_, Journal> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Consigne une nouvelle alerte, non lue.
    pub fn push(&self, alert: Alert) {
        self.push_at(alert, now_secs());
    }

    fn push_at(&self, alert: Alert, now: i64) {
        let mut j = self.lock();
        // Le superviseur n'émet qu'une fois chaque alerte ; la garde couvre un éventuel double
        // relais (même règle que le centre d'alertes desktop : serveur, type, horodatage).
        if j.items.iter().any(|e| {
            e.alert.server_id == alert.server_id && e.alert.kind == alert.kind && e.alert.ts == alert.ts
        }) {
            return;
        }
        j.next_id += 1;
        let id = j.next_id;
        j.items.insert(0, AlertEntry { id, read: false, alert });
        prune(&mut j, now);
        self.save(&j);
    }

    /// Alertes conservées, les plus récentes d'abord.
    pub fn list(&self) -> Vec<AlertEntry> {
        self.list_at(now_secs())
    }

    fn list_at(&self, now: i64) -> Vec<AlertEntry> {
        let mut j = self.lock();
        if prune(&mut j, now) {
            self.save(&j);
        }
        j.items.clone()
    }

    /// Marque tout comme lu (ouverture du centre d'alertes, comme sur le desktop).
    pub fn mark_all_read(&self) {
        let mut j = self.lock();
        if j.items.iter().all(|e| e.read) {
            return;
        }
        for e in &mut j.items {
            e.read = true;
        }
        self.save(&j);
    }

    /// Efface tout l'historique. `next_id` n'est pas remis à zéro : un identifiant ne resert jamais.
    pub fn clear(&self) {
        let mut j = self.lock();
        j.items.clear();
        self.save(&j);
    }

    /// Écriture atomique (fichier temporaire puis renommage) : une coupure en pleine écriture
    /// laisse l'ancien journal intact plutôt qu'un fichier tronqué.
    fn save(&self, j: &Journal) {
        let Some(path) = &self.path else { return };
        if let Err(e) = write_atomic(path, j) {
            tracing::warn!("journal des alertes non enregistré : {e}");
        }
    }
}

fn write_atomic(path: &Path, j: &Journal) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(j).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Applique la rétention (âge puis nombre). Renvoie `true` si quelque chose a été retiré.
fn prune(j: &mut Journal, now: i64) -> bool {
    let before = j.items.len();
    j.items.retain(|e| now - e.alert.ts <= RETENTION_S);
    j.items.truncate(MAX_ITEMS);
    j.items.len() != before
}

// --- Commandes ------------------------------------------------------------------------------

/// Historique des alertes du superviseur, les plus récentes d'abord.
#[tauri::command]
pub fn alerts_list(journal: State<'_, AlertJournal>) -> Vec<AlertEntry> {
    journal.list()
}

/// Marque toutes les alertes comme lues.
#[tauri::command]
pub fn alerts_mark_read(journal: State<'_, AlertJournal>) {
    journal.mark_all_read();
}

/// Efface l'historique des alertes.
#[tauri::command]
pub fn alerts_clear(journal: State<'_, AlertJournal>) {
    journal.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(server_id: i64, kind: &str, ts: i64) -> Alert {
        Alert {
            server_id,
            server_name: format!("srv{server_id}"),
            kind: kind.into(),
            severity: "critical".into(),
            message: "message".into(),
            ts,
        }
    }

    /// Dossier temporaire propre au test, supprimé à la fin.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mnstr-alerts-{tag}-{}-{}",
                std::process::id(),
                now_secs()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn file(&self) -> PathBuf {
            self.0.join("alerts.json")
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn plus_recentes_d_abord_et_non_lues() {
        let j = AlertJournal::open(None);
        j.push_at(alert(1, "crash", 100), 100);
        j.push_at(alert(2, "cpu", 200), 200);
        let list = j.list_at(200);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].alert.kind, "cpu");
        assert!(list[0].id > list[1].id, "identifiants croissants");
        assert!(list.iter().all(|e| !e.read));
    }

    #[test]
    fn doublon_ignore() {
        let j = AlertJournal::open(None);
        j.push_at(alert(1, "crash", 100), 100);
        j.push_at(alert(1, "crash", 100), 100);
        assert_eq!(j.list_at(100).len(), 1);
    }

    #[test]
    fn marquage_puis_effacement() {
        let j = AlertJournal::open(None);
        j.push_at(alert(1, "crash", 100), 100);
        j.mark_all_read();
        assert!(j.list_at(100).iter().all(|e| e.read));
        // Une alerte postérieure arrive non lue.
        j.push_at(alert(1, "ram", 150), 150);
        assert!(!j.list_at(150)[0].read);
        j.clear();
        assert!(j.list_at(150).is_empty());
        // Après effacement, les identifiants continuent de croître.
        j.push_at(alert(1, "disk", 160), 160);
        assert_eq!(j.list_at(160)[0].id, 3);
    }

    #[test]
    fn retention_par_age_et_par_nombre() {
        let j = AlertJournal::open(None);
        j.push_at(alert(1, "crash", 0), 0);
        assert_eq!(j.list_at(RETENTION_S).len(), 1, "pile à la limite : conservée");
        assert!(j.list_at(RETENTION_S + 1).is_empty(), "au-delà : oubliée");

        let now = 1_000_000;
        for i in 0..(MAX_ITEMS as i64 + 5) {
            j.push_at(alert(i, "cpu", now + i), now + i);
        }
        let list = j.list_at(now + MAX_ITEMS as i64 + 5);
        assert_eq!(list.len(), MAX_ITEMS);
        assert_eq!(list[0].alert.server_id, MAX_ITEMS as i64 + 4, "les plus récentes restent");
    }

    #[test]
    fn survit_a_une_reouverture() {
        let dir = TempDir::new("reopen");
        {
            let j = AlertJournal::open(Some(dir.file()));
            j.push_at(alert(7, "expiry", now_secs()), now_secs());
            j.mark_all_read();
        }
        let j = AlertJournal::open(Some(dir.file()));
        let list = j.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].alert.server_name, "srv7");
        assert!(list[0].read);
        // L'identifiant suivant reprend après le dernier attribué.
        j.push(alert(8, "crash", now_secs() + 1));
        assert_eq!(j.list()[0].id, 2);
        assert!(!dir.0.join("alerts.json.tmp").exists(), "pas de fichier temporaire résiduel");
    }

    #[test]
    fn fichier_corrompu_donne_un_journal_vide() {
        let dir = TempDir::new("corrupt");
        std::fs::write(dir.file(), b"{ pas du json").unwrap();
        let j = AlertJournal::open(Some(dir.file()));
        assert!(j.list().is_empty());
        j.push(alert(1, "crash", now_secs()));
        assert_eq!(AlertJournal::open(Some(dir.file())).list().len(), 1, "réécrit proprement");
    }

    #[test]
    fn serialisation_a_plat() {
        let e = AlertEntry {
            id: 3,
            read: false,
            alert: alert(42, "crash", 10),
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["id"], 3);
        assert_eq!(v["server_id"], 42);
        assert_eq!(v["kind"], "crash");
        assert!(v.get("alert").is_none(), "champs de l'alerte à plat");
    }
}
