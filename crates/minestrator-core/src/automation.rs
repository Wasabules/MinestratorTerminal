//! Automatisations locales de supervision, exécutées par le **superviseur actif** — l'app, ou son
//! service d'arrière-plan quand il supervise le même compte, **jamais les deux** :
//!
//! - **redémarrage automatique après crash**, activable par serveur (désactivé par défaut), borné
//!   par un délai croissant entre tentatives ([`RESTART_BACKOFF_S`]) et un maximum de redémarrages
//!   par heure, au-delà duquel une alerte critique est émise ;
//! - **planificateur** : redémarrage programmé (avec annonces aux joueurs quand le jeu sait en
//!   diffuser, voir [`crate::games`]), snapshot, commandes console ; quotidien, certains jours de
//!   la semaine, ou à intervalle ; en heure locale.
//!
//! Ce sont des automatisations **de l'app** : l'ordinateur doit être allumé. Elles n'ont rien à voir
//! avec les tâches planifiées du panel web MineStrator, exécutées par l'hébergeur.
//!
//! **Une seule exécution.** Le planificateur ne tourne que si le superviseur local n'est pas en
//! veille ; dans l'app, un dernier contrôle auprès du service précède chaque action
//! ([`Core::is_active_supervisor`]). Le service reçoit la configuration par IPC
//! (`set_automations`, voir [`crate::service`]). Chaque tâche et chaque règle est **rattachée à un
//! compte** : un processus n'exécute que celles du compte dont il détient la clé.
//!
//! **Pas de rattrapage.** Une occurrence n'est exécutée que dans les [`TOLERANCE_S`] secondes qui
//! suivent son heure ; au-delà (ordinateur en veille, app fermée), elle est consignée comme
//! manquée si le planificateur tournait, ignorée sinon.
//!
//! Le journal des exécutions vit dans `metrics.db` ([`crate::store`]) ; un échec déclenche une
//! alerte (`kind = "task"`), une réussite part seulement vers les webhooks ([`crate::webhook`]).

use crate::error::{Error, Result};
use crate::events::{Alert, CoreEvent};
use crate::persist::PersistedConfig;
use crate::store::now_secs;
use crate::supervisor::CrashEvent;
use crate::webhook::{Notice, WebhookEvent};
use crate::Core;
use chrono::{Datelike, LocalResult, NaiveDateTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, Notify};

/// Redémarrages automatiques par heure, par défaut et au plus.
pub const DEFAULT_MAX_RESTARTS_PER_HOUR: u32 = 3;
pub const MAX_RESTARTS_PER_HOUR: u32 = 10;
/// Délai avant la 1re, 2e, 3e tentative (puis le dernier, répété) : laisse le temps de lire le
/// crash et évite de relancer en boucle un serveur qui plante au démarrage.
pub const RESTART_BACKOFF_S: [i64; 3] = [30, 120, 300];
/// Retard maximal toléré pour exécuter une occurrence planifiée (pas de rattrapage au-delà).
pub const TOLERANCE_S: i64 = 300;
/// Bornes des tâches.
pub const MIN_INTERVAL_MINUTES: u32 = 5;
pub const MAX_INTERVAL_MINUTES: u32 = 30 * 24 * 60;
pub const MAX_WARNING_MINUTES: u32 = 60;
pub const MAX_TASKS_PER_ACCOUNT: usize = 50;
const MAX_WARNINGS: usize = 6;
const MAX_COMMANDS: usize = 20;
const MAX_COMMAND_LEN: usize = 500;
const MAX_NAME_LEN: usize = 80;
const MAX_MESSAGE_LEN: usize = 200;
/// Période du planificateur.
const TICK: Duration = Duration::from_secs(20);
/// Une annonce en retard de moins que ce délai est encore envoyée (tick de 20 s).
const ANNOUNCE_GRACE_S: i64 = 30;
/// Profondeur maximale de recherche des occurrences passées.
const LOOKBACK_S: i64 = 8 * 86_400;
/// Pause entre deux commandes console d'une même tâche.
const COMMAND_GAP: Duration = Duration::from_secs(1);
/// Une alerte « redémarrage automatique abandonné » au plus par heure et par serveur.
const GIVE_UP_COOLDOWN_S: i64 = 3600;

// --- Configuration ------------------------------------------------------------------------

/// Automatisations configurées (persistées dans `automations.json`, transmises au service).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutomationConfig {
    pub restart_rules: Vec<RestartRule>,
    pub tasks: Vec<ScheduledTask>,
}

/// Redémarrage automatique après crash d'un serveur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RestartRule {
    /// Compte MineStrator propriétaire de la règle.
    pub account_id: i64,
    pub server_id: i64,
    pub enabled: bool,
    #[serde(default = "default_max_per_hour")]
    pub max_per_hour: u32,
}

fn default_max_per_hour() -> u32 {
    DEFAULT_MAX_RESTARTS_PER_HOUR
}

impl RestartRule {
    /// Règle par défaut : désactivée.
    pub fn disabled(account_id: i64, server_id: i64) -> Self {
        Self { account_id, server_id, enabled: false, max_per_hour: DEFAULT_MAX_RESTARTS_PER_HOUR }
    }
}

/// Tâche planifiée d'un serveur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ScheduledTask {
    /// Identifiant attribué par le cœur (vide à la création).
    pub id: String,
    /// Compte MineStrator propriétaire (attribué par le cœur).
    pub account_id: i64,
    pub server_id: i64,
    /// Libellé libre (facultatif).
    #[serde(default)]
    pub name: String,
    pub enabled: bool,
    pub action: TaskAction,
    pub schedule: Schedule,
    /// Dernière modification (secondes Unix, attribuée par le cœur) : aucune occurrence antérieure
    /// n'est exécutée, et c'est l'origine des intervalles.
    #[serde(default)]
    pub updated_at: i64,
}

impl ScheduledTask {
    /// Avance avec laquelle la tâche démarre avant son heure (annonces d'un redémarrage).
    pub fn lead_s(&self) -> i64 {
        match &self.action {
            TaskAction::Restart { warnings, .. } => {
                warnings.iter().copied().filter(|w| *w <= MAX_WARNING_MINUTES).max().unwrap_or(0) as i64 * 60
            }
            _ => 0,
        }
    }

    /// Libellé lisible (journal, alertes) : le nom, sinon le type d'action.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            return self.name.trim().to_string();
        }
        match self.action {
            TaskAction::Restart { .. } => "Redémarrage programmé".into(),
            TaskAction::Snapshot { .. } => "Snapshot programmé".into(),
            TaskAction::Commands { .. } => "Commandes programmées".into(),
        }
    }
}

/// Action d'une tâche.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum TaskAction {
    /// Redémarrage, précédé d'annonces aux joueurs `warnings` minutes avant (ex. `[10, 5, 1]`).
    /// `message` : texte de l'annonce, `{minutes}` remplacé par le délai restant. Sans effet si le
    /// jeu n'a pas de commande de diffusion.
    Restart { warnings: Vec<u32>, message: String },
    /// Snapshot (point de sauvegarde MineStrator). `name` vide : nom daté automatique.
    Snapshot { name: String },
    /// Commandes console envoyées dans l'ordre (une par ligne).
    Commands { commands: Vec<String> },
}

impl TaskAction {
    fn kind(&self) -> &'static str {
        match self {
            TaskAction::Restart { .. } => "restart",
            TaskAction::Snapshot { .. } => "snapshot",
            TaskAction::Commands { .. } => "commands",
        }
    }
}

/// Récurrence d'une tâche, en heure locale de la machine qui l'exécute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Schedule {
    /// Tous les jours à `hour:minute`.
    Daily { hour: u8, minute: u8 },
    /// Certains jours (`1` = lundi … `7` = dimanche, ISO 8601) à `hour:minute`.
    Weekly { days: Vec<u8>, hour: u8, minute: u8 },
    /// Toutes les `every_minutes` minutes, à compter de la dernière modification de la tâche.
    Interval { every_minutes: u32 },
}

/// Une exécution consignée (tâche planifiée ou redémarrage automatique).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutomationRun {
    /// Identifiant dans la base de la machine qui l'a exécutée (app ou service : pas unique entre
    /// les deux).
    pub id: i64,
    pub account_id: i64,
    pub server_id: i64,
    /// Tâche planifiée (`null` : redémarrage automatique).
    pub task_id: Option<String>,
    /// Libellé de la tâche au moment de l'exécution.
    pub task_name: Option<String>,
    /// `restart` | `snapshot` | `commands` | `auto_restart`
    #[cfg_attr(feature = "ts", ts(type = "'restart' | 'snapshot' | 'commands' | 'auto_restart'"))]
    pub kind: String,
    /// Occurrence planifiée (tâches).
    pub scheduled_ts: Option<i64>,
    pub started_ts: i64,
    pub finished_ts: Option<i64>,
    /// `ok` | `failed` | `skipped` | `missed` | `gave_up`
    #[cfg_attr(feature = "ts", ts(type = "'ok' | 'failed' | 'skipped' | 'missed' | 'gave_up'"))]
    pub status: String,
    /// Résultat lisible (français).
    pub message: String,
    /// Détail anonymisé (erreur, logs de crash).
    pub details: Option<String>,
}

/// Tâche + sa prochaine exécution (vue UI).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TaskInfo {
    pub task: ScheduledTask,
    /// Prochaine occurrence (secondes Unix), `null` si la tâche est désactivée.
    pub next_run: Option<i64>,
}

/// Automatisations d'un serveur, pour le compte courant (vue UI).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ServerAutomations {
    pub server_id: i64,
    pub account_id: i64,
    pub restart: RestartRule,
    pub tasks: Vec<TaskInfo>,
    /// Qui exécute en ce moment : `app` | `service`.
    #[cfg_attr(feature = "ts", ts(type = "'app' | 'service'"))]
    pub executor: String,
    /// Retard maximal toléré d'une occurrence (secondes).
    pub tolerance_s: i64,
    /// Délais avant chaque tentative de redémarrage automatique (secondes).
    pub backoff_s: Vec<i64>,
    /// Commande de diffusion du jeu (`null` : pas d'annonce possible).
    pub broadcast: Option<String>,
}

// --- Calcul des occurrences (pur) ---------------------------------------------------------

/// Instant (secondes Unix) d'une heure locale « murale ». Heure d'été :
/// - heure **ambiguë** (retour à l'heure d'hiver) : la première des deux, une seule exécution ;
/// - heure **inexistante** (passage à l'heure d'été) : décalée d'une heure — soit l'instant où
///   l'horloge l'aurait affichée avec l'ancien décalage.
fn resolve_local<Tz: TimeZone>(tz: &Tz, naive: NaiveDateTime) -> Option<i64> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.timestamp()),
        LocalResult::Ambiguous(a, b) => Some(a.timestamp().min(b.timestamp())),
        LocalResult::None => (1..=3).find_map(|h| {
            tz.from_local_datetime(&(naive + chrono::TimeDelta::hours(h)))
                .earliest()
                .map(|dt| dt.timestamp())
        }),
    }
}

fn next_wall_clock<Tz: TimeZone>(
    days: Option<&[u8]>,
    hour: u8,
    minute: u8,
    after: i64,
    tz: &Tz,
) -> Option<i64> {
    let start = tz.timestamp_opt(after, 0).single()?.date_naive();
    for offset in 0..=8u64 {
        let date = start.checked_add_days(chrono::Days::new(offset))?;
        if let Some(days) = days {
            if !days.contains(&(date.weekday().number_from_monday() as u8)) {
                continue;
            }
        }
        let naive = date.and_hms_opt(hour.into(), minute.into(), 0)?;
        match resolve_local(tz, naive) {
            Some(ts) if ts > after => return Some(ts),
            _ => {}
        }
    }
    None
}

/// Prochaine occurrence **strictement** après `after` (secondes Unix), dans le fuseau `tz`.
/// `anchor` : origine des intervalles (dernière modification de la tâche).
pub fn next_occurrence<Tz: TimeZone>(
    schedule: &Schedule,
    anchor: i64,
    after: i64,
    tz: &Tz,
) -> Option<i64> {
    match schedule {
        Schedule::Daily { hour, minute } => next_wall_clock(None, *hour, *minute, after, tz),
        Schedule::Weekly { days, hour, minute } => {
            if days.is_empty() {
                return None;
            }
            next_wall_clock(Some(days), *hour, *minute, after, tz)
        }
        Schedule::Interval { every_minutes } => {
            let period = i64::from((*every_minutes).max(1)) * 60;
            let k = if after < anchor { 1 } else { (after - anchor) / period + 1 };
            Some(anchor + k * period)
        }
    }
}

/// Décision du planificateur pour une tâche, à l'instant `now`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Due {
    /// Occurrence à exécuter maintenant : sa fenêtre de départ (`occurrence - lead`) est
    /// atteinte et elle n'a pas plus de [`TOLERANCE_S`] secondes de retard.
    pub run: Option<i64>,
    /// Dernière occurrence trop ancienne pour être rattrapée (à consigner comme manquée).
    pub missed: Option<i64>,
}

/// Occurrences postérieures à `base` (dernière traitée) : laquelle exécuter, laquelle est
/// manquée. `lead` : avance de démarrage (annonces avant un redémarrage).
pub fn due<Tz: TimeZone>(
    schedule: &Schedule,
    anchor: i64,
    base: i64,
    now: i64,
    lead: i64,
    tz: &Tz,
) -> Due {
    let mut out = Due::default();
    let mut after = base;
    // Borne de sécurité : `base` est limité à LOOKBACK_S, et l'intervalle minimal à 5 min.
    for _ in 0..20_000 {
        let Some(occ) = next_occurrence(schedule, anchor, after, tz) else { break };
        if occ - lead > now {
            break;
        }
        if now - occ <= TOLERANCE_S {
            out.run = Some(occ);
            break;
        }
        out.missed = Some(occ);
        after = occ;
    }
    out
}

/// Point de départ de la recherche d'occurrences d'une tâche : la plus récente de sa dernière
/// occurrence traitée, de sa dernière modification, et du début de la période active du
/// planificateur moins la tolérance (on ne rattrape pas ce qui précède le lancement de l'app ou
/// du service au-delà de la tolérance — et on ne le signale pas comme manqué).
pub fn scheduling_base(
    last_handled: Option<i64>,
    updated_at: i64,
    active_since: i64,
    lead: i64,
    now: i64,
) -> i64 {
    [
        last_handled.unwrap_or(i64::MIN),
        updated_at,
        active_since - TOLERANCE_S - lead,
        now - LOOKBACK_S,
    ]
    .into_iter()
    .max()
    .unwrap_or(now)
}

/// Étape d'un redémarrage programmé.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Annonce « redémarrage dans `minutes` min ».
    Announce { minutes: u32 },
    Restart,
}

/// Déroulé d'un redémarrage programmé à `occurrence` : annonces aux minutes demandées (les plus
/// éloignées d'abord), puis redémarrage. Les annonces déjà passées (démarrage en retard) sont
/// omises ; un retard sur l'heure du redémarrage le fait partir tout de suite.
pub fn restart_plan(occurrence: i64, warnings: &[u32], now: i64) -> Vec<(i64, Step)> {
    let mut minutes: Vec<u32> = warnings
        .iter()
        .copied()
        .filter(|m| (1..=MAX_WARNING_MINUTES).contains(m))
        .collect();
    minutes.sort_unstable_by(|a, b| b.cmp(a));
    minutes.dedup();
    let mut plan: Vec<(i64, Step)> = minutes
        .into_iter()
        .map(|m| (occurrence - i64::from(m) * 60, Step::Announce { minutes: m }))
        .filter(|(at, _)| *at >= now - ANNOUNCE_GRACE_S)
        .map(|(at, step)| (at.max(now), step))
        .collect();
    plan.push((occurrence.max(now), Step::Restart));
    plan
}

/// Commande console d'annonce : `template` du jeu (`{message}`), message de l'utilisateur
/// (`{minutes}` remplacé). Les caractères de contrôle sont neutralisés — un saut de ligne
/// enverrait une **seconde commande** à la console — et le texte est borné. `None` si vide.
pub fn announcement(template: &str, message: &str, minutes: u32) -> Option<String> {
    let text: String = message
        .replace("{minutes}", &minutes.to_string())
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(MAX_MESSAGE_LEN).collect();
    if text.is_empty() {
        return None;
    }
    Some(template.replace("{message}", &text))
}

/// Décision de redémarrage automatique après un crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartDecision {
    /// Relancer après `delay_s` secondes ; c'est la tentative n° `attempt` de l'heure.
    Restart { delay_s: i64, attempt: u32 },
    /// Plafond horaire atteint : on s'arrête (alerte critique).
    GiveUp,
}

/// `recent` : redémarrages automatiques déjà lancés pour ce serveur (dates). Délai croissant
/// ([`RESTART_BACKOFF_S`]) et au plus `max_per_hour` redémarrages sur une heure glissante.
pub fn plan_auto_restart(recent: &[i64], now: i64, max_per_hour: u32) -> RestartDecision {
    let count = recent.iter().filter(|t| now - **t < 3600).count();
    if count >= max_per_hour.clamp(1, MAX_RESTARTS_PER_HOUR) as usize {
        return RestartDecision::GiveUp;
    }
    RestartDecision::Restart {
        delay_s: RESTART_BACKOFF_S[count.min(RESTART_BACKOFF_S.len() - 1)],
        attempt: count as u32 + 1,
    }
}

// --- Validation (pure) --------------------------------------------------------------------

fn invalid(msg: impl Into<String>) -> Error {
    Error::Unexpected(msg.into())
}

/// Valide et normalise une tâche saisie dans l'UI (bornes, jours triés, commandes nettoyées).
pub fn normalize_task(mut task: ScheduledTask) -> Result<ScheduledTask> {
    task.name = task.name.trim().chars().take(MAX_NAME_LEN).collect();
    match &mut task.schedule {
        Schedule::Daily { hour, minute } => check_time(*hour, *minute)?,
        Schedule::Weekly { days, hour, minute } => {
            check_time(*hour, *minute)?;
            days.retain(|d| (1..=7).contains(d));
            days.sort_unstable();
            days.dedup();
            if days.is_empty() {
                return Err(invalid("Choisis au moins un jour de la semaine."));
            }
        }
        Schedule::Interval { every_minutes } => {
            if !(MIN_INTERVAL_MINUTES..=MAX_INTERVAL_MINUTES).contains(every_minutes) {
                return Err(invalid(format!(
                    "Intervalle hors bornes ({MIN_INTERVAL_MINUTES} min à {} jours).",
                    MAX_INTERVAL_MINUTES / 1440
                )));
            }
        }
    }
    match &mut task.action {
        TaskAction::Restart { warnings, message } => {
            warnings.retain(|m| (1..=MAX_WARNING_MINUTES).contains(m));
            warnings.sort_unstable_by(|a, b| b.cmp(a));
            warnings.dedup();
            warnings.truncate(MAX_WARNINGS);
            *message = message.trim().chars().take(MAX_MESSAGE_LEN).collect();
        }
        TaskAction::Snapshot { name } => {
            *name = name.trim().chars().take(MAX_NAME_LEN).collect();
        }
        TaskAction::Commands { commands } => {
            let cleaned: Vec<String> = commands
                .iter()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect();
            if cleaned.is_empty() {
                return Err(invalid("Indique au moins une commande."));
            }
            if cleaned.len() > MAX_COMMANDS {
                return Err(invalid(format!("{MAX_COMMANDS} commandes au plus.")));
            }
            if cleaned.iter().any(|c| c.len() > MAX_COMMAND_LEN || c.chars().any(char::is_control)) {
                return Err(invalid(format!(
                    "Commande invalide : une ligne par commande, {MAX_COMMAND_LEN} caractères au plus."
                )));
            }
            *commands = cleaned;
        }
    }
    Ok(task)
}

fn check_time(hour: u8, minute: u8) -> Result<()> {
    if hour > 23 || minute > 59 {
        return Err(invalid("Heure invalide."));
    }
    Ok(())
}

/// Identifiant unique (non secret) : préfixe + horloge + compteur + processus.
pub(crate) fn new_id(prefix: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{nanos:x}{:x}{seq:x}", std::process::id())
}

// --- État d'exécution ------------------------------------------------------------------

/// État des automatisations, porté par le [`Core`].
pub(crate) struct AutomationState {
    config: PersistedConfig<AutomationConfig>,
    /// Tâches en cours (une seule exécution à la fois par tâche).
    running: Mutex<HashSet<String>>,
    /// Serveurs dont un redémarrage automatique est en attente.
    restarting: Mutex<HashSet<i64>>,
    /// Dernière occurrence traitée par tâche (cache du journal).
    handled: Mutex<HashMap<String, i64>>,
    /// Dernière alerte « redémarrage automatique abandonné » par serveur.
    gave_up: Mutex<HashMap<i64, i64>>,
    /// Début de la période active du planificateur (lancement, sortie de veille).
    active_since: AtomicI64,
    wake: Notify,
}

impl AutomationState {
    pub(crate) fn load(dir: &Path) -> Self {
        Self {
            config: PersistedConfig::load(dir, "automations.json"),
            running: Mutex::new(HashSet::new()),
            restarting: Mutex::new(HashSet::new()),
            handled: Mutex::new(HashMap::new()),
            gave_up: Mutex::new(HashMap::new()),
            active_since: AtomicI64::new(now_secs()),
            wake: Notify::new(),
        }
    }

    fn cooled_give_up(&self, id: i64, now: i64) -> bool {
        let mut map = self.gave_up.lock().unwrap();
        if map.get(&id).is_none_or(|t| now - t >= GIVE_UP_COOLDOWN_S) {
            map.insert(id, now);
            true
        } else {
            false
        }
    }
}

/// Issue d'une tâche.
enum TaskOutcome {
    Done(String),
    Skipped(String),
    Failed(String),
}

// --- Façade ------------------------------------------------------------------------------

impl Core {
    /// Configuration complète (tous comptes) — transmise au service d'arrière-plan.
    pub fn automation_config(&self) -> AutomationConfig {
        self.automation.config.get()
    }

    /// Remplace la configuration (réception par le service d'arrière-plan), après l'avoir validée
    /// comme une saisie de l'UI. Les tâches d'un autre compte que celui de la clé du processus
    /// sont conservées mais jamais exécutées.
    pub fn apply_automation_config(&self, cfg: AutomationConfig) -> Result<()> {
        if cfg.tasks.len() > 10 * MAX_TASKS_PER_ACCOUNT {
            return Err(invalid("trop de tâches"));
        }
        let mut tasks = Vec::with_capacity(cfg.tasks.len());
        for task in cfg.tasks {
            if task.id.is_empty() || task.id.len() > 64 {
                return Err(invalid("tâche sans identifiant valide"));
            }
            tasks.push(normalize_task(task)?);
        }
        let restart_rules = cfg
            .restart_rules
            .into_iter()
            .map(|mut r| {
                r.max_per_hour = r.max_per_hour.clamp(1, MAX_RESTARTS_PER_HOUR);
                r
            })
            .collect();
        // Le cache des occurrences traitées n'est valable que pour des tâches inchangées : il est
        // reconstruit depuis le journal.
        self.automation.handled.lock().unwrap().clear();
        self.automation.config.set(AutomationConfig { restart_rules, tasks });
        self.automation.wake.notify_one();
        Ok(())
    }

    /// Démarre les automatisations (planificateur, redémarrage automatique, webhooks), dans un
    /// runtime tokio, **après** le superviseur. Appelé par l'app desktop et par le service.
    pub fn start_automations(self: Arc<Self>) {
        self.automation.active_since.store(now_secs(), Ordering::SeqCst);
        self.clone().spawn_scheduler();
        self.clone().spawn_auto_restart();
        self.start_webhooks();
    }

    /// Automatisations d'un serveur pour le compte courant.
    pub async fn server_automations(&self, server_id: i64) -> Result<ServerAutomations> {
        let account = self.user_id().await?;
        let cfg = self.automation.config.get();
        let now = now_secs();
        let restart = cfg
            .restart_rules
            .iter()
            .find(|r| r.account_id == account && r.server_id == server_id)
            .cloned()
            .unwrap_or_else(|| RestartRule::disabled(account, server_id));
        let tasks = cfg
            .tasks
            .into_iter()
            .filter(|t| t.account_id == account && t.server_id == server_id)
            .map(|task| TaskInfo { next_run: self.next_run(&task, now), task })
            .collect();
        let broadcast = self.broadcast_template(server_id).await;
        Ok(ServerAutomations {
            server_id,
            account_id: account,
            restart,
            tasks,
            executor: if self.service_mode() { "service" } else { "app" }.into(),
            tolerance_s: TOLERANCE_S,
            backoff_s: RESTART_BACKOFF_S.to_vec(),
            broadcast,
        })
    }

    /// Active ou règle le redémarrage automatique après crash d'un serveur.
    pub async fn set_auto_restart(
        &self,
        server_id: i64,
        enabled: bool,
        max_per_hour: u32,
    ) -> Result<RestartRule> {
        let account = self.user_id().await?;
        let rule = RestartRule {
            account_id: account,
            server_id,
            enabled,
            max_per_hour: max_per_hour.clamp(1, MAX_RESTARTS_PER_HOUR),
        };
        let mut cfg = self.automation.config.get();
        cfg.restart_rules.retain(|r| !(r.account_id == account && r.server_id == server_id));
        cfg.restart_rules.push(rule.clone());
        self.automation.config.set(cfg);
        self.sync_automations().await;
        Ok(rule)
    }

    /// Crée (`id` vide) ou met à jour une tâche planifiée du compte courant.
    pub async fn save_scheduled_task(&self, task: ScheduledTask) -> Result<TaskInfo> {
        let account = self.user_id().await?;
        let mut task = normalize_task(task)?;
        let mut cfg = self.automation.config.get();
        if task.id.is_empty() {
            let owned = cfg.tasks.iter().filter(|t| t.account_id == account).count();
            if owned >= MAX_TASKS_PER_ACCOUNT {
                return Err(invalid(format!("{MAX_TASKS_PER_ACCOUNT} tâches au plus.")));
            }
            task.id = new_id("task");
        } else if !cfg.tasks.iter().any(|t| t.id == task.id && t.account_id == account) {
            return Err(invalid("Tâche introuvable."));
        }
        task.account_id = account;
        task.updated_at = now_secs();
        match cfg.tasks.iter_mut().find(|t| t.id == task.id) {
            Some(existing) => *existing = task.clone(),
            None => cfg.tasks.push(task.clone()),
        }
        self.automation.handled.lock().unwrap().remove(&task.id);
        self.automation.config.set(cfg);
        self.automation.wake.notify_one();
        self.sync_automations().await;
        Ok(TaskInfo { next_run: self.next_run(&task, now_secs()), task })
    }

    /// Supprime une tâche planifiée du compte courant.
    pub async fn delete_scheduled_task(&self, task_id: &str) -> Result<()> {
        let account = self.user_id().await?;
        let mut cfg = self.automation.config.get();
        let before = cfg.tasks.len();
        cfg.tasks.retain(|t| !(t.id == task_id && t.account_id == account));
        if cfg.tasks.len() == before {
            return Err(invalid("Tâche introuvable."));
        }
        self.automation.config.set(cfg);
        self.sync_automations().await;
        Ok(())
    }

    /// Journal local des exécutions (le service d'arrière-plan y répond par IPC).
    pub fn local_automation_runs(
        &self,
        account_id: i64,
        server_id: Option<i64>,
        limit: u32,
    ) -> Result<Vec<AutomationRun>> {
        self.store.runs(account_id, server_id, limit.clamp(1, 500))
    }

    // --- Planificateur -------------------------------------------------------------------

    fn spawn_scheduler(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut was_active = true;
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(TICK) => {}
                    _ = self.automation.wake.notified() => {}
                }
                // En veille, c'est le service d'arrière-plan qui exécute ; au retour, on repart
                // de maintenant (ce qu'il a traité ne doit être ni rejoué ni signalé manqué).
                if self.sup_state.is_suspended() {
                    was_active = false;
                    continue;
                }
                if !was_active {
                    self.automation.active_since.store(now_secs(), Ordering::SeqCst);
                    was_active = true;
                }
                self.scheduler_tick().await;
            }
        });
    }

    async fn scheduler_tick(self: &Arc<Self>) {
        // Sans clé (ou hors ligne au premier appel) : rien à exécuter.
        let Ok(account) = self.user_id().await else { return };
        let now = now_secs();
        let active_since = self.automation.active_since.load(Ordering::SeqCst);
        let tasks: Vec<ScheduledTask> = self.automation.config.with(|c| {
            c.tasks
                .iter()
                .filter(|t| t.enabled && t.account_id == account)
                .cloned()
                .collect()
        });
        for task in tasks {
            if self.automation.running.lock().unwrap().contains(&task.id) {
                continue;
            }
            let lead = task.lead_s();
            let base = scheduling_base(self.last_handled(&task.id), task.updated_at, active_since, lead, now);
            let due = due(&task.schedule, task.updated_at, base, now, lead, &chrono::Local);
            if let Some(missed) = due.missed {
                self.record_run(&task, Some(missed), now, Some(now), "missed",
                    "Occurrence manquée : l'app ou l'ordinateur n'était pas disponible à l'heure prévue.".into(),
                    None);
                self.set_handled(&task.id, missed);
            }
            let Some(occurrence) = due.run else { continue };
            // Dernier contrôle : si le service d'arrière-plan vient de prendre la main, c'est lui
            // qui exécute.
            if !self.is_active_supervisor().await {
                return;
            }
            self.set_handled(&task.id, occurrence);
            self.automation.running.lock().unwrap().insert(task.id.clone());
            let core = self.clone();
            tokio::spawn(async move { core.execute_task(task, occurrence).await });
        }
    }

    fn last_handled(&self, task_id: &str) -> Option<i64> {
        if let Some(t) = self.automation.handled.lock().unwrap().get(task_id) {
            return Some(*t);
        }
        let t = self.store.last_scheduled(task_id).ok().flatten();
        if let Some(t) = t {
            self.automation.handled.lock().unwrap().insert(task_id.to_string(), t);
        }
        t
    }

    fn set_handled(&self, task_id: &str, ts: i64) {
        self.automation.handled.lock().unwrap().insert(task_id.to_string(), ts);
    }

    /// Prochaine exécution affichée d'une tâche.
    fn next_run(&self, task: &ScheduledTask, now: i64) -> Option<i64> {
        if !task.enabled {
            return None;
        }
        let after = [now, task.updated_at, self.last_handled(&task.id).unwrap_or(i64::MIN)]
            .into_iter()
            .max()
            .unwrap_or(now);
        next_occurrence(&task.schedule, task.updated_at, after, &chrono::Local)
    }

    /// La tâche existe-t-elle toujours, active et inchangée ? (vérifié avant chaque étape)
    fn task_unchanged(&self, task: &ScheduledTask) -> bool {
        self.automation
            .config
            .with(|c| c.tasks.iter().any(|t| t == task))
    }

    async fn execute_task(self: Arc<Self>, task: ScheduledTask, occurrence: i64) {
        let started = now_secs();
        let outcome = match &task.action {
            TaskAction::Restart { warnings, message } => {
                self.run_scheduled_restart(&task, occurrence, warnings, message).await
            }
            TaskAction::Snapshot { name } => self.run_snapshot(&task, name).await,
            TaskAction::Commands { commands } => self.run_commands(&task, commands).await,
        };
        let finished = now_secs();
        let name = self.server_name(task.server_id);
        match outcome {
            TaskOutcome::Done(msg) => {
                self.record_run(&task, Some(occurrence), started, Some(finished), "ok", msg.clone(), None);
                self.notify_webhooks(Notice {
                    event: WebhookEvent::TaskSuccess,
                    severity: "info".into(),
                    server_id: task.server_id,
                    server_name: name,
                    message: format!("{} : {msg}", task.label()),
                    details: None,
                    ts: finished,
                });
            }
            TaskOutcome::Skipped(msg) => {
                self.record_run(&task, Some(occurrence), started, Some(finished), "skipped", msg, None);
            }
            TaskOutcome::Failed(err) => {
                let err = crate::redact::redact(&err);
                self.record_run(&task, Some(occurrence), started, Some(finished), "failed",
                    "Échec de la tâche.".into(), Some(err.clone()));
                self.emit(CoreEvent::Alert(Alert {
                    server_id: task.server_id,
                    server_name: name.clone(),
                    kind: "task".into(),
                    severity: "warning".into(),
                    message: format!("« {name} » — la tâche « {} » a échoué : {err}", task.label()),
                    ts: finished,
                    details: Some(err),
                }));
            }
        }
        self.automation.running.lock().unwrap().remove(&task.id);
    }

    #[allow(clippy::too_many_arguments)]
    fn record_run(
        &self,
        task: &ScheduledTask,
        scheduled: Option<i64>,
        started: i64,
        finished: Option<i64>,
        status: &str,
        message: String,
        details: Option<String>,
    ) {
        let run = AutomationRun {
            id: 0,
            account_id: task.account_id,
            server_id: task.server_id,
            task_id: Some(task.id.clone()),
            task_name: Some(task.label()),
            kind: task.action.kind().into(),
            scheduled_ts: scheduled,
            started_ts: started,
            finished_ts: finished,
            status: status.into(),
            message,
            details,
        };
        if let Err(e) = self.store.insert_run(&run) {
            tracing::warn!("journal des automatisations : {e}");
        }
    }

    /// Commande de diffusion du jeu d'un serveur (capacités de la liste des serveurs).
    async fn broadcast_template(&self, server_id: i64) -> Option<String> {
        self.list_servers()
            .await
            .ok()?
            .servers
            .into_iter()
            .find(|s| s.id == server_id)?
            .capabilities
            .broadcast
    }

    /// État courant d'un serveur selon l'API (`online` | `starting` | `stopping` | `offline`).
    async fn live_state(&self, server_id: i64) -> Result<String> {
        Ok(self.server_live(server_id).await?.state)
    }

    async fn sleep_until(at: i64) {
        let wait = at - now_secs();
        if wait > 0 {
            tokio::time::sleep(Duration::from_secs(wait as u64)).await;
        }
    }

    async fn run_scheduled_restart(
        &self,
        task: &ScheduledTask,
        occurrence: i64,
        warnings: &[u32],
        message: &str,
    ) -> TaskOutcome {
        let id = task.server_id;
        // Un redémarrage programmé ne démarre jamais un serveur arrêté volontairement.
        match self.live_state(id).await {
            Ok(s) if s == "offline" => return TaskOutcome::Skipped("Serveur arrêté : redémarrage ignoré.".into()),
            Ok(_) => {}
            Err(e) => return TaskOutcome::Failed(format!("état du serveur indisponible : {e}")),
        }
        let template = self.broadcast_template(id).await;
        let mut announced = 0;
        for (at, step) in restart_plan(occurrence, warnings, now_secs()) {
            Self::sleep_until(at).await;
            if !self.task_unchanged(task) {
                return TaskOutcome::Skipped("Tâche modifiée ou supprimée pendant le compte à rebours.".into());
            }
            if !self.is_active_supervisor().await {
                return TaskOutcome::Skipped("Exécution reprise par le service d'arrière-plan.".into());
            }
            match step {
                Step::Announce { minutes } => {
                    let Some(cmd) = template.as_deref().and_then(|t| announcement(t, message, minutes)) else {
                        continue;
                    };
                    match self.send_command(id, &cmd).await {
                        Ok(()) => announced += 1,
                        Err(e) => tracing::warn!(server = id, "annonce de redémarrage non envoyée : {e}"),
                    }
                }
                Step::Restart => {
                    if let Ok(s) = self.live_state(id).await {
                        if s == "offline" {
                            return TaskOutcome::Skipped(
                                "Serveur arrêté pendant le compte à rebours : redémarrage annulé.".into(),
                            );
                        }
                    }
                    return match self.power_action(id, "restart").await {
                        Ok(()) => TaskOutcome::Done(format!(
                            "Redémarrage lancé ({announced} annonce(s) envoyée(s))."
                        )),
                        Err(e) => TaskOutcome::Failed(e.to_string()),
                    };
                }
            }
        }
        TaskOutcome::Failed("déroulé du redémarrage incomplet".into())
    }

    async fn run_snapshot(&self, task: &ScheduledTask, name: &str) -> TaskOutcome {
        let name = if name.trim().is_empty() {
            format!("Auto {}", chrono::Local::now().format("%Y-%m-%d %H:%M"))
        } else {
            name.trim().to_string()
        };
        match self.create_snapshot(task.server_id, &name).await {
            Ok(job) => TaskOutcome::Done(format!("Snapshot « {name} » demandé (opération n° {job}).")),
            Err(e) => TaskOutcome::Failed(e.to_string()),
        }
    }

    async fn run_commands(&self, task: &ScheduledTask, commands: &[String]) -> TaskOutcome {
        let id = task.server_id;
        match self.live_state(id).await {
            Ok(s) if s == "offline" => {
                return TaskOutcome::Skipped("Serveur arrêté : commandes non envoyées.".into())
            }
            Ok(_) => {}
            Err(e) => return TaskOutcome::Failed(format!("état du serveur indisponible : {e}")),
        }
        let total = commands.len();
        for (i, cmd) in commands.iter().enumerate() {
            if i > 0 {
                tokio::time::sleep(COMMAND_GAP).await;
            }
            if let Err(e) = self.send_command(id, cmd).await {
                return TaskOutcome::Failed(format!("commande {} sur {total} : {e}", i + 1));
            }
        }
        TaskOutcome::Done(format!("{total} commande(s) envoyée(s)."))
    }

    // --- Redémarrage automatique après crash ----------------------------------------------

    fn spawn_auto_restart(self: Arc<Self>) {
        let mut rx = self.sup_state.subscribe_crashes();
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(crash) => {
                        let core = self.clone();
                        tokio::spawn(async move { core.handle_crash(crash).await });
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn restart_rule(&self, account: i64, server_id: i64) -> Option<RestartRule> {
        self.automation.config.with(|c| {
            c.restart_rules
                .iter()
                .find(|r| r.account_id == account && r.server_id == server_id && r.enabled)
                .cloned()
        })
    }

    async fn handle_crash(self: Arc<Self>, crash: CrashEvent) {
        let id = crash.server_id;
        let Ok(account) = self.user_id().await else { return };
        let Some(rule) = self.restart_rule(account, id) else { return };
        // Un redémarrage est déjà en attente pour ce serveur.
        if !self.automation.restarting.lock().unwrap().insert(id) {
            return;
        }
        self.auto_restart(account, rule, crash).await;
        self.automation.restarting.lock().unwrap().remove(&id);
    }

    fn record_auto_restart(&self, account: i64, id: i64, status: &str, message: String, details: Option<String>) {
        let now = now_secs();
        let run = AutomationRun {
            id: 0,
            account_id: account,
            server_id: id,
            task_id: None,
            task_name: None,
            kind: "auto_restart".into(),
            scheduled_ts: None,
            started_ts: now,
            finished_ts: Some(now),
            status: status.into(),
            message,
            details,
        };
        if let Err(e) = self.store.insert_run(&run) {
            tracing::warn!("journal des automatisations : {e}");
        }
    }

    async fn auto_restart(&self, account: i64, rule: RestartRule, crash: CrashEvent) {
        let id = crash.server_id;
        let now = now_secs();
        let recent = self.store.auto_restarts_since(id, now - 3600).unwrap_or_default();
        let name = self.server_name(id);
        let max = rule.max_per_hour.clamp(1, MAX_RESTARTS_PER_HOUR);
        let (delay_s, attempt) = match plan_auto_restart(&recent, now, max) {
            RestartDecision::Restart { delay_s, attempt } => (delay_s, attempt),
            RestartDecision::GiveUp => {
                if self.automation.cooled_give_up(id, now) {
                    let crashes = recent.len() + 1;
                    // Le journal ne nomme pas le serveur (la vue est déjà la sienne) ; l'alerte, si.
                    self.record_auto_restart(account, id, "gave_up",
                        format!("{crashes} crashs en une heure : redémarrage automatique suspendu."),
                        crash.details.clone());
                    let msg = format!(
                        "« {name} » a planté {crashes} fois en une heure : redémarrage automatique suspendu."
                    );
                    self.emit(CoreEvent::Alert(Alert {
                        server_id: id,
                        server_name: name,
                        kind: "auto_restart".into(),
                        severity: "critical".into(),
                        message: msg,
                        ts: crash.ts,
                        details: crash.details,
                    }));
                }
                return;
            }
        };
        tokio::time::sleep(Duration::from_secs(delay_s as u64)).await;

        // Contrôles au moment d'agir : règle toujours active, superviseur toujours actif, serveur
        // toujours arrêté (l'utilisateur a pu le relancer lui-même entre-temps).
        if self.restart_rule(account, id).is_none() || !self.is_active_supervisor().await {
            return;
        }
        let still_offline = match self.sup_state.current_state(id) {
            Some(s) => s == "offline",
            None => matches!(self.live_state(id).await.as_deref(), Ok("offline")),
        };
        if !still_offline {
            self.record_auto_restart(account, id, "skipped",
                "Serveur déjà relancé : redémarrage automatique inutile.".into(), None);
            return;
        }
        match self.power_action(id, "start").await {
            Ok(()) => {
                let msg = format!(
                    "« {name} » a été redémarré automatiquement après un crash (tentative {attempt}/{max})."
                );
                self.record_auto_restart(account, id, "ok",
                    format!("Redémarrage automatique après crash (tentative {attempt}/{max})."), None);
                self.emit(CoreEvent::Alert(Alert {
                    server_id: id,
                    server_name: name,
                    kind: "auto_restart".into(),
                    severity: "info".into(),
                    message: msg,
                    ts: now_secs(),
                    details: None,
                }));
            }
            Err(e) => {
                let err = crate::redact::redact(&e.to_string());
                self.record_auto_restart(account, id, "failed",
                    "Échec du redémarrage automatique.".into(), Some(err.clone()));
                self.emit(CoreEvent::Alert(Alert {
                    server_id: id,
                    server_name: name.clone(),
                    kind: "auto_restart".into(),
                    severity: "critical".into(),
                    message: format!("Échec du redémarrage automatique de « {name} » : {err}"),
                    ts: now_secs(),
                    details: Some(err),
                }));
            }
        }
    }
}

// --- Tests ----------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, NaiveDate, Offset};

    /// 2026-11-18 (mercredi) à `h:m` UTC.
    fn utc(day: u32, h: u32, m: u32) -> i64 {
        NaiveDate::from_ymd_opt(2026, 11, day)
            .unwrap()
            .and_hms_opt(h, m, 0)
            .unwrap()
            .and_utc()
            .timestamp()
    }

    fn paris_winter() -> FixedOffset {
        FixedOffset::east_opt(3600).unwrap()
    }

    /// Fuseau de test à changement d'heure : décalage `before` jusqu'à l'instant UTC `switch`,
    /// `after` ensuite (passage à l'heure d'été si `after > before`, retour sinon).
    #[derive(Clone, Copy, Debug)]
    struct FakeDst {
        before: i32,
        after: i32,
        switch: i64,
    }

    impl TimeZone for FakeDst {
        type Offset = FixedOffset;
        fn from_offset(offset: &FixedOffset) -> Self {
            let s = offset.local_minus_utc();
            FakeDst { before: s, after: s, switch: 0 }
        }
        fn offset_from_local_date(&self, _: &NaiveDate) -> LocalResult<FixedOffset> {
            LocalResult::Single(FixedOffset::east_opt(self.before).unwrap())
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let l = local.and_utc().timestamp();
            let a = (l - i64::from(self.before) < self.switch).then(|| FixedOffset::east_opt(self.before).unwrap());
            let b = (l - i64::from(self.after) >= self.switch).then(|| FixedOffset::east_opt(self.after).unwrap());
            match (a, b) {
                (Some(x), Some(y)) if x != y => LocalResult::Ambiguous(x, y),
                (Some(x), _) | (None, Some(x)) => LocalResult::Single(x),
                (None, None) => LocalResult::None,
            }
        }
        fn offset_from_utc_date(&self, _: &NaiveDate) -> FixedOffset {
            FixedOffset::east_opt(self.before).unwrap()
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            let s = if utc.and_utc().timestamp() < self.switch { self.before } else { self.after };
            FixedOffset::east_opt(s).unwrap()
        }
    }

    // --- Occurrences ----------------------------------------------------------------------

    #[test]
    fn quotidien_a_heure_locale() {
        let tz = paris_winter();
        let daily = Schedule::Daily { hour: 4, minute: 0 };
        // 04:00 à Paris (UTC+1) = 03:00 UTC.
        assert_eq!(next_occurrence(&daily, 0, utc(18, 1, 0), &tz), Some(utc(18, 3, 0)));
        // Strictement après : à 03:00 UTC pile, c'est le lendemain.
        assert_eq!(next_occurrence(&daily, 0, utc(18, 3, 0), &tz), Some(utc(19, 3, 0)));
        assert_eq!(next_occurrence(&daily, 0, utc(18, 10, 0), &tz), Some(utc(19, 3, 0)));
        // Juste avant minuit local (23:30 Paris = 22:30 UTC).
        assert_eq!(
            next_occurrence(&Schedule::Daily { hour: 0, minute: 15 }, 0, utc(18, 22, 30), &tz),
            Some(utc(18, 23, 15))
        );
    }

    #[test]
    fn certains_jours_de_la_semaine() {
        let tz = paris_winter();
        // Le 18/11/2026 est un mercredi (3). Lundi (1) et vendredi (5) à 20:00 Paris.
        let weekly = Schedule::Weekly { days: vec![1, 5], hour: 20, minute: 0 };
        assert_eq!(next_occurrence(&weekly, 0, utc(18, 12, 0), &tz), Some(utc(20, 19, 0)));
        assert_eq!(next_occurrence(&weekly, 0, utc(20, 19, 0), &tz), Some(utc(23, 19, 0)));
        // Mercredi même, pas encore passé.
        let wed = Schedule::Weekly { days: vec![3], hour: 20, minute: 0 };
        assert_eq!(next_occurrence(&wed, 0, utc(18, 12, 0), &tz), Some(utc(18, 19, 0)));
        // Mercredi passé : la semaine suivante (7 jours plus loin).
        assert_eq!(next_occurrence(&wed, 0, utc(18, 20, 0), &tz), Some(utc(25, 19, 0)));
        assert_eq!(next_occurrence(&Schedule::Weekly { days: vec![], hour: 1, minute: 0 }, 0, 0, &tz), None);
    }

    #[test]
    fn intervalle_depuis_la_modification() {
        let tz = paris_winter();
        let every = Schedule::Interval { every_minutes: 30 };
        let anchor = utc(18, 10, 7);
        assert_eq!(next_occurrence(&every, anchor, anchor, &tz), Some(anchor + 1800));
        assert_eq!(next_occurrence(&every, anchor, anchor - 999, &tz), Some(anchor + 1800));
        assert_eq!(next_occurrence(&every, anchor, anchor + 1799, &tz), Some(anchor + 1800));
        assert_eq!(next_occurrence(&every, anchor, anchor + 1800, &tz), Some(anchor + 3600));
        assert_eq!(next_occurrence(&every, anchor, anchor + 5 * 3600 + 1, &tz), Some(anchor + 5 * 3600 + 1800));
    }

    #[test]
    fn heure_dete_inexistante_et_ambigue() {
        // Passage à l'heure d'été (UTC+1 → UTC+2) le 29/03/2026 à 01:00 UTC (02:00 → 03:00 local).
        let spring = FakeDst {
            before: 3600,
            after: 7200,
            switch: NaiveDate::from_ymd_opt(2026, 3, 29).unwrap().and_hms_opt(1, 0, 0).unwrap().and_utc().timestamp(),
        };
        let at = |d: u32, h: u32, m: u32| {
            NaiveDate::from_ymd_opt(2026, 3, d).unwrap().and_hms_opt(h, m, 0).unwrap().and_utc().timestamp()
        };
        // 02:30 n'existe pas ce jour-là : exécutée à 03:30 heure d'été (= 01:30 UTC), une fois.
        let s = Schedule::Daily { hour: 2, minute: 30 };
        assert_eq!(next_occurrence(&s, 0, at(28, 12, 0), &spring), Some(at(29, 1, 30)));
        // Le lendemain, 02:30 heure d'été = 00:30 UTC.
        assert_eq!(next_occurrence(&s, 0, at(29, 1, 30), &spring), Some(at(30, 0, 30)));
        // 04:00 reste 04:00 locale : 02:00 UTC après le changement.
        assert_eq!(next_occurrence(&Schedule::Daily { hour: 4, minute: 0 }, 0, at(29, 0, 0), &spring), Some(at(29, 2, 0)));

        // Retour à l'heure d'hiver (UTC+2 → UTC+1) le 25/10/2026 à 01:00 UTC (03:00 → 02:00 local).
        let autumn = FakeDst {
            before: 7200,
            after: 3600,
            switch: NaiveDate::from_ymd_opt(2026, 10, 25).unwrap().and_hms_opt(1, 0, 0).unwrap().and_utc().timestamp(),
        };
        let oct = |d: u32, h: u32, m: u32| {
            NaiveDate::from_ymd_opt(2026, 10, d).unwrap().and_hms_opt(h, m, 0).unwrap().and_utc().timestamp()
        };
        // 02:30 existe deux fois : une seule exécution, à la première (00:30 UTC).
        let first = next_occurrence(&s, 0, oct(24, 12, 0), &autumn).unwrap();
        assert_eq!(first, oct(25, 0, 30));
        // Après elle : pas la seconde 02:30 (01:30 UTC) mais le lendemain.
        assert_eq!(next_occurrence(&s, 0, first, &autumn), Some(oct(26, 1, 30)));
    }

    // --- Tolérance et rattrapage ------------------------------------------------------------

    #[test]
    fn execution_dans_la_tolerance() {
        let tz = paris_winter();
        let daily = Schedule::Daily { hour: 4, minute: 0 };
        let occ = utc(18, 3, 0);
        // Pile à l'heure, ou un peu en retard : on exécute.
        assert_eq!(due(&daily, 0, occ - 3600, occ, 0, &tz), Due { run: Some(occ), missed: None });
        assert_eq!(due(&daily, 0, occ - 3600, occ + TOLERANCE_S, 0, &tz).run, Some(occ));
        // Trop tard : manquée, pas exécutée.
        assert_eq!(
            due(&daily, 0, occ - 3600, occ + TOLERANCE_S + 1, 0, &tz),
            Due { run: None, missed: Some(occ) }
        );
        // Pas encore l'heure.
        assert_eq!(due(&daily, 0, occ - 3600, occ - 1, 0, &tz), Due::default());
        // Déjà traitée : rien.
        assert_eq!(due(&daily, 0, occ, occ + 10, 0, &tz), Due::default());
    }

    #[test]
    fn avance_des_annonces() {
        let tz = paris_winter();
        let daily = Schedule::Daily { hour: 4, minute: 0 };
        let occ = utc(18, 3, 0);
        // Annonces 10 min avant : la tâche démarre dès occ - 10 min.
        assert_eq!(due(&daily, 0, occ - 3600, occ - 601, 600, &tz).run, None);
        assert_eq!(due(&daily, 0, occ - 3600, occ - 600, 600, &tz).run, Some(occ));
    }

    #[test]
    fn plusieurs_occurrences_manquees_une_seule_consignee() {
        let tz = paris_winter();
        let every = Schedule::Interval { every_minutes: 60 };
        let anchor = utc(18, 0, 0);
        // Ordinateur en veille de 01:00 à 05:02 : les occurrences de 01h à 04h sont manquées,
        // celle de 05:00 est dans la tolérance.
        let d = due(&every, anchor, anchor + 1800, anchor + 5 * 3600 + 120, 0, &tz);
        assert_eq!(d.run, Some(anchor + 5 * 3600));
        assert_eq!(d.missed, Some(anchor + 4 * 3600), "seule la plus récente est signalée");
    }

    #[test]
    fn point_de_depart_de_la_recherche() {
        let now = 1_000_000;
        // Nouvelle tâche : rien avant sa création.
        assert_eq!(scheduling_base(None, now - 10, now - 3600, 0, now), now - 10);
        // App lancée il y a une minute : on remonte jusqu'à la tolérance (+ l'avance), pas avant.
        assert_eq!(scheduling_base(None, 0, now - 60, 600, now), now - 60 - TOLERANCE_S - 600);
        // Dernière occurrence traitée plus récente que tout le reste.
        assert_eq!(scheduling_base(Some(now - 5), 0, 0, 0, now), now - 5);
        // Jamais plus loin que LOOKBACK_S.
        assert_eq!(scheduling_base(None, 0, 0, 0, now), now - LOOKBACK_S);
    }

    #[test]
    fn rien_nest_rattrape_au_lancement_au_dela_de_la_tolerance() {
        let tz = paris_winter();
        let daily = Schedule::Daily { hour: 4, minute: 0 };
        let occ = utc(18, 3, 0);
        // App lancée à 04:30 locale (03:30 UTC) : l'occurrence de 04:00 n'est ni exécutée ni
        // signalée manquée (le planificateur ne tournait pas).
        let started = occ + 1800;
        let base = scheduling_base(None, 0, started, 0, started + 20);
        assert_eq!(due(&daily, 0, base, started + 20, 0, &tz), Due::default());
        // Lancée à 04:02 : rattrapée.
        let started = occ + 120;
        let base = scheduling_base(None, 0, started, 0, started + 20);
        assert_eq!(due(&daily, 0, base, started + 20, 0, &tz).run, Some(occ));
    }

    // --- Redémarrage programmé ---------------------------------------------------------------

    #[test]
    fn deroule_dun_redemarrage_avec_annonces() {
        let occ = 10_000;
        let plan = restart_plan(occ, &[1, 10, 5, 5, 0, 99], occ - 600);
        assert_eq!(
            plan,
            vec![
                (occ - 600, Step::Announce { minutes: 10 }),
                (occ - 300, Step::Announce { minutes: 5 }),
                (occ - 60, Step::Announce { minutes: 1 }),
                (occ, Step::Restart),
            ]
        );
        // Démarré en retard (app lancée à T-4 min) : les annonces passées sont omises.
        let plan = restart_plan(occ, &[10, 5, 1], occ - 240);
        assert_eq!(plan, vec![(occ - 60, Step::Announce { minutes: 1 }), (occ, Step::Restart)]);
        // Léger retard (tick) : l'annonce part tout de suite.
        let plan = restart_plan(occ, &[5], occ - 290);
        assert_eq!(plan[0], (occ - 290, Step::Announce { minutes: 5 }));
        // Après l'heure (dans la tolérance) : redémarrage immédiat, sans annonce.
        assert_eq!(restart_plan(occ, &[5], occ + 60), vec![(occ + 60, Step::Restart)]);
        assert_eq!(restart_plan(occ, &[], occ - 10), vec![(occ, Step::Restart)]);
    }

    #[test]
    fn annonce_adaptee_au_jeu_et_neutralisee() {
        assert_eq!(
            announcement("say {message}", "Redémarrage dans {minutes} min !", 5).as_deref(),
            Some("say Redémarrage dans 5 min !")
        );
        assert_eq!(announcement("{message}", "Reboot in {minutes}m", 1).as_deref(), Some("Reboot in 1m"));
        // Un saut de ligne enverrait une seconde commande : neutralisé.
        assert_eq!(
            announcement("say {message}", "bye\nop Pirate\r\n", 1).as_deref(),
            Some("say bye op Pirate")
        );
        assert_eq!(announcement("say {message}", "  \n ", 1), None);
        let long = "a".repeat(500);
        assert_eq!(announcement("{message}", &long, 1).unwrap().len(), MAX_MESSAGE_LEN);
    }

    #[test]
    fn avance_dune_tache() {
        let task = |action| ScheduledTask {
            id: "t".into(),
            account_id: 1,
            server_id: 2,
            name: String::new(),
            enabled: true,
            action,
            schedule: Schedule::Daily { hour: 4, minute: 0 },
            updated_at: 0,
        };
        assert_eq!(task(TaskAction::Restart { warnings: vec![1, 10, 5], message: String::new() }).lead_s(), 600);
        assert_eq!(task(TaskAction::Restart { warnings: vec![], message: String::new() }).lead_s(), 0);
        assert_eq!(task(TaskAction::Snapshot { name: String::new() }).lead_s(), 0);
        assert_eq!(task(TaskAction::Snapshot { name: String::new() }).label(), "Snapshot programmé");
    }

    // --- Redémarrage automatique : délais et plafond horaire ----------------------------------

    #[test]
    fn delais_croissants_puis_abandon() {
        let now = 100_000;
        assert_eq!(plan_auto_restart(&[], now, 3), RestartDecision::Restart { delay_s: 30, attempt: 1 });
        assert_eq!(plan_auto_restart(&[now - 100], now, 3), RestartDecision::Restart { delay_s: 120, attempt: 2 });
        assert_eq!(
            plan_auto_restart(&[now - 900, now - 100], now, 3),
            RestartDecision::Restart { delay_s: 300, attempt: 3 }
        );
        assert_eq!(plan_auto_restart(&[now - 900, now - 500, now - 100], now, 3), RestartDecision::GiveUp);
        // Au-delà du 3e délai, le dernier se répète (plafond relevé).
        assert_eq!(
            plan_auto_restart(&[now - 3, now - 2, now - 1], now, 5),
            RestartDecision::Restart { delay_s: 300, attempt: 4 }
        );
    }

    #[test]
    fn la_fenetre_dune_heure_glisse() {
        let now = 100_000;
        // Trois redémarrages il y a plus d'une heure : on repart de zéro.
        let old = [now - 4000, now - 3800, now - 3600];
        assert_eq!(plan_auto_restart(&old, now, 3), RestartDecision::Restart { delay_s: 30, attempt: 1 });
        // Plafond borné à [1, MAX_RESTARTS_PER_HOUR].
        assert_eq!(plan_auto_restart(&[], now, 0), RestartDecision::Restart { delay_s: 30, attempt: 1 });
        assert_eq!(plan_auto_restart(&[now - 1], now, 0), RestartDecision::GiveUp);
        let many: Vec<i64> = (1..=10).map(|i| now - i).collect();
        assert_eq!(plan_auto_restart(&many, now, 50), RestartDecision::GiveUp);
    }

    // --- Validation ---------------------------------------------------------------------------

    fn base_task(action: TaskAction, schedule: Schedule) -> ScheduledTask {
        ScheduledTask {
            id: String::new(),
            account_id: 0,
            server_id: 9,
            name: "  Nuit  ".into(),
            enabled: true,
            action,
            schedule,
            updated_at: 0,
        }
    }

    #[test]
    fn normalisation_des_taches() {
        let t = normalize_task(base_task(
            TaskAction::Restart { warnings: vec![1, 10, 10, 0, 61, 5], message: "  bye  ".into() },
            Schedule::Weekly { days: vec![5, 1, 1, 9], hour: 4, minute: 30 },
        ))
        .unwrap();
        assert_eq!(t.name, "Nuit");
        assert_eq!(t.action, TaskAction::Restart { warnings: vec![10, 5, 1], message: "bye".into() });
        assert_eq!(t.schedule, Schedule::Weekly { days: vec![1, 5], hour: 4, minute: 30 });

        let t = normalize_task(base_task(
            TaskAction::Commands { commands: vec![" save-all ".into(), "".into(), "say ok".into()] },
            Schedule::Interval { every_minutes: 60 },
        ))
        .unwrap();
        assert_eq!(t.action, TaskAction::Commands { commands: vec!["save-all".into(), "say ok".into()] });
    }

    #[test]
    fn taches_refusees() {
        let daily = Schedule::Daily { hour: 4, minute: 0 };
        let cmds = |c: Vec<&str>| TaskAction::Commands { commands: c.into_iter().map(String::from).collect() };
        assert!(normalize_task(base_task(cmds(vec![]), daily.clone())).is_err());
        assert!(normalize_task(base_task(cmds(vec!["  "]), daily.clone())).is_err());
        assert!(normalize_task(base_task(cmds(vec!["a\nb"]), daily.clone())).is_err(), "injection de commande");
        assert!(normalize_task(base_task(cmds(vec!["x"; 21]), daily.clone())).is_err());
        let snap = TaskAction::Snapshot { name: String::new() };
        assert!(normalize_task(base_task(snap.clone(), Schedule::Daily { hour: 24, minute: 0 })).is_err());
        assert!(normalize_task(base_task(snap.clone(), Schedule::Daily { hour: 1, minute: 60 })).is_err());
        assert!(normalize_task(base_task(snap.clone(), Schedule::Weekly { days: vec![0, 8], hour: 1, minute: 0 })).is_err());
        assert!(normalize_task(base_task(snap.clone(), Schedule::Interval { every_minutes: 4 })).is_err());
        assert!(normalize_task(base_task(snap.clone(), Schedule::Interval { every_minutes: MAX_INTERVAL_MINUTES + 1 })).is_err());
        assert!(normalize_task(base_task(snap, Schedule::Interval { every_minutes: 5 })).is_ok());
    }

    // --- Sérialisation (IPC, fichier, front) --------------------------------------------------

    #[test]
    fn forme_json_des_taches() {
        let t = base_task(
            TaskAction::Restart { warnings: vec![5], message: "bye".into() },
            Schedule::Weekly { days: vec![1], hour: 4, minute: 0 },
        );
        let v = serde_json::to_value(&t).unwrap();
        assert_eq!(v["action"], serde_json::json!({ "type": "restart", "warnings": [5], "message": "bye" }));
        assert_eq!(v["schedule"], serde_json::json!({ "kind": "weekly", "days": [1], "hour": 4, "minute": 0 }));
        let back: ScheduledTask = serde_json::from_value(v).unwrap();
        assert_eq!(back, t);

        // Fichier d'une version future avec un champ inconnu, ou ancien sans `max_per_hour`.
        let cfg: AutomationConfig = serde_json::from_str(
            r#"{ "restart_rules": [{ "account_id": 1, "server_id": 2, "enabled": true }], "futur": 1 }"#,
        )
        .unwrap();
        assert_eq!(cfg.restart_rules[0].max_per_hour, DEFAULT_MAX_RESTARTS_PER_HOUR);
        assert!(cfg.tasks.is_empty());
    }

    #[test]
    fn identifiants_uniques() {
        let a = new_id("task");
        let b = new_id("task");
        assert!(a.starts_with("task-") && a != b);
    }

    #[test]
    fn decalage_horaire_local_coherent() {
        // Le fuseau réel de la machine sert en production : l'occurrence calculée doit retomber
        // sur l'heure murale demandée.
        let tz = chrono::Local;
        let now = now_secs();
        let next = next_occurrence(&Schedule::Daily { hour: 12, minute: 34 }, 0, now, &tz).unwrap();
        let local = tz.timestamp_opt(next, 0).unwrap();
        assert!(next > now && next <= now + 26 * 3600);
        assert_eq!(local.format("%M").to_string(), "34");
        let _ = local.offset().fix();
    }
}
