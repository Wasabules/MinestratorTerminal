//! Superviseur : monitoring de fond, historique et détection d'alertes (crash, seuils,
//! expiration). Configurable via `SupervisorConfig` (persistée dans le dossier data).
//!
//! Il peut être **mis en veille** à chaud (`SupervisorState::set_suspended`) : l'app le fait
//! quand le service d'arrière-plan supervise déjà le même compte (voir [`crate::service`]),
//! pour éviter une double collecte et des alertes en double.

use crate::api::ApiClient;
use crate::console::ConsoleManager;
use crate::events::{Alert, CoreEvent};
use crate::secrets;
use crate::store::{now_secs, MetricsStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, Notify};

const SAMPLE_EVERY_S: i64 = 10;
const RETENTION_S: i64 = 14 * 24 * 3600;
const POLL_EVERY_S: u64 = 60;
const CRIT_THRESHOLD: f64 = 98.0;
const THRESHOLD_COOLDOWN_S: i64 = 600;
const CRASH_COOLDOWN_S: i64 = 120;
const EXPIRED_SOON_DAYS: i64 = 3;
const EXPIRY_COOLDOWN_S: i64 = 12 * 3600;
const EXPECTED_STOP_WINDOW_S: i64 = 60;

/// Réglages du superviseur (modifiables depuis l'UI, persistés).
///
/// `#[serde(default)]` : un champ absent du `supervisor.json` (ajouté par une version ultérieure)
/// prend sa valeur par défaut, au lieu de rendre tout le fichier illisible — ce qui ramenait
/// silencieusement TOUS les réglages de l'utilisateur aux valeurs d'usine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SupervisorConfig {
    pub enabled: bool,
    pub crash_detection: bool,
    pub expiry_alerts: bool,
    pub cpu_threshold: f64,
    pub ram_threshold: f64,
    pub disk_threshold: f64,
    /// Serveurs exclus du monitoring.
    pub disabled_servers: Vec<i64>,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            crash_detection: true,
            expiry_alerts: true,
            cpu_threshold: 90.0,
            ram_threshold: 90.0,
            disk_threshold: 90.0,
            disabled_servers: Vec::new(),
        }
    }
}

/// État partagé du superviseur (accédé par ses tâches et par `Core`).
pub struct SupervisorState {
    names: Mutex<HashMap<i64, String>>,
    states: Mutex<HashMap<i64, String>>,
    limits: Mutex<HashMap<i64, (i64, i64)>>,
    expected_stops: Mutex<HashMap<i64, i64>>,
    cooldowns: Mutex<HashMap<(i64, String), i64>>,
    config: crate::persist::PersistedConfig<SupervisorConfig>,
    /// Veille à chaud (non persistée) : le service d'arrière-plan supervise à la place de l'app.
    suspended: AtomicBool,
    /// Dernier tour de poll réussi (secondes Unix, 0 = jamais) : battement de cœur du service.
    last_poll: AtomicI64,
    /// Nombre de serveurs actuellement sous connexion monitor.
    monitored: AtomicUsize,
    /// Réveille le poller sans attendre la fin de sa pause (veille, nouvelle clé…).
    wake: Notify,
}

/// État observable du superviseur (exposé par le service d'arrière-plan).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SupervisorHeartbeat {
    /// Dernier tour de poll réussi (`None` = aucun depuis le démarrage).
    pub last_poll: Option<i64>,
    /// Serveurs sous connexion monitor.
    pub monitored: usize,
    pub suspended: bool,
}

impl SupervisorState {
    pub fn load(dir: &Path) -> Self {
        Self {
            names: Mutex::new(HashMap::new()),
            states: Mutex::new(HashMap::new()),
            limits: Mutex::new(HashMap::new()),
            expected_stops: Mutex::new(HashMap::new()),
            cooldowns: Mutex::new(HashMap::new()),
            config: crate::persist::PersistedConfig::load(dir, "supervisor.json"),
            suspended: AtomicBool::new(false),
            last_poll: AtomicI64::new(0),
            monitored: AtomicUsize::new(0),
            wake: Notify::new(),
        }
    }

    pub fn config(&self) -> SupervisorConfig {
        self.config.get()
    }

    pub fn set_config(&self, cfg: SupervisorConfig) {
        self.config.set(cfg);
        self.wake.notify_one();
    }

    /// Met le superviseur en veille (ferme ses connexions monitor) ou l'en sort. Sans effet si
    /// l'état ne change pas.
    pub fn set_suspended(&self, suspended: bool) {
        if self.suspended.swap(suspended, Ordering::SeqCst) != suspended {
            tracing::info!(suspended, "superviseur local : changement de veille");
            self.wake.notify_one();
        }
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended.load(Ordering::SeqCst)
    }

    /// Réveille le poller (ex. après l'enregistrement d'une nouvelle clé).
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    pub fn heartbeat(&self) -> SupervisorHeartbeat {
        let t = self.last_poll.load(Ordering::SeqCst);
        SupervisorHeartbeat {
            last_poll: (t > 0).then_some(t),
            monitored: self.monitored.load(Ordering::SeqCst),
            suspended: self.is_suspended(),
        }
    }

    pub fn mark_expected_stop(&self, id: i64) {
        self.expected_stops.lock().unwrap().insert(id, now_secs());
    }

    fn name(&self, id: i64) -> String {
        self.names
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("#{id}"))
    }

    /// Nom connu d'un serveur (ou `#id`). Exposé pour le Copilote.
    pub fn name_of(&self, id: i64) -> String {
        self.name(id)
    }

    /// Limites connues `(cpu_limit_centièmes, disk_mb)` d'un serveur (récupérées par le poller).
    pub fn limits_of(&self, id: i64) -> Option<(i64, i64)> {
        self.limits.lock().unwrap().get(&id).copied()
    }

    fn cooled(&self, id: i64, kind: &str, cooldown_s: i64) -> bool {
        let mut cd = self.cooldowns.lock().unwrap();
        let now = now_secs();
        let key = (id, kind.to_string());
        if cd.get(&key).is_none_or(|t| now - t >= cooldown_s) {
            cd.insert(key, now);
            true
        } else {
            false
        }
    }
}

pub struct Supervisor {
    api: ApiClient,
    console: Arc<ConsoleManager>,
    store: Arc<MetricsStore>,
    events: broadcast::Sender<CoreEvent>,
    state: Arc<SupervisorState>,
}

impl Supervisor {
    pub fn new(
        api: ApiClient,
        console: Arc<ConsoleManager>,
        store: Arc<MetricsStore>,
        events: broadcast::Sender<CoreEvent>,
        state: Arc<SupervisorState>,
    ) -> Self {
        Self {
            api,
            console,
            store,
            events,
            state,
        }
    }

    pub fn start(&self) {
        self.spawn_recorder();
        self.spawn_poller();
    }

    fn spawn_recorder(&self) {
        let store = self.store.clone();
        let events = self.events.clone();
        let state = self.state.clone();
        let mut rx = self.events.subscribe();
        tokio::spawn(async move {
            let mut last: HashMap<i64, i64> = HashMap::new();
            loop {
                match rx.recv().await {
                    Ok(CoreEvent::ConsoleStats(s)) => {
                        let Some(id) = monitor_id(&s.conn_id) else {
                            continue;
                        };
                        let now = now_secs();
                        if last.get(&id).is_none_or(|t| now - t >= SAMPLE_EVERY_S) {
                            last.insert(id, now);
                            let _ = store.insert(
                                id,
                                now,
                                s.cpu_absolute,
                                s.memory_bytes as i64,
                                s.memory_limit_bytes as i64,
                                s.disk_bytes as i64,
                                &s.state,
                            );
                            check_thresholds(&events, &state, id, &s);
                        }
                    }
                    Ok(CoreEvent::ConsoleStatus(s)) => {
                        if let Some(id) = monitor_id(&s.conn_id) {
                            detect_crash(&events, &state, id, &s.state);
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn spawn_poller(&self) {
        let api = self.api.clone();
        let console = self.console.clone();
        let store = self.store.clone();
        let events = self.events.clone();
        let state = self.state.clone();
        tokio::spawn(async move {
            let mut opened: HashSet<i64> = HashSet::new();
            // Id utilisateur résolu une fois PAR CLÉ (stable) → pas de GET /user à chaque tour de
            // poll ; une nouvelle clé (éventuellement d'un autre compte) force une résolution.
            let mut user: Option<(String, i64)> = None;
            loop {
                let cfg = state.config();
                // En veille, tout se passe comme sans clé : les connexions monitor sont fermées.
                let key = if state.is_suspended() {
                    Ok(None)
                } else {
                    secrets::read_key()
                };
                if let Ok(None) = key {
                    for id in opened.drain() {
                        console.disconnect(&monitor_conn_id(id));
                    }
                    user = None;
                }
                if let Ok(Some(token)) = key {
                    if user.as_ref().is_none_or(|(t, _)| *t != token) {
                        // Clé changée : les connexions monitor ouvertes portent l'ancienne.
                        for id in opened.drain() {
                            console.disconnect(&monitor_conn_id(id));
                        }
                        user = api.get_user(&token).await.ok().map(|u| (token.clone(), u.id));
                    }
                    let user_id = user.as_ref().map(|(_, id)| *id);
                    let overview = match user_id {
                        Some(uid) => api.list_servers(&token, uid).await,
                        None => Err(crate::Error::NoKey), // id pas encore résolu → on ignore ce tour
                    };
                    if let Ok(overview) = overview {
                        state.last_poll.store(now_secs(), Ordering::SeqCst);
                        {
                            let mut names = state.names.lock().unwrap();
                            for s in &overview.servers {
                                names.insert(s.id, s.name.clone());
                            }
                        }

                        if cfg.enabled && cfg.expiry_alerts {
                            for mb in &overview.myboxes {
                                // Détail (un GET) seulement si une alerte est en jeu.
                                let details = if at_risk(mb.days_left, mb.expired) {
                                    api.get_mybox(&token, mb.id).await.ok()
                                } else {
                                    None
                                };
                                let deadline = MyBoxDeadline {
                                    days_left: mb.days_left,
                                    expired: mb.expired,
                                    auto_renew: details.as_ref().is_some_and(|d| d.mybox.auto_renew),
                                    deletion_days: details.and_then(|d| d.mybox.deletion_days),
                                };
                                check_expiry(&events, &state, mb.id, &mb.name, deadline);
                            }
                        }

                        let desired = desired_servers(
                            &cfg,
                            overview.servers.iter().map(|s| (s.id, s.status.as_str())),
                        );

                        for id in desired.difference(&opened).copied().collect::<Vec<_>>() {
                            console.connect(api.clone(), token.clone(), monitor_conn_id(id), id, true);
                            opened.insert(id);
                        }
                        for id in opened.difference(&desired).copied().collect::<Vec<_>>() {
                            console.disconnect(&monitor_conn_id(id));
                            opened.remove(&id);
                        }
                        for id in &desired {
                            if let Ok(ll) = api.get_live_light(&token, *id).await {
                                state
                                    .limits
                                    .lock()
                                    .unwrap()
                                    .insert(*id, (ll.cpu.limit, ll.disk.limit));
                            }
                        }
                    }
                    let _ = store.prune(now_secs() - RETENTION_S);
                }
                state.monitored.store(opened.len(), Ordering::SeqCst);
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(POLL_EVERY_S)) => {}
                    _ = state.wake.notified() => {}
                }
            }
        });
    }
}

/// Serveurs à surveiller (`(id, statut)` en entrée) : actifs et non exclus, aucun si le
/// superviseur est désactivé.
fn desired_servers<'a>(
    cfg: &SupervisorConfig,
    servers: impl IntoIterator<Item = (i64, &'a str)>,
) -> HashSet<i64> {
    if !cfg.enabled {
        return HashSet::new();
    }
    servers
        .into_iter()
        .filter(|(id, status)| *status == "active" && !cfg.disabled_servers.contains(id))
        .map(|(id, _)| id)
        .collect()
}

/// Échéance d'une MyBox. `auto_renew` et `deletion_days` viennent de `GET /mybox/{id}`, demandé
/// seulement quand une alerte est en jeu (voir [`at_risk`]).
#[derive(Debug, Clone, Copy, Default)]
struct MyBoxDeadline {
    days_left: i64,
    expired: bool,
    auto_renew: bool,
    deletion_days: Option<i64>,
}

/// Une MyBox mérite-t-elle le GET de détail (échéance proche ou dépassée) ?
fn at_risk(days_left: i64, expired: bool) -> bool {
    expired || days_left <= EXPIRED_SOON_DAYS
}

/// Alertes d'échéance d'une MyBox, au plus une fois par `EXPIRY_COOLDOWN_S` chacune :
/// - **expiration proche** (≤ `EXPIRED_SOON_DAYS` jours), sauf renouvellement automatique actif —
///   l'échéance n'est alors pas une menace ;
/// - **suppression programmée** d'une MyBox expirée (critique : les données vont disparaître).
fn check_expiry(
    events: &broadcast::Sender<CoreEvent>,
    state: &SupervisorState,
    id: i64,
    name: &str,
    d: MyBoxDeadline,
) {
    if d.expired {
        if let Some(days) = d.deletion_days {
            if state.cooled(id, "deletion", EXPIRY_COOLDOWN_S) {
                alert(
                    events,
                    id,
                    name,
                    "expiry",
                    "critical",
                    format!(
                        "La MyBox « {name} » a expiré : elle sera supprimée dans {days} jour(s)."
                    ),
                );
            }
        }
        return;
    }
    if d.days_left <= EXPIRED_SOON_DAYS
        && !d.auto_renew
        && state.cooled(id, "expiry", EXPIRY_COOLDOWN_S)
    {
        let days_left = d.days_left;
        alert(
            events,
            id,
            name,
            "expiry",
            "warning",
            format!("La MyBox « {name} » expire dans {days_left} jour(s)."),
        );
    }
}

fn detect_crash(
    events: &broadcast::Sender<CoreEvent>,
    state: &SupervisorState,
    id: i64,
    new_state: &str,
) {
    let prev = state.states.lock().unwrap().insert(id, new_state.to_string());
    let Some(prev) = prev else { return };
    let cfg = state.config();
    if !cfg.enabled || !cfg.crash_detection {
        return;
    }
    // Wings ne passe par `stopping` que pour un arrêt DEMANDÉ (power action : panel web, API,
    // app) ; un plantage va directement de `running`/`starting` à `offline`.
    if prev != "offline" && prev != "stopping" && new_state == "offline" {
        let expected = state
            .expected_stops
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|t| now_secs() - t < EXPECTED_STOP_WINDOW_S);
        if !expected && state.cooled(id, "crash", CRASH_COOLDOWN_S) {
            let name = state.name(id);
            alert(
                events,
                id,
                &name,
                "crash",
                "critical",
                format!("« {name} » s'est arrêté de façon inattendue."),
            );
        }
    }
}

fn check_thresholds(
    events: &broadcast::Sender<CoreEvent>,
    state: &SupervisorState,
    id: i64,
    s: &crate::events::ConsoleStats,
) {
    let cfg = state.config();
    if !cfg.enabled {
        return;
    }
    let (cpu_limit, disk_mb) = state.limits.lock().unwrap().get(&id).copied().unwrap_or((0, 0));
    let ram = pct(s.memory_bytes as f64, s.memory_limit_bytes as f64);
    let cpu = if cpu_limit > 0 {
        s.cpu_absolute / cpu_limit as f64 * 100.0
    } else {
        0.0
    };
    let disk = pct(s.disk_bytes as f64, disk_mb as f64 * 1024.0 * 1024.0);

    let checks = [
        ("cpu", "CPU", cpu, cfg.cpu_threshold),
        ("ram", "RAM", ram, cfg.ram_threshold),
        ("disk", "Disque", disk, cfg.disk_threshold),
    ];
    for (kind, label, value, threshold) in checks {
        if threshold > 0.0 && value >= threshold && state.cooled(id, kind, THRESHOLD_COOLDOWN_S) {
            let severity = if value >= CRIT_THRESHOLD { "critical" } else { "warning" };
            let name = state.name(id);
            alert(events, id, &name, kind, severity, format!("« {name} » — {label} à {value:.0} %."));
        }
    }
}

fn pct(v: f64, total: f64) -> f64 {
    if total > 0.0 {
        v / total * 100.0
    } else {
        0.0
    }
}

fn alert(
    events: &broadcast::Sender<CoreEvent>,
    id: i64,
    name: &str,
    kind: &str,
    severity: &str,
    message: String,
) {
    let _ = events.send(CoreEvent::Alert(Alert {
        server_id: id,
        server_name: name.to_string(),
        kind: kind.to_string(),
        severity: severity.to_string(),
        message,
        ts: now_secs(),
    }));
}

fn monitor_conn_id(server_id: i64) -> String {
    format!("mon:{server_id}")
}

fn monitor_id(conn_id: &str) -> Option<i64> {
    conn_id.strip_prefix("mon:").and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::ConsoleStats;
    use std::path::PathBuf;

    #[test]
    fn config_missing_fields_keep_user_values() {
        // Fichier écrit par une version antérieure, sans les champs ajoutés depuis.
        let cfg: SupervisorConfig =
            serde_json::from_str(r#"{ "enabled": false, "cpu_threshold": 75.0 }"#).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.cpu_threshold, 75.0);
        assert!(cfg.crash_detection, "champ absent → valeur par défaut");
        assert_eq!(cfg.ram_threshold, 90.0);
    }
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Dossier temporaire unique, supprimé à la fin du test.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicUsize = AtomicUsize::new(0);
            let n = SEQ.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("mst-test-supervisor-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// État de superviseur à la config par défaut + bus d'events dont on garde le récepteur.
    struct Fixture {
        _dir: TempDir,
        state: SupervisorState,
        tx: broadcast::Sender<CoreEvent>,
        rx: broadcast::Receiver<CoreEvent>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = TempDir::new();
            let state = SupervisorState::load(&dir.0);
            let (tx, rx) = broadcast::channel(64);
            Self {
                _dir: dir,
                state,
                tx,
                rx,
            }
        }

        fn with_config(f: impl FnOnce(&mut SupervisorConfig)) -> Self {
            let fx = Self::new();
            let mut cfg = fx.state.config();
            f(&mut cfg);
            fx.state.set_config(cfg);
            fx
        }

        fn status(&self, id: i64, state: &str) {
            detect_crash(&self.tx, &self.state, id, state);
        }

        fn stats(&self, id: i64, s: &ConsoleStats) {
            check_thresholds(&self.tx, &self.state, id, s);
        }

        /// Alertes émises depuis le dernier appel.
        fn alerts(&mut self) -> Vec<Alert> {
            let mut out = Vec::new();
            while let Ok(ev) = self.rx.try_recv() {
                if let CoreEvent::Alert(a) = ev {
                    out.push(a);
                }
            }
            out
        }
    }

    const GIB: u64 = 1024 * 1024 * 1024;

    fn stats(cpu_absolute: f64, memory_bytes: u64, memory_limit_bytes: u64, disk_bytes: u64) -> ConsoleStats {
        ConsoleStats {
            conn_id: monitor_conn_id(1),
            cpu_absolute,
            memory_bytes,
            memory_limit_bytes,
            disk_bytes,
            uptime: 0,
            state: "running".into(),
        }
    }

    // --- Identifiants de connexion monitor / config -------------------------------

    #[test]
    fn monitor_conn_ids_round_trip() {
        assert_eq!(monitor_conn_id(42), "mon:42");
        assert_eq!(monitor_id(&monitor_conn_id(42)), Some(42));
        // Les connexions des onglets (autres préfixes) ne sont jamais prises pour du monitoring.
        assert_eq!(monitor_id("tab-3:42"), None);
        assert_eq!(monitor_id("mon:"), None);
        assert_eq!(monitor_id("mon:abc"), None);
        assert_eq!(monitor_id("42"), None);
    }

    #[test]
    fn default_config_and_persistence() {
        let cfg = SupervisorConfig::default();
        assert!(cfg.enabled && cfg.crash_detection && cfg.expiry_alerts);
        assert_eq!((cfg.cpu_threshold, cfg.ram_threshold, cfg.disk_threshold), (90.0, 90.0, 90.0));
        assert!(cfg.disabled_servers.is_empty());

        let dir = TempDir::new();
        let state = SupervisorState::load(&dir.0);
        state.set_config(SupervisorConfig {
            cpu_threshold: 75.0,
            disabled_servers: vec![7],
            ..SupervisorConfig::default()
        });
        // Relu depuis `supervisor.json` par une nouvelle instance.
        let reloaded = SupervisorState::load(&dir.0).config();
        assert_eq!(reloaded.cpu_threshold, 75.0);
        assert_eq!(reloaded.disabled_servers, vec![7]);

        // Fichier corrompu → défauts, sans panique.
        std::fs::write(dir.0.join("supervisor.json"), "{ pas du json").unwrap();
        assert_eq!(SupervisorState::load(&dir.0).config().cpu_threshold, 90.0);
    }

    #[test]
    fn unknown_server_name_falls_back_to_its_id() {
        let fx = Fixture::new();
        assert_eq!(fx.state.name_of(12), "#12");
        fx.state.names.lock().unwrap().insert(12, "Survie".into());
        assert_eq!(fx.state.name_of(12), "Survie");
        assert_eq!(fx.state.limits_of(12), None);
    }

    // --- Cooldowns -----------------------------------------------------------------

    #[test]
    fn cooldown_is_per_server_and_per_kind() {
        let fx = Fixture::new();
        assert!(fx.state.cooled(1, "cpu", 600));
        assert!(!fx.state.cooled(1, "cpu", 600), "2e alerte dans la fenêtre");
        assert!(fx.state.cooled(1, "ram", 600), "autre type : indépendant");
        assert!(fx.state.cooled(2, "cpu", 600), "autre serveur : indépendant");
        // Cooldown nul → jamais bloquant.
        assert!(fx.state.cooled(3, "x", 0));
        assert!(fx.state.cooled(3, "x", 0));
    }

    #[test]
    fn cooldown_expires_after_its_window() {
        let fx = Fixture::new();
        fx.state
            .cooldowns
            .lock()
            .unwrap()
            .insert((1, "cpu".into()), now_secs() - 601);
        assert!(fx.state.cooled(1, "cpu", 600));
        assert!(!fx.state.cooled(1, "cpu", 600), "la fenêtre repart du dernier envoi");
    }

    // --- Détection de crash ----------------------------------------------------------

    #[test]
    fn first_status_is_only_recorded() {
        let mut fx = Fixture::new();
        // Premier état connu = offline (app lancée sur un serveur éteint) : pas de crash.
        fx.status(1, "offline");
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn unexpected_offline_is_a_crash() {
        let mut fx = Fixture::new();
        fx.state.names.lock().unwrap().insert(1, "Survie".into());
        fx.status(1, "running");
        fx.status(1, "offline");
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!(a.server_id, 1);
        assert_eq!(a.server_name, "Survie");
        assert_eq!(a.kind, "crash");
        assert_eq!(a.severity, "critical");
        assert_eq!(a.message, "« Survie » s'est arrêté de façon inattendue.");
        assert!(a.ts > 0);

        // Un échec au démarrage (starting → offline) est aussi un crash.
        fx.status(2, "starting");
        fx.status(2, "offline");
        assert_eq!(fx.alerts().len(), 1);
    }

    #[test]
    fn transitions_that_are_not_crashes() {
        let mut fx = Fixture::new();
        fx.status(1, "offline");
        fx.status(1, "offline");
        fx.status(1, "starting");
        fx.status(1, "running");
        fx.status(1, "stopping");
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn expected_stop_is_not_a_crash() {
        let mut fx = Fixture::new();
        fx.status(1, "running");
        fx.state.mark_expected_stop(1);
        fx.status(1, "offline");
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn expected_stop_mark_expires() {
        let mut fx = Fixture::new();
        fx.status(1, "running");
        // Arrêt demandé il y a plus de EXPECTED_STOP_WINDOW_S : ne couvre plus cette coupure.
        fx.state
            .expected_stops
            .lock()
            .unwrap()
            .insert(1, now_secs() - EXPECTED_STOP_WINDOW_S - 1);
        fx.status(1, "offline");
        assert_eq!(fx.alerts().len(), 1);
    }

    #[test]
    fn crash_alerts_are_rate_limited() {
        let mut fx = Fixture::new();
        fx.status(1, "running");
        fx.status(1, "offline");
        fx.status(1, "running");
        fx.status(1, "offline");
        assert_eq!(fx.alerts().len(), 1, "boucle de crash : une alerte par CRASH_COOLDOWN_S");
    }

    #[test]
    fn crash_detection_can_be_disabled() {
        for f in [
            (|c: &mut SupervisorConfig| c.enabled = false) as fn(&mut SupervisorConfig),
            |c: &mut SupervisorConfig| c.crash_detection = false,
        ] {
            let mut fx = Fixture::with_config(f);
            fx.status(1, "running");
            fx.status(1, "offline");
            assert!(fx.alerts().is_empty());
        }
    }

    /// Arrêt demandé hors de l'app (panel web, API) : Wings passe par `stopping`. Ce n'est pas un
    /// plantage, même sans `mark_expected_stop`.
    #[test]
    fn graceful_stop_through_stopping_state_is_not_a_crash() {
        let mut fx = Fixture::new();
        fx.status(1, "running");
        fx.status(1, "stopping");
        fx.status(1, "offline");
        assert!(fx.alerts().is_empty());
    }

    // --- Seuils CPU / RAM / disque ----------------------------------------------------

    #[test]
    fn ram_over_threshold_raises_a_warning() {
        let mut fx = Fixture::new();
        fx.state.names.lock().unwrap().insert(1, "Survie".into());
        fx.stats(1, &stats(0.0, 95 * GIB / 100, GIB, 0));
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "ram");
        assert_eq!(alerts[0].severity, "warning");
        assert_eq!(alerts[0].message, "« Survie » — RAM à 95 %.");
    }

    #[test]
    fn critical_from_98_percent() {
        let mut fx = Fixture::new();
        fx.stats(1, &stats(0.0, GIB, GIB, 0));
        let alerts = fx.alerts();
        assert_eq!(alerts[0].severity, "critical");
        assert_eq!(alerts[0].message, "« #1 » — RAM à 100 %.");
    }

    #[test]
    fn below_threshold_is_silent() {
        let mut fx = Fixture::new();
        fx.state.limits.lock().unwrap().insert(1, (200, 10_000));
        fx.stats(1, &stats(100.0, GIB / 2, GIB, GIB));
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn cpu_is_relative_to_the_server_limit() {
        let mut fx = Fixture::new();
        // Limite de 200 (= 2 cœurs) : 190 en absolu = 95 % de la limite.
        fx.state.limits.lock().unwrap().insert(1, (200, 0));
        fx.stats(1, &stats(190.0, 0, 0, 0));
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "cpu");
        assert_eq!(alerts[0].message, "« #1 » — CPU à 95 %.");
    }

    #[test]
    fn unknown_limits_disable_cpu_and_disk_checks() {
        let mut fx = Fixture::new();
        // Limites pas encore récupérées par le poller : ni CPU ni disque (division par 0 évitée).
        fx.stats(1, &stats(10_000.0, 0, 0, 100 * GIB));
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn disk_is_relative_to_the_quota_in_mb() {
        let mut fx = Fixture::new();
        fx.state.limits.lock().unwrap().insert(1, (0, 1000));
        fx.stats(1, &stats(0.0, 0, 0, 950 * 1024 * 1024));
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].kind, "disk");
        assert_eq!(alerts[0].message, "« #1 » — Disque à 95 %.");
    }

    #[test]
    fn several_metrics_alert_independently_with_their_own_cooldown() {
        let mut fx = Fixture::new();
        fx.state.limits.lock().unwrap().insert(1, (100, 1000));
        let hot = stats(99.0, GIB, GIB, 1000 * 1024 * 1024);
        fx.stats(1, &hot);
        let kinds: Vec<String> = fx.alerts().into_iter().map(|a| a.kind).collect();
        assert_eq!(kinds, ["cpu", "ram", "disk"]);
        // Échantillon suivant, toujours au-dessus : rien avant THRESHOLD_COOLDOWN_S.
        fx.stats(1, &hot);
        assert!(fx.alerts().is_empty());
        // Un autre serveur n'est pas concerné par ce cooldown.
        fx.state.limits.lock().unwrap().insert(2, (100, 1000));
        fx.stats(2, &hot);
        assert_eq!(fx.alerts().len(), 3);
    }

    #[test]
    fn zero_threshold_disables_a_metric() {
        let mut fx = Fixture::with_config(|c| {
            c.ram_threshold = 0.0;
        });
        fx.stats(1, &stats(0.0, GIB, GIB, 0));
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn disabled_supervisor_checks_nothing() {
        let mut fx = Fixture::with_config(|c| c.enabled = false);
        fx.state.limits.lock().unwrap().insert(1, (100, 1000));
        fx.stats(1, &stats(99.0, GIB, GIB, 1000 * 1024 * 1024));
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn pct_guards_against_zero_total() {
        assert_eq!(pct(50.0, 200.0), 25.0);
        assert_eq!(pct(50.0, 0.0), 0.0);
        assert_eq!(pct(50.0, -1.0), 0.0);
    }

    // --- Serveurs surveillés / expiration MyBox ---------------------------------------------

    #[test]
    fn desired_servers_are_active_and_not_excluded() {
        let servers = [(1, "active"), (2, "suspended"), (3, "active"), (4, "expired"), (5, "active")];
        let cfg = SupervisorConfig {
            disabled_servers: vec![3],
            ..SupervisorConfig::default()
        };
        assert_eq!(desired_servers(&cfg, servers), HashSet::from([1, 5]));

        // Superviseur désactivé → plus aucune connexion monitor.
        let off = SupervisorConfig {
            enabled: false,
            ..SupervisorConfig::default()
        };
        assert!(desired_servers(&off, servers).is_empty());
    }

    #[test]
    fn mybox_expiring_soon_is_announced_once() {
        let mut fx = Fixture::new();
        check_expiry(&fx.tx, &fx.state, 10, "Box Pro", soon(2));
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        let a = &alerts[0];
        assert_eq!((a.server_id, a.kind.as_str(), a.severity.as_str()), (10, "expiry", "warning"));
        assert_eq!(a.server_name, "Box Pro");
        assert_eq!(a.message, "La MyBox « Box Pro » expire dans 2 jour(s).");

        // Re-poll une minute plus tard : silence jusqu'à EXPIRY_COOLDOWN_S.
        check_expiry(&fx.tx, &fx.state, 10, "Box Pro", soon(2));
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn mybox_expiry_window() {
        let mut fx = Fixture::new();
        // Limite incluse : EXPIRED_SOON_DAYS jours.
        check_expiry(&fx.tx, &fx.state, 1, "a", soon(EXPIRED_SOON_DAYS));
        check_expiry(&fx.tx, &fx.state, 2, "b", soon(0));
        assert_eq!(fx.alerts().len(), 2);
        // Échéance lointaine, ou MyBox déjà expirée sans suppression annoncée : rien.
        check_expiry(&fx.tx, &fx.state, 3, "c", soon(EXPIRED_SOON_DAYS + 1));
        check_expiry(&fx.tx, &fx.state, 4, "d", MyBoxDeadline { expired: true, ..soon(0) });
        assert!(fx.alerts().is_empty());
        assert!(at_risk(EXPIRED_SOON_DAYS, false) && at_risk(30, true));
        assert!(!at_risk(EXPIRED_SOON_DAYS + 1, false));
    }

    fn soon(days_left: i64) -> MyBoxDeadline {
        MyBoxDeadline { days_left, ..Default::default() }
    }

    #[test]
    fn auto_renewed_mybox_is_not_a_threat() {
        let mut fx = Fixture::new();
        check_expiry(&fx.tx, &fx.state, 1, "a", MyBoxDeadline { auto_renew: true, ..soon(1) });
        assert!(fx.alerts().is_empty());
    }

    #[test]
    fn expired_mybox_announces_its_deletion() {
        let mut fx = Fixture::new();
        let expired = MyBoxDeadline { expired: true, deletion_days: Some(5), ..soon(0) };
        check_expiry(&fx.tx, &fx.state, 7, "Ancienne", expired);
        let alerts = fx.alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, "critical");
        assert_eq!(
            alerts[0].message,
            "La MyBox « Ancienne » a expiré : elle sera supprimée dans 5 jour(s)."
        );
        // Cooldown propre : ne se répète pas au poll suivant.
        check_expiry(&fx.tx, &fx.state, 7, "Ancienne", expired);
        assert!(fx.alerts().is_empty());
    }
}
