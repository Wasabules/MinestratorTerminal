//! Superviseur : monitoring de fond, historique et détection d'alertes (crash, seuils,
//! expiration). Configurable via `SupervisorConfig` (persistée dans le dossier data).

use crate::api::ApiClient;
use crate::console::ConsoleManager;
use crate::events::{Alert, CoreEvent};
use crate::secrets;
use crate::store::{now_secs, MetricsStore};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
    }

    pub fn config(&self) -> SupervisorConfig {
        self.config.get()
    }

    pub fn set_config(&self, cfg: SupervisorConfig) {
        self.config.set(cfg);
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
            // Id utilisateur résolu UNE seule fois (stable) → pas de GET /user à chaque tour de poll.
            let mut user_id: Option<i64> = None;
            loop {
                let cfg = state.config();
                if let Ok(Some(token)) = secrets::read_key() {
                    if user_id.is_none() {
                        user_id = api.get_user(&token).await.ok().map(|u| u.id);
                    }
                    let overview = match user_id {
                        Some(uid) => api.list_servers(&token, uid).await,
                        None => Err(crate::Error::NoKey), // id pas encore résolu → on ignore ce tour
                    };
                    if let Ok(overview) = overview {
                        {
                            let mut names = state.names.lock().unwrap();
                            for s in &overview.servers {
                                names.insert(s.id, s.name.clone());
                            }
                        }

                        if cfg.enabled && cfg.expiry_alerts {
                            for mb in &overview.myboxes {
                                if !mb.expired
                                    && mb.days_left <= EXPIRED_SOON_DAYS
                                    && state.cooled(mb.id, "expiry", EXPIRY_COOLDOWN_S)
                                {
                                    alert(
                                        &events,
                                        mb.id,
                                        &mb.name,
                                        "expiry",
                                        "warning",
                                        format!("La MyBox « {} » expire dans {} jour(s).", mb.name, mb.days_left),
                                    );
                                }
                            }
                        }

                        let desired: HashSet<i64> = if cfg.enabled {
                            overview
                                .servers
                                .iter()
                                .filter(|s| s.status == "active" && !cfg.disabled_servers.contains(&s.id))
                                .map(|s| s.id)
                                .collect()
                        } else {
                            HashSet::new()
                        };

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
                tokio::time::sleep(Duration::from_secs(POLL_EVERY_S)).await;
            }
        });
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
    if prev != "offline" && new_state == "offline" {
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
