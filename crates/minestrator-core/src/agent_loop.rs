//! Boucle « agir puis vérifier » des agents MCP : commande avec sa sortie, attente d'un état ou
//! d'un motif, console lue par curseur, déploiement vérifié.
//!
//! **Tampon console en direct.** À la première demande pour un serveur, une connexion WebSocket
//! dédiée (`agent:<id>:<époque>`, distincte des onglets et du superviseur) alimente un tampon
//! circulaire de lignes numérotées ([`ConsoleBuffer`]). Les lignes que Wings rejoue à la connexion
//! (`send logs`) sont marquées historiques ; l'état du serveur suit les trames `status` et
//! `stats` ; une reconnexion est raccordée au tampon sans doublon (recouvrement des dernières
//! lignes) ou signalée comme un trou ; la connexion se ferme après dix minutes sans lecture, au
//! changement de compte, ou quand la clé change. Un curseur (`<époque>-<numéro>`) désigne la
//! dernière ligne lue : la lecture suivante rend les lignes d'après et compte celles que le tampon
//! a perdues entre-temps (`missed`). Faute de WebSocket, repli sur l'API REST (100 dernières
//! lignes, sans curseur), dit dans la sortie.
//!
//! **Envoi en REST, WebSocket en lecture seule** (invariant n° 4) : `run_command` envoie par
//! `PUT /server/{id}/command`, puis lit la réponse dans le tampon.
//!
//! Tout ce qui part vers le modèle passe par l'anonymisation IA (`redact`). Les motifs (`filter`,
//! `until`, `pattern`) s'appliquent au texte anonymisé : celui que voit l'agent.

use crate::console::ConsoleManager;
use crate::events::CoreEvent;
use crate::jar_meta::{self, norm_id, JarMeta, Platform, ZipProbe};
use crate::server_logs::{self, Digest, ErrorGroup, Level, Names, Window};
use crate::{mcp_ctx, sftp, Core, SftpEntry};
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, watch};

// --- Plafonds -------------------------------------------------------------------------------

/// Lignes gardées par serveur (de l'ordre du Mio au pire).
const BUFFER_LINES: usize = 5000;
/// Octets gardés d'une ligne : une ligne démesurée ne gonfle pas le tampon.
const LINE_MAX_BYTES: usize = 8 * 1024;
/// Octets d'une ligne renvoyée au modèle.
const OUT_LINE_BYTES: usize = 2000;
/// Volume de lignes renvoyé par appel ; au-delà, `more` et reprise au curseur.
const OUT_BUDGET: usize = 48 * 1024;
/// Lignes gardées en mémoire pendant la collecte d'une réponse.
const COLLECT_MAX: usize = 5000;
/// Fermeture de la connexion après cette durée sans lecture.
const IDLE_CLOSE_MS: i64 = 10 * 60 * 1000;
/// Rejeu de l'historique terminé : stats reçues puis ce silence…
const REPLAY_QUIET_MS: i64 = 300;
/// … ou, au plus tard, ce délai après l'ouverture.
const REPLAY_GRACE_MS: i64 = 3000;
/// Attente maximale d'une console prête (historique reçu) avant le repli REST.
const READY_WAIT: Duration = Duration::from_secs(8);
/// Pas des attentes (annulation, échéance, sondage).
const POLL: Duration = Duration::from_millis(250);
/// Sondage de l'état par l'API quand le WebSocket ne le donne pas.
const REST_POLL: Duration = Duration::from_secs(4);
/// Sans `until`, `run_command` rend la main après ce silence qui suit la première ligne.
const QUIET: Duration = Duration::from_millis(800);
/// Lignes encore attendues après le démarrage (erreurs juste après « Done »).
const SETTLE: Duration = Duration::from_secs(2);
/// Une action d'alimentation plus ancienne ne rend plus suspect un état « running ».
const MARK_WINDOW_MS: i64 = 120_000;
/// Marge sur l'heure de démarrage du processus (uptime à la seconde près).
const START_SLACK_MS: i64 = 3000;
/// Sondage d'une tâche de sauvegarde.
const JOB_POLL: Duration = Duration::from_secs(3);
/// Avancement signalé au client au plus toutes les…
const PROGRESS_EVERY: Duration = Duration::from_secs(5);
/// Lignes de console jointes au verdict d'un déploiement.
const DEPLOY_TAIL: usize = 40;
/// Groupes d'erreurs ou d'avertissements joints à un résumé.
const MAX_GROUPS: usize = 10;
/// Jars voisins examinés pour trouver une autre version du même plugin.
const MAX_SIBLINGS: usize = 300;

/// Délais en secondes : `(défaut, maximum, maximum pour le Copilote)`.
type Limits = (f64, f64, f64);
const TAIL_WAIT: Limits = (0.0, 60.0, 30.0);
const RUN_TIMEOUT: Limits = (5.0, 60.0, 60.0);
const WAIT_TIMEOUT: Limits = (60.0, 600.0, 120.0);
const DEPLOY_TIMEOUT: Limits = (180.0, 600.0, 600.0);

/// Délai demandé, borné : défaut si absent ou invalide, plafonné (plus bas pour le Copilote, qui
/// ne doit pas bloquer un diagnostic plusieurs minutes).
fn cap_secs(
    requested: Option<f64>,
    (default, max, copilot_max): Limits,
    copilot: bool,
) -> Duration {
    let limit = if copilot { max.min(copilot_max) } else { max };
    let v = requested
        .filter(|v| v.is_finite() && *v >= 0.0)
        .unwrap_or(default);
    Duration::from_secs_f64(v.min(limit))
}

/// Appel hors transport MCP : le Copilote (diagnostic, chat).
fn copilot_call() -> bool {
    mcp_ctx::session_id().is_none()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// État Wings (`running`…) ou de l'API (`online`…) ramené au vocabulaire des outils :
/// `running` | `starting` | `stopping` | `offline` (autre valeur gardée telle quelle).
pub(crate) fn normalize_state(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "running" | "online" => "running".into(),
        "offline" | "stopped" | "hibernated" => "offline".into(),
        "" => "unknown".into(),
        other => other.to_string(),
    }
}

/// Lignes propres d'une trame : couleurs ANSI retirées, retours chariot résolus (le dernier
/// segment l'emporte, comme dans un terminal), longueur bornée.
fn clean_lines(raw: &str) -> Vec<String> {
    raw.trim_end_matches(['\r', '\n'])
        .split('\n')
        .map(|l| {
            let l = l.trim_end_matches('\r');
            let l = l.rsplit('\r').find(|s| !s.is_empty()).unwrap_or(l);
            let l = if l.contains('\u{1b}') {
                crate::paste::strip_ansi(l)
            } else {
                l.to_string()
            };
            crate::util::truncate_on_boundary(&l, LINE_MAX_BYTES, "…")
        })
        .collect()
}

/// Anonymisation IA, lue une fois par appel (au lieu d'une lecture des réglages par ligne).
#[derive(Debug, Clone, Copy)]
struct Redactor(bool);

impl Redactor {
    fn apply(self, s: &str) -> String {
        if self.0 {
            crate::redact::redact(s)
        } else {
            s.to_string()
        }
    }
}

// --- Curseurs -------------------------------------------------------------------------------

/// Curseur rendu par un outil : `<époque>-<numéro>`. L'époque identifie le tampon (recréé après
/// une fermeture) ; un nombre seul désigne une ligne du tampon courant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub epoch: Option<String>,
    pub seq: u64,
}

const CURSOR_HELP: &str =
    "curseur invalide : reprendre la valeur `cursor` rendue par l'appel précédent, telle quelle";

pub(crate) fn parse_cursor(v: Option<&Value>) -> Result<Option<Cursor>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => n
            .as_u64()
            .map(|seq| Some(Cursor { epoch: None, seq }))
            .ok_or_else(|| CURSOR_HELP.into()),
        Some(Value::String(s)) => {
            let s = s.trim();
            if s.is_empty() {
                return Ok(None);
            }
            let (epoch, seq) = match s.rsplit_once('-') {
                Some((e, n)) if !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) => {
                    (Some(e.to_string()), n)
                }
                Some(_) => return Err(CURSOR_HELP.into()),
                None => (None, s),
            };
            seq.parse()
                .map(|seq| Some(Cursor { epoch, seq }))
                .map_err(|_| CURSOR_HELP.into())
        }
        Some(_) => Err(CURSOR_HELP.into()),
    }
}

fn format_cursor(epoch: &str, seq: u64) -> String {
    format!("{epoch}-{seq}")
}

/// Point de départ d'une lecture.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Start {
    /// Aucun curseur : les dernières lignes.
    Latest,
    /// Les lignes après ce numéro.
    After(u64),
    /// Curseur d'un tampon précédent (connexion rouverte depuis) : les dernières lignes, signalé.
    Reset,
}

pub(crate) fn resolve_cursor(
    c: Option<&Cursor>,
    epoch: &str,
    last_seq: u64,
) -> Result<Start, String> {
    let Some(c) = c else { return Ok(Start::Latest) };
    if c.epoch.as_deref().is_some_and(|e| e != epoch) {
        return Ok(Start::Reset);
    }
    if c.seq > last_seq {
        return Err(format!(
            "{CURSOR_HELP} (ligne {} jamais émise ; dernière : {last_seq})",
            c.seq
        ));
    }
    Ok(Start::After(c.seq))
}

fn new_epoch() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{}{}", base36(now_ms().max(0) as u64), base36(n))
}

fn base36(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// --- Motifs ---------------------------------------------------------------------------------

/// Motif d'un outil (`filter`, `until`, `pattern`) : texte littéral, ou expression régulière avec
/// `regex`. Casse ignorée ; mêmes garde-fous que `search_logs` (taille, temps linéaire).
pub(crate) fn compile_pattern(text: &str, regex: bool, key: &str) -> Result<Regex, String> {
    if text.trim().is_empty() {
        return Err(format!("paramètre `{key}` vide"));
    }
    server_logs::compile_query(text, regex, false, &Names::default())
        .map(|(re, _)| re)
        .map_err(|e| format!("`{key}` : {e}"))
}

// --- Tampon (pur) ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Line {
    pub seq: u64,
    pub text: String,
    /// Rejouée par Wings à la connexion : antérieure à la première lecture de l'agent.
    pub history: bool,
}

/// Changement d'état observé.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StateChange {
    /// Dernière ligne reçue avant le changement : celles du nouvel état suivent ce numéro.
    pub after_seq: u64,
    pub state: String,
    /// Heure de réception (ms Unix).
    pub at_ms: i64,
}

/// Lignes peut-être perdues après la ligne `after_seq` (reconnexion sans raccord, bus saturé).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Gap {
    pub after_seq: u64,
    pub reason: String,
}

/// Rejeu de l'historique en cours (connexion ou reconnexion).
#[derive(Debug)]
struct Replay {
    lines: Vec<String>,
    opened_ms: i64,
    /// Dernière activité (ligne ou stats) depuis l'ouverture.
    last_ms: i64,
    /// Stats reçues : Wings a traité nos demandes (`send logs`, puis `send stats`).
    stats: bool,
    /// État connu avant cette connexion (raccord d'une reconnexion).
    state_before: Option<String>,
}

/// Lecture du tampon depuis un curseur.
#[derive(Debug, Default)]
pub(crate) struct Slice {
    pub lines: Vec<Line>,
    /// Lignes sorties du tampon (débordement) entre le curseur et la première ligne rendue.
    pub missed: u64,
    pub gaps: Vec<Gap>,
    pub last_seq: u64,
}

const KEEP_CHANGES: usize = 64;
const KEEP_GAPS: usize = 64;
/// Lignes comparées pour raccorder une reconnexion.
const OVERLAP_WINDOW: usize = 20;

/// Tampon circulaire de la console d'un serveur, alimenté par les events de sa connexion.
/// Indépendant de toute E/S : l'heure est passée par l'appelant.
#[derive(Debug)]
pub(crate) struct ConsoleBuffer {
    lines: VecDeque<Line>,
    capacity: usize,
    next_seq: u64,
    state: Option<String>,
    changes: VecDeque<StateChange>,
    phase: String,
    hibernated: bool,
    replay: Option<Replay>,
    connected_once: bool,
    ready: bool,
    seeded: bool,
    gaps: VecDeque<Gap>,
    /// Démarrage du processus (ms Unix), d'après l'uptime des stats Wings.
    process_start_ms: Option<i64>,
    /// Connexion impossible pour de bon (serveur suspendu ou expiré).
    terminal: Option<String>,
}

impl ConsoleBuffer {
    pub(crate) fn new(capacity: usize) -> Self {
        ConsoleBuffer {
            lines: VecDeque::new(),
            capacity: capacity.max(1),
            next_seq: 1,
            state: None,
            changes: VecDeque::new(),
            phase: String::new(),
            hibernated: false,
            replay: None,
            connected_once: false,
            ready: false,
            seeded: false,
            gaps: VecDeque::new(),
            process_start_ms: None,
            terminal: None,
        }
    }

    pub(crate) fn last_seq(&self) -> u64 {
        self.next_seq - 1
    }

    fn first_seq(&self) -> u64 {
        self.lines.front().map_or(self.next_seq, |l| l.seq)
    }

    pub(crate) fn ready(&self) -> bool {
        self.ready
    }

    pub(crate) fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }

    /// État fiable : connexion ouverte et historique reçu, ou serveur hiberné (éteint).
    pub(crate) fn live_state(&self) -> Option<&str> {
        if self.hibernated {
            return Some("offline");
        }
        if self.ready && self.phase == "open" {
            self.state.as_deref()
        } else {
            None
        }
    }

    fn replaying(&self) -> bool {
        self.replay.is_some()
    }

    fn push(&mut self, text: String, history: bool) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.lines.push_back(Line { seq, text, history });
        while self.lines.len() > self.capacity {
            self.lines.pop_front();
        }
    }

    fn gap(&mut self, reason: String) {
        self.gaps.push_back(Gap {
            after_seq: self.last_seq(),
            reason,
        });
        while self.gaps.len() > KEEP_GAPS {
            self.gaps.pop_front();
        }
    }

    fn set_state(&mut self, state: &str, now_ms: i64) {
        let state = normalize_state(state);
        if self.state.as_deref() == Some(state.as_str()) {
            return;
        }
        self.changes.push_back(StateChange {
            after_seq: self.last_seq(),
            state: state.clone(),
            at_ms: now_ms,
        });
        while self.changes.len() > KEEP_CHANGES {
            self.changes.pop_front();
        }
        self.state = Some(state);
    }

    /// Ligne(s) de console reçue(s).
    pub(crate) fn on_line(&mut self, raw: &str, now_ms: i64) {
        for text in clean_lines(raw) {
            match self.replay.as_mut() {
                Some(r) => {
                    if r.lines.len() >= self.capacity {
                        r.lines.remove(0);
                    }
                    r.lines.push(text);
                    r.last_ms = now_ms;
                }
                None => self.push(text, false),
            }
        }
    }

    /// Phase de la connexion (`connecting`, `open`, `reconnecting`, `closed`, `hibernated`,
    /// `suspended`).
    pub(crate) fn on_phase(&mut self, phase: &str, now_ms: i64) {
        match phase {
            "open" => {
                // Un rejeu interrompu par une coupure reste valable.
                self.finish_replay();
                self.hibernated = false;
                self.replay = Some(Replay {
                    lines: Vec::new(),
                    opened_ms: now_ms,
                    last_ms: now_ms,
                    stats: false,
                    state_before: self.state.clone(),
                });
            }
            "hibernated" => {
                self.finish_replay();
                self.hibernated = true;
                self.ready = true;
                self.set_state("offline", now_ms);
            }
            "suspended" => {
                self.finish_replay();
                self.terminal = Some("serveur suspendu ou expiré".into());
            }
            "reconnecting" | "closed" => self.finish_replay(),
            _ => {}
        }
        self.phase = phase.to_string();
    }

    pub(crate) fn on_status(&mut self, state: &str, now_ms: i64) {
        if !state.trim().is_empty() {
            self.set_state(state, now_ms);
        }
    }

    pub(crate) fn on_stats(&mut self, state: &str, uptime_ms: u64, now_ms: i64) {
        self.on_status(state, now_ms);
        let uptime = i64::try_from(uptime_ms).unwrap_or(0);
        self.process_start_ms = (uptime > 0).then_some(now_ms.saturating_sub(uptime));
        if let Some(r) = self.replay.as_mut() {
            r.stats = true;
            r.last_ms = now_ms;
        }
    }

    /// Events perdus par l'abonnement au bus (débit trop élevé).
    pub(crate) fn on_lag(&mut self, n: u64) {
        self.gap(format!(
            "{n} événement(s) de console perdus (débit trop élevé) : des lignes ont pu manquer"
        ));
    }

    /// Termine le rejeu quand il est dû. Renvoie `true` si le tampon a changé.
    pub(crate) fn on_tick(&mut self, now_ms: i64) -> bool {
        let due = self.replay.as_ref().is_some_and(|r| {
            (r.stats && now_ms - r.last_ms >= REPLAY_QUIET_MS)
                || now_ms - r.opened_ms >= REPLAY_GRACE_MS
        });
        if due {
            self.finish_replay();
        }
        due
    }

    fn finish_replay(&mut self) {
        let Some(r) = self.replay.take() else { return };
        self.ready = true;
        if !self.connected_once {
            self.connected_once = true;
            for l in r.lines {
                self.push(l, true);
            }
            return;
        }
        // Reconnexion : Wings rejoue ses dernières lignes, dont celles déjà reçues.
        let n = self.lines.len().min(OVERLAP_WINDOW);
        let tail: Vec<&str> = self
            .lines
            .iter()
            .skip(self.lines.len() - n)
            .map(|l| l.text.as_str())
            .collect();
        let start = if tail.is_empty() {
            Some(0)
        } else {
            new_lines_start(&tail, &r.lines)
        };
        match start {
            Some(i) => {
                for l in r.lines.into_iter().skip(i) {
                    self.push(l, false);
                }
            }
            None => {
                // Rien de rejoué (serveur éteint) alors qu'il tournait : sa fin a pu être perdue.
                let quiet =
                    r.lines.is_empty() && r.state_before.as_deref().is_none_or(|s| s == "offline");
                if !quiet {
                    self.gap("reconnexion de la console sans raccord : des lignes émises pendant la coupure ont pu manquer".into());
                }
                for l in r.lines {
                    self.push(l, false);
                }
            }
        }
    }

    /// Lignes lues par l'API faute de rejeu (serveur éteint : Wings ne rejoue rien), une fois.
    pub(crate) fn needs_seed(&self) -> bool {
        self.ready
            && !self.seeded
            && self.lines.is_empty()
            && self.replay.is_none()
            && !self.hibernated
    }

    pub(crate) fn seed_history(&mut self, lines: Vec<String>) {
        if self.needs_seed() {
            for raw in lines {
                for l in clean_lines(&raw) {
                    self.push(l, true);
                }
            }
        }
        self.seeded = true;
    }

    /// Pourquoi la console en direct n'est pas exploitable (`None` : elle l'est).
    pub(crate) fn unusable(&self, ready: bool) -> Option<String> {
        if let Some(t) = &self.terminal {
            return Some(t.clone());
        }
        if !ready {
            let phase = if self.phase.is_empty() {
                "en attente"
            } else {
                self.phase.as_str()
            };
            return Some(format!("connexion WebSocket non établie (phase : {phase})"));
        }
        if self.hibernated || self.state.is_some() || !self.lines.is_empty() {
            None
        } else {
            Some("aucune réponse de la console".into())
        }
    }

    /// Lignes après `after` (toutes sans curseur).
    pub(crate) fn since(&self, after: Option<u64>) -> Result<Slice, String> {
        let last = self.last_seq();
        let first = self.first_seq();
        let from = match after {
            Some(a) if a > last => {
                return Err(format!(
                    "{CURSOR_HELP} (ligne {a} jamais émise ; dernière : {last})"
                ))
            }
            Some(a) => a,
            None => first - 1,
        };
        let missed = (first - 1).saturating_sub(from);
        let skip = (from + 1).saturating_sub(first) as usize;
        Ok(Slice {
            lines: self.lines.iter().skip(skip).cloned().collect(),
            missed,
            gaps: self
                .gaps
                .iter()
                .filter(|g| g.after_seq >= from)
                .cloned()
                .collect(),
            last_seq: last,
        })
    }

    /// Changements d'état reçus depuis `since_ms`.
    pub(crate) fn changes_since(&self, since_ms: i64) -> Vec<StateChange> {
        self.changes
            .iter()
            .filter(|c| c.at_ms >= since_ms)
            .cloned()
            .collect()
    }
}

/// Raccord d'une reconnexion : indice, dans les lignes rejouées, de la première ligne nouvelle.
/// `old_tail` : dernières lignes déjà reçues (ordre chronologique). On cherche, de la fin vers le
/// début, la position où le rejeu reprend ces lignes (fenêtre de 20 lignes, 3 au moins quand on
/// en a autant) ; `None` si aucun recouvrement (coupure plus longue que le rejeu).
pub(crate) fn new_lines_start(old_tail: &[&str], replay: &[String]) -> Option<usize> {
    let t = old_tail.len();
    if t == 0 {
        return Some(0);
    }
    let min_overlap = t.min(3);
    (0..replay.len()).rev().find_map(|j| {
        let l = (j + 1).min(t).min(OVERLAP_WINDOW);
        if l < min_overlap {
            return None;
        }
        let same = replay[j + 1 - l..=j]
            .iter()
            .map(String::as_str)
            .eq(old_tail[t - l..].iter().copied());
        same.then_some(j + 1)
    })
}

// --- Sélection des lignes renvoyées ---------------------------------------------------------

/// Lignes renvoyées au modèle, dans l'ordre : anonymisées, filtrées, coupées, dans un budget.
struct Picker<'a> {
    filter: Option<&'a Regex>,
    max: usize,
    used: usize,
    redact: Redactor,
    lines: Vec<String>,
    history: usize,
    /// Dernière ligne consommée (rendue, ou écartée par le filtre).
    cursor: u64,
    /// Arrêt sur le nombre de lignes ou le budget : la suite reste à lire.
    truncated: bool,
}

impl<'a> Picker<'a> {
    fn new(filter: Option<&'a Regex>, max: usize, redact: Redactor, cursor: u64) -> Self {
        Picker {
            filter,
            max,
            used: 0,
            redact,
            lines: Vec::new(),
            history: 0,
            cursor,
            truncated: false,
        }
    }

    fn feed(&mut self, lines: &[Line]) {
        for l in lines {
            if self.truncated {
                return;
            }
            let text = self.redact.apply(&l.text);
            if self.filter.is_some_and(|f| !f.is_match(&text)) {
                self.cursor = l.seq;
                continue;
            }
            let text = crate::util::truncate_on_boundary(&text, OUT_LINE_BYTES, "…");
            if self.lines.len() >= self.max
                || (!self.lines.is_empty() && self.used + text.len() > OUT_BUDGET)
            {
                self.truncated = true;
                return;
            }
            self.used += text.len();
            self.history += usize::from(l.history);
            self.lines.push(text);
            self.cursor = l.seq;
        }
    }
}

/// Les `max` dernières lignes (filtrées) dans le budget. Renvoie `(lignes, historiques, plus
/// anciennes omises)`.
fn pick_latest(
    lines: &[Line],
    filter: Option<&Regex>,
    max: usize,
    redact: Redactor,
) -> (Vec<String>, usize, bool) {
    let mut out = Vec::new();
    let mut history = 0;
    let mut used = 0;
    let mut omitted = false;
    for l in lines.iter().rev() {
        let text = redact.apply(&l.text);
        if filter.is_some_and(|f| !f.is_match(&text)) {
            continue;
        }
        let text = crate::util::truncate_on_boundary(&text, OUT_LINE_BYTES, "…");
        if out.len() >= max || (!out.is_empty() && used + text.len() > OUT_BUDGET) {
            omitted = true;
            break;
        }
        used += text.len();
        history += usize::from(l.history);
        out.push(text);
    }
    out.reverse();
    (out, history, omitted)
}

// --- Verdicts d'état (pur) ------------------------------------------------------------------

/// Ce que l'on sait de l'état du serveur pendant une attente.
pub(crate) struct Observation<'a> {
    pub state: &'a str,
    /// État au début de l'attente.
    pub initial: &'a str,
    /// Changements observés (WebSocket, ou sondages de l'API).
    pub changes: &'a [StateChange],
    /// Démarrage du processus (ms Unix), s'il est connu.
    pub process_start_ms: Option<i64>,
    /// Arrêt ou redémarrage demandé récemment (ms Unix).
    pub mark_ms: Option<i64>,
    /// Début de l'attente (ms Unix).
    pub since_ms: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Judgement {
    Reached,
    Pending,
    Failed(&'static str),
}

/// L'état attendu est-il atteint ? Un « running » lu juste après un redémarrage demandé est celui
/// d'avant : il ne compte qu'après un passage par un autre état, ou si le processus a démarré
/// après la demande. Un arrêt pendant le démarrage (`starting` → `offline`, sans `stopping`) est
/// un échec immédiat plutôt qu'une attente jusqu'au délai.
pub(crate) fn judge_state(target: &str, o: &Observation) -> Judgement {
    let from = o.mark_ms.map_or(o.since_ms, |m| m.min(o.since_ms));
    let recent = || o.changes.iter().filter(move |c| c.at_ms >= from);
    match target {
        "running" => {
            if o.state == "running" {
                let fresh = match o.mark_ms {
                    None => true,
                    Some(m) => {
                        recent().any(|c| c.state != "running")
                            || o.process_start_ms.is_some_and(|p| p >= m - START_SLACK_MS)
                    }
                };
                return if fresh {
                    Judgement::Reached
                } else {
                    Judgement::Pending
                };
            }
            // Démarrage en cours (au début de l'attente, ou vu depuis) sans arrêt demandé ensuite.
            let mut starting = o.initial == "starting";
            for c in recent() {
                match c.state.as_str() {
                    "starting" => starting = true,
                    "stopping" => starting = false,
                    _ => {}
                }
            }
            if o.state == "offline" && starting {
                Judgement::Failed(
                    "le serveur s'est arrêté pendant son démarrage (plantage probable)",
                )
            } else {
                Judgement::Pending
            }
        }
        "offline" => {
            if o.state == "offline" {
                Judgement::Reached
            } else {
                Judgement::Pending
            }
        }
        other => {
            if o.state == other || recent().any(|c| c.state == other) {
                Judgement::Reached
            } else {
                Judgement::Pending
            }
        }
    }
}

// --- Analyse du démarrage et du déploiement (pur) -------------------------------------------

static DONE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Done \((\d+(?:[.,]\d+)?)\s*s\)!").expect("motif « Done » valide")
});

/// Résumé d'un démarrage d'après ses lignes de console.
pub(crate) struct Startup {
    pub done_line: Option<String>,
    /// Durée annoncée par le serveur (« Done (12.3s)! »).
    pub reported_secs: Option<f64>,
    pub errors: Vec<ErrorGroup>,
    pub warnings: Vec<ErrorGroup>,
}

/// Erreurs et avertissements regroupés par signature (même regroupement que `error_digest`) et
/// ligne « Done ».
pub(crate) fn startup_digest(lines: &[String]) -> Startup {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let dates = vec![None; refs.len()];
    let mut names = Names::default();
    for l in &refs {
        names.discover(server_logs::parse_header(l).message);
    }
    let mut dg = Digest::default();
    dg.add(
        &server_logs::records(&refs, &dates),
        "console",
        Level::Warn,
        &Window::default(),
        &names,
    );
    let (errors, warnings): (Vec<ErrorGroup>, Vec<ErrorGroup>) = dg
        .finish()
        .into_iter()
        .partition(|g| g.severity >= Level::Error);
    let done = lines.iter().rev().find_map(|l| {
        DONE_RE
            .captures(l)
            .map(|c| (l.clone(), c[1].replace(',', ".").parse::<f64>().ok()))
    });
    Startup {
        reported_secs: done.as_ref().and_then(|d| d.1),
        done_line: done.map(|d| d.0),
        errors,
        warnings,
    }
}

/// Le plugin ou mod déployé, d'après son descripteur.
#[derive(Debug, Clone)]
pub(crate) struct Identity {
    /// Nom affiché (`name` de plugin.yml, `name`/`displayName` d'un mod).
    pub name: String,
    /// Identifiant de chargement (nom Bukkit, id Fabric, modId Forge).
    pub id: String,
    pub version: String,
    pub platform: Option<Platform>,
    /// Nom du jar envoyé.
    pub file: String,
}

/// Lettres et chiffres seulement, en minuscules : `My-Plugin` et `my_plugin` se rejoignent.
fn squash(s: &str) -> String {
    s.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `needle` présent comme mot (bornes non alphanumériques) dans `hay`.
fn contains_token(hay: &str, needle: &str) -> bool {
    !needle.is_empty()
        && hay.match_indices(needle).any(|(i, _)| {
            let before = hay[..i].chars().next_back();
            let after = hay[i + needle.len()..].chars().next();
            before.is_none_or(|c| !c.is_alphanumeric())
                && after.is_none_or(|c| !c.is_alphanumeric())
        })
}

fn norm_version(v: &str) -> String {
    v.split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches(['v', 'V'])
        .trim_end_matches('*')
        .to_string()
}

impl Identity {
    fn keys(&self) -> Vec<String> {
        let mut k: Vec<String> = [
            self.name.as_str(),
            self.id.as_str(),
            &jar_meta::nested_id(&self.file),
        ]
        .iter()
        .map(|s| squash(s))
        .filter(|s| s.len() >= 3)
        .collect();
        k.sort();
        k.dedup();
        k
    }

    fn named(&self, n: &str) -> bool {
        let n = norm_id(n);
        !n.is_empty() && (n == norm_id(&self.name) || n == norm_id(&self.id))
    }

    /// Le serveur annonce-t-il l'activation de chaque élément ? (Bukkit, Paper, proxys : oui ;
    /// mods : un mod qui ne charge pas arrête le serveur.)
    fn announced(&self) -> bool {
        self.platform.is_none_or(Platform::is_plugin)
    }
}

/// Le nom d'une source probable désigne-t-il ce plugin ?
fn key_match(candidate: &str, keys: &[String]) -> bool {
    let c = squash(candidate);
    !c.is_empty()
        && keys
            .iter()
            .any(|k| c == *k || (k.len() >= 4 && c.contains(k.as_str())))
}

/// Un texte (en minuscules) cite-t-il ce plugin : nom du jar, `[Nom]`, nom ou id comme mot ?
fn mentions(text: &str, ident: &Identity) -> bool {
    let file = ident.file.to_lowercase();
    if file.len() > 4 && text.contains(&file) {
        return true;
    }
    [&ident.name, &ident.id]
        .into_iter()
        .map(|n| n.to_lowercase())
        .filter(|n| n.len() >= 4)
        .any(|n| contains_token(text, &n) || text.contains(&format!("[{n}]")))
}

/// Un groupe d'erreurs concerne-t-il ce plugin ? Sa source probable d'abord ; un indice sûr qui
/// en désigne un autre l'écarte ; sinon, le texte qui le cite.
fn concerns(ident: &Identity, g: &ErrorGroup) -> bool {
    if let Some(c) = &g.probable_source {
        if key_match(&c.name, &ident.keys()) {
            return true;
        }
        if matches!(c.evidence, "event" | "enable" | "load" | "task" | "mod") {
            return false;
        }
    }
    mentions(
        &format!("{}\n{}", g.signature, g.example).to_lowercase(),
        ident,
    )
}

/// Ligne de chargement qui concerne le plugin.
#[derive(Debug, PartialEq)]
enum Marker {
    Enabled(Option<String>),
    Disabled,
    Failed,
    Ambiguous,
}

/// `MyPlugin v1.2.3` → (`MyPlugin`, `1.2.3`).
fn split_version(rest: &str) -> (&str, Option<String>) {
    match rest.rfind(" v") {
        Some(i) if !rest[i + 2..].trim().is_empty() => {
            (&rest[..i], Some(norm_version(&rest[i + 2..])))
        }
        _ => (rest.trim(), None),
    }
}

/// Préfixe `[NomDuPlugin] ` d'un message retiré.
fn untag(msg: &str) -> &str {
    msg.strip_prefix('[')
        .and_then(|r| r.split_once("] "))
        .map_or(msg, |(_, after)| after.trim_start())
}

fn marker(line: &str, ident: &Identity) -> Option<Marker> {
    let msg = untag(server_logs::parse_header(line).message);
    // Bukkit, Spigot, Paper : « [X] Enabling X v1.0 » (Spigot ajoute `*` sans api-version).
    if let Some(rest) = msg.strip_prefix("Enabling ") {
        let (name, version) = split_version(rest);
        if ident.named(name) {
            return Some(Marker::Enabled(version));
        }
    }
    if let Some(rest) = msg.strip_prefix("Disabling ") {
        if ident.named(split_version(rest).0) {
            return Some(Marker::Disabled);
        }
    }
    // BungeeCord : « Enabled plugin X version 1.0 by Auteur ».
    if let Some((name, after)) = msg
        .strip_prefix("Enabled plugin ")
        .and_then(|r| r.split_once(" version "))
    {
        if ident.named(name) {
            return Some(Marker::Enabled(
                after.split_whitespace().next().map(norm_version),
            ));
        }
    }
    // Velocity : « Loaded plugin id 1.0 by Auteur ».
    if let Some(rest) = msg.strip_prefix("Loaded plugin ") {
        let mut it = rest.split_whitespace();
        if it.next().is_some_and(|n| ident.named(n)) {
            return Some(Marker::Enabled(it.next().map(norm_version)));
        }
    }
    // Fabric : liste « Loading N mods: » puis « - id version » (« |-- » pour les jars inclus).
    if matches!(ident.platform, Some(Platform::Fabric | Platform::Quilt)) {
        let t = line.trim_start();
        if let Some(item) = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("|-- "))
            .or_else(|| t.strip_prefix("\\-- "))
        {
            let mut it = item.split_whitespace();
            if it
                .next()
                .is_some_and(|id| id.eq_ignore_ascii_case(&ident.id))
            {
                return Some(Marker::Enabled(it.next().map(norm_version)));
            }
        }
        // Quilt : tableau « | Index | Mod | ID | Version | … ».
        if t.starts_with('|')
            && t.split('|')
                .any(|cell| cell.trim().eq_ignore_ascii_case(&ident.id))
        {
            return Some(Marker::Enabled(None));
        }
    }
    // Échecs : chargement refusé, activation en erreur, deux jars pour un même plugin.
    for m in [
        "Error occurred while enabling ",
        "Error occurred while loading ",
    ] {
        if let Some((_, rest)) = line.split_once(m) {
            if ident.named(split_version(rest).0) {
                return Some(Marker::Failed);
            }
        }
    }
    if line.contains("Could not load") && !ident.file.is_empty() && line.contains(&ident.file) {
        return Some(Marker::Failed);
    }
    let lower = line.to_lowercase();
    if lower.contains("ambiguous plugin name")
        && !ident.name.is_empty()
        && lower.contains(&ident.name.to_lowercase())
    {
        return Some(Marker::Ambiguous);
    }
    None
}

/// Constat d'un déploiement d'après les lignes du démarrage.
#[derive(Debug, Default)]
pub(crate) struct DeployCheck {
    pub enabled: Option<bool>,
    pub evidence: Option<String>,
    /// Activation déduite (mods : pas d'annonce par mod, mais un échec arrête le serveur).
    pub deduced: bool,
    pub loaded_version: Option<String>,
    /// Ligne « Ambiguous plugin name » : deux jars du même plugin.
    pub duplicate: Option<String>,
    pub errors: Vec<ErrorGroup>,
    pub warnings: Vec<ErrorGroup>,
    pub other_errors: Vec<ErrorGroup>,
    pub done_line: Option<String>,
    pub reported_secs: Option<f64>,
}

impl DeployCheck {
    /// Version chargée différente de celle envoyée (un autre jar a été chargé à sa place).
    fn version_mismatch(&self, ident: &Identity) -> Option<String> {
        let loaded = self.loaded_version.as_deref()?;
        let sent = norm_version(&ident.version);
        (!sent.is_empty() && !loaded.is_empty() && loaded != sent).then(|| loaded.to_string())
    }

    /// Verdict : démarré, activé (ou déduit), sans erreur qui le concerne, sans doublon.
    fn ok(&self, ident: &Identity, reached: bool, known: bool) -> bool {
        reached
            && self.enabled != Some(false)
            && (self.enabled == Some(true) || !known)
            && self.errors.is_empty()
            && self.duplicate.is_none()
            && self.version_mismatch(ident).is_none()
    }
}

/// Analyse les lignes d'un démarrage pour le plugin `ident`. `reached` : le serveur a atteint
/// `running`.
pub(crate) fn check_deploy(lines: &[String], ident: &Identity, reached: bool) -> DeployCheck {
    let startup = startup_digest(lines);
    let mut c = DeployCheck {
        done_line: startup.done_line,
        reported_secs: startup.reported_secs,
        ..Default::default()
    };
    for g in startup.errors {
        if concerns(ident, &g) {
            c.errors.push(g);
        } else {
            c.other_errors.push(g);
        }
    }
    c.warnings = startup
        .warnings
        .into_iter()
        .filter(|g| concerns(ident, g))
        .collect();
    let mut enabled_seen = false;
    for line in lines {
        match marker(line, ident) {
            Some(Marker::Enabled(version)) => {
                enabled_seen = true;
                if c.enabled != Some(false) {
                    c.enabled = Some(true);
                    c.evidence = Some(line.clone());
                }
                if version.is_some() {
                    c.loaded_version = version;
                }
            }
            Some(Marker::Disabled) if enabled_seen => {
                c.enabled = Some(false);
                c.evidence = Some(line.clone());
            }
            Some(Marker::Failed) => {
                c.enabled = Some(false);
                c.evidence = Some(line.clone());
            }
            Some(Marker::Ambiguous) => c.duplicate = Some(line.clone()),
            _ => {}
        }
    }
    if c.enabled.is_none() && !ident.announced() {
        if reached && c.errors.is_empty() {
            c.enabled = Some(true);
            c.deduced = true;
        } else if !c.errors.is_empty() {
            c.enabled = Some(false);
        }
    }
    c
}

fn group_json(g: &ErrorGroup, r: Redactor) -> Value {
    let mut v = json!({
        "level": g.level,
        "count": g.count,
        "signature": r.apply(&g.signature),
        "example": r.apply(&g.example),
    });
    if let Some(c) = &g.probable_source {
        v["probable_source"] = json!(c.name);
    }
    v
}

fn groups_json(groups: &[ErrorGroup], r: Redactor) -> Value {
    Value::Array(
        groups
            .iter()
            .take(MAX_GROUPS)
            .map(|g| group_json(g, r))
            .collect(),
    )
}

// --- Jars (descripteurs) ----------------------------------------------------------------------

/// Descripteurs d'un jar en mémoire.
pub(crate) fn jar_meta_of(bytes: &[u8]) -> Result<JarMeta, String> {
    let len = bytes.len() as u64;
    let mut probe = ZipProbe::new(len);
    for _ in 0..32 {
        match probe.next_read() {
            Ok(Some((offset, n))) => {
                let end = offset.saturating_add(n).min(len) as usize;
                let start = (offset.min(len) as usize).min(end);
                probe.feed(offset, bytes[start..end].to_vec());
            }
            Ok(None) => {
                return probe
                    .finish()
                    .map(|f| jar_meta::read_meta(&f))
                    .map_err(|e| e.to_string())
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("lecture du jar interrompue (trop d'étapes)".into())
}

/// Descripteurs d'un jar distant, lus à la demande (fin d'archive, répertoire, descripteurs).
/// `None` si illisible ; `Err` seulement si la session SFTP est morte.
async fn remote_jar_meta(
    conn: &sftp::SftpConn,
    path: &str,
    size: u64,
) -> crate::Result<Option<JarMeta>> {
    let mut probe = ZipProbe::new(size);
    let mut file: Option<sftp::RemoteFile> = None;
    for _ in 0..16 {
        let (offset, len) = match probe.next_read() {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(probe.finish().ok().map(|f| jar_meta::read_meta(&f))),
            Err(_) => return Ok(None),
        };
        if file.is_none() {
            match sftp::open_read(conn, path).await {
                Ok(f) => file = Some(f),
                Err(e) if sftp::is_session_dead(&e) => return Err(e),
                Err(_) => return Ok(None),
            }
        }
        let Some(f) = file.as_mut() else {
            return Ok(None);
        };
        match f.read_at(offset, len as usize).await {
            Ok(bytes) if bytes.is_empty() => return Ok(None),
            Ok(bytes) => probe.feed(offset, bytes),
            Err(e) if sftp::is_session_dead(&e) => return Err(e),
            Err(_) => return Ok(None),
        }
    }
    Ok(None)
}

/// Descripteur retenu pour le déploiement : celui d'une plateforme que le serveur charge, sinon
/// celui du dossier visé, sinon le premier.
fn pick_descriptor<'a>(
    meta: &'a JarMeta,
    accepted: Option<&std::collections::BTreeSet<Platform>>,
    folder_is_plugins: Option<bool>,
) -> Option<&'a jar_meta::Descriptor> {
    let d = &meta.descriptors;
    accepted
        .and_then(|acc| d.iter().find(|x| acc.contains(&x.platform)))
        .or_else(|| folder_is_plugins.and_then(|p| d.iter().find(|x| x.platform.is_plugin() == p)))
        .or_else(|| d.first())
}

/// Dossier distant et nom de fichier : `remote_path` tel quel, ou un dossier (finissant par `/`)
/// complété du nom du jar, ou le dossier par défaut.
fn resolve_remote(remote: Option<&str>, file: &str, default_dir: &str) -> String {
    match remote.map(str::trim).filter(|r| !r.is_empty()) {
        Some(r) => {
            let r = if r.starts_with('/') {
                r.to_string()
            } else {
                format!("/{r}")
            };
            if r.ends_with('/') {
                format!("{r}{file}")
            } else {
                r
            }
        }
        None => format!("{}/{file}", default_dir.trim_end_matches('/')),
    }
}

/// Nom libre pour mettre un jar de côté : `X.jar.old`, sinon `X.jar.2.old`…
fn old_name(name: &str, taken: &[String]) -> String {
    let free = |n: &String| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    std::iter::once(format!("{name}.old"))
        .chain((2..100).map(|k| format!("{name}.{k}.old")))
        .find(free)
        .unwrap_or_else(|| format!("{name}.{}.old", now_ms()))
}

// --- Surveillance (WebSocket) ---------------------------------------------------------------

/// Console d'un serveur suivie pour les agents : connexion dédiée et son tampon.
pub(crate) struct Watcher {
    server_id: i64,
    conn_id: String,
    epoch: String,
    /// Empreinte de la clé API de la connexion : une autre clé (changement de compte) la rouvre.
    token_fp: u64,
    buf: Mutex<ConsoleBuffer>,
    /// Compteur de changements : réveille les attentes (lignes, état, phase).
    changes: watch::Sender<u64>,
    last_use_ms: AtomicI64,
    alive: AtomicBool,
}

/// Ce qu'un event du bus a fait au tampon.
#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Ignored,
    Changed,
    /// Connexion terminée (fermée, suspendue, compte changé).
    Stop,
}

impl Watcher {
    fn new(server_id: i64, token_fp: u64) -> Self {
        let epoch = new_epoch();
        Watcher {
            server_id,
            conn_id: format!("agent:{server_id}:{epoch}"),
            epoch,
            token_fp,
            buf: Mutex::new(ConsoleBuffer::new(BUFFER_LINES)),
            changes: watch::channel(0u64).0,
            last_use_ms: AtomicI64::new(now_ms()),
            alive: AtomicBool::new(true),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut ConsoleBuffer) -> R) -> R {
        let mut guard = self.buf.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    fn bump(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn touch(&self) {
        self.last_use_ms.store(now_ms(), Ordering::Relaxed);
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn cursor(&self, seq: u64) -> String {
        format_cursor(&self.epoch, seq)
    }

    fn apply(&self, ev: &CoreEvent, now: i64) -> Flow {
        let mine = |id: &str| id == self.conn_id;
        match ev {
            CoreEvent::ConsoleOutput(o) if mine(&o.conn_id) => {
                self.with(|b| b.on_line(&o.line, now))
            }
            CoreEvent::ConsoleStatus(s) if mine(&s.conn_id) => {
                self.with(|b| b.on_status(&s.state, now))
            }
            CoreEvent::ConsoleStats(s) if mine(&s.conn_id) => {
                self.with(|b| b.on_stats(&s.state, s.uptime, now))
            }
            CoreEvent::ConsoleConnection(c) if mine(&c.conn_id) => {
                self.with(|b| b.on_phase(&c.phase, now));
                if matches!(c.phase.as_str(), "closed" | "suspended") {
                    return Flow::Stop;
                }
            }
            CoreEvent::AccountSwitched(_) => return Flow::Stop,
            _ => return Flow::Ignored,
        }
        self.with(|b| b.on_tick(now));
        Flow::Changed
    }
}

/// Surveillances ouvertes, par serveur.
static WATCHERS: LazyLock<Mutex<HashMap<i64, Arc<Watcher>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn fingerprint(token: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    token.hash(&mut h);
    h.finish()
}

/// Boucle d'une surveillance : events du bus → tampon ; fin du rejeu ; fermeture après
/// inactivité, à la fin de la connexion ou au changement de compte.
async fn pump(
    w: Arc<Watcher>,
    mut rx: broadcast::Receiver<CoreEvent>,
    console: Arc<ConsoleManager>,
) {
    const FAST: Duration = Duration::from_millis(100);
    const SLOW: Duration = Duration::from_secs(1);
    let mut next_tick = tokio::time::Instant::now() + FAST;
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let flow = match ev {
                    Ok(ev) => w.apply(&ev, now_ms()),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        w.with(|b| b.on_lag(n));
                        Flow::Changed
                    }
                    Err(broadcast::error::RecvError::Closed) => Flow::Stop,
                };
                match flow {
                    Flow::Stop => break,
                    Flow::Changed => w.bump(),
                    Flow::Ignored => {}
                }
                // Rejeu en cours : le terminer sans attendre la seconde suivante.
                if w.with(|b| b.replaying()) {
                    next_tick = next_tick.min(tokio::time::Instant::now() + FAST);
                }
            }
            _ = tokio::time::sleep_until(next_tick) => {
                if w.with(|b| b.on_tick(now_ms())) {
                    w.bump();
                }
                if !w.alive() || now_ms() - w.last_use_ms.load(Ordering::Relaxed) > IDLE_CLOSE_MS {
                    break;
                }
                next_tick = tokio::time::Instant::now() + if w.with(|b| b.replaying()) { FAST } else { SLOW };
            }
        }
    }
    w.alive.store(false, Ordering::Relaxed);
    console.disconnect(&w.conn_id);
    {
        let mut reg = WATCHERS.lock().unwrap_or_else(|e| e.into_inner());
        if reg.get(&w.server_id).is_some_and(|x| Arc::ptr_eq(x, &w)) {
            reg.remove(&w.server_id);
        }
    }
    w.bump();
    tracing::debug!(conn = %w.conn_id, "console des agents fermée");
}

/// Attend que l'historique soit reçu (ou le serveur vu hiberné), au plus `max`.
async fn wait_ready(w: &Watcher, max: Duration) -> bool {
    let deadline = Instant::now() + max;
    let mut rx = w.changes.subscribe();
    loop {
        if w.with(|b| b.ready()) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() || !w.alive() || mcp_ctx::cancelled() {
            return false;
        }
        let _ = tokio::time::timeout(left.min(POLL), rx.changed()).await;
    }
}

/// Lignes collectées après un curseur.
#[derive(Debug, Default)]
struct Collected {
    /// Texte déjà anonymisé.
    lines: Vec<Line>,
    /// Ligne qui a vérifié le motif (déjà dans `lines` si gardée).
    matched: Option<Line>,
    /// Dernière ligne examinée.
    last_seq: u64,
    missed: u64,
    gaps: Vec<Gap>,
    deadline_hit: bool,
    cancelled: bool,
    /// Lignes vues au total (gardées ou non).
    seen: usize,
}

/// Collecte les lignes après `from` jusqu'au motif `until`, ou (avec `quiet`) un silence après
/// la première ligne, ou l'échéance. `keep` : lignes gardées au plus.
async fn collect_after(
    w: &Watcher,
    from: u64,
    until: Option<&Regex>,
    quiet: Option<Duration>,
    deadline: Instant,
    keep: usize,
    redact: Redactor,
) -> Collected {
    let mut rx = w.changes.subscribe();
    let mut out = Collected {
        last_seq: from,
        ..Default::default()
    };
    let mut last_line_at: Option<Instant> = None;
    loop {
        w.touch();
        if let Ok(slice) = w.with(|b| b.since(Some(out.last_seq))) {
            out.missed += slice.missed;
            out.gaps.extend(
                slice
                    .gaps
                    .into_iter()
                    .filter(|g| g.after_seq < slice.last_seq),
            );
            for l in slice.lines {
                out.last_seq = l.seq;
                out.seen += 1;
                last_line_at = Some(Instant::now());
                let line = Line {
                    text: redact.apply(&l.text),
                    ..l
                };
                let hit = until.is_some_and(|re| re.is_match(&line.text));
                if out.lines.len() < keep {
                    out.lines.push(line.clone());
                }
                if hit {
                    out.matched = Some(line);
                    return out;
                }
            }
        }
        let now = Instant::now();
        if now >= deadline {
            out.deadline_hit = true;
            return out;
        }
        if let (Some(q), Some(t)) = (quiet, last_line_at) {
            if now.duration_since(t) >= q {
                return out;
            }
        }
        if mcp_ctx::cancelled() {
            out.cancelled = true;
            return out;
        }
        if !w.alive() {
            return out;
        }
        let _ = tokio::time::timeout(
            deadline.saturating_duration_since(now).min(POLL),
            rx.changed(),
        )
        .await;
    }
}

// --- Arguments ------------------------------------------------------------------------------

fn arg_i64(args: &Value, key: &str) -> Option<i64> {
    match args.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn req_i64(args: &Value, key: &str) -> Result<i64, String> {
    arg_i64(args, key).ok_or_else(|| format!("paramètre `{key}` (entier) requis"))
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    match args.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn arg_bool(args: &Value, key: &str) -> Option<bool> {
    match args.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn arg_count(args: &Value, key: &str, default: usize, max: usize) -> usize {
    arg_i64(args, key).map_or(default, |n| n.clamp(1, max as i64) as usize)
}

/// Motif optionnel `key` (avec `regex`).
fn arg_pattern(args: &Value, key: &str) -> Result<Option<Regex>, String> {
    let regex = arg_bool(args, "regex").unwrap_or(false);
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => compile_pattern(s, regex, key).map(Some),
        Some(_) => Err(format!("paramètre `{key}` : texte attendu")),
    }
}

fn secs(d: Duration) -> f64 {
    (d.as_secs_f64() * 10.0).round() / 10.0
}

fn pretty(v: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(v).map_err(|e| e.to_string())
}

const OFFLINE_REFUSAL: &str = "Serveur hors ligne (état offline) : commande NON envoyée. Démarre-le (power_action start), attends wait_for state=running, puis relance.";

/// Résultat d'une attente d'état.
struct StateWait {
    reached: bool,
    reason: Option<String>,
    state: String,
    elapsed: Duration,
    /// Dernière ligne avant le démarrage observé (ou avant l'attente) : les lignes du démarrage
    /// suivent ce numéro.
    startup_from: Option<u64>,
    /// Durée du démarrage depuis le passage à `starting` observé.
    startup_ms: Option<i64>,
    via_rest: bool,
}

// --- Outils (façade Core) -------------------------------------------------------------------

impl Core {
    fn ai_redactor(&self) -> Redactor {
        Redactor(self.get_privacy_config().redact_ai)
    }

    /// Surveillance console d'un serveur pour les agents : réutilisée, ou ouverte (connexion
    /// dédiée) si absente, fermée, ou ouverte avec la clé d'un autre compte.
    fn agent_watcher(&self, id: i64) -> crate::Result<Arc<Watcher>> {
        let token = self.token()?;
        let fp = fingerprint(&token);
        let mut reg = WATCHERS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = reg.get(&id) {
            if w.alive() && w.token_fp == fp {
                w.touch();
                return Ok(w.clone());
            }
        }
        if let Some(old) = reg.remove(&id) {
            old.alive.store(false, Ordering::Relaxed);
            self.console.disconnect(&old.conn_id);
        }
        let w = Arc::new(Watcher::new(id, fp));
        // Abonné AVANT la connexion : aucun event de celle-ci ne peut manquer.
        let rx = self.events.subscribe();
        self.console
            .connect(self.api.clone(), token, w.conn_id.clone(), id, false);
        tokio::spawn(pump(w.clone(), rx, self.console.clone()));
        reg.insert(id, w.clone());
        Ok(w)
    }

    /// Surveillance prête à servir, ou la raison du repli sur l'API.
    async fn agent_console(&self, id: i64) -> Result<(Arc<Watcher>, Option<String>), String> {
        let w = self.agent_watcher(id).map_err(|e| e.to_string())?;
        let ready = wait_ready(&w, READY_WAIT).await;
        if ready && w.with(|b| b.needs_seed()) {
            // Serveur éteint : Wings ne rejoue rien ; les dernières lignes connues de l'API.
            let lines = self.console_logs(id).await.unwrap_or_default();
            w.with(|b| b.seed_history(lines));
            w.bump();
        }
        let why = w.with(|b| b.unusable(ready));
        Ok((w, why))
    }

    /// État par l'API : `(état normalisé, uptime en secondes)`.
    async fn rest_state(&self, id: i64) -> crate::Result<(String, Option<i64>)> {
        match self.server_live(id).await {
            Ok(l) => Ok((
                normalize_state(&l.state),
                l.stats.uptime.map(|u| u.total_seconds),
            )),
            Err(e) => match self.live_light(id).await {
                Ok(ll) => Ok((normalize_state(ll.status.as_deref().unwrap_or("")), None)),
                Err(_) => Err(e),
            },
        }
    }

    /// Dernier arrêt ou redémarrage demandé, s'il est récent (ms Unix).
    fn recent_power_mark(&self, id: i64) -> Option<i64> {
        let mark = self.sup_state.last_expected_stop(id)? * 1000;
        (now_ms() - mark < MARK_WINDOW_MS).then_some(mark)
    }

    /// Outil `tail_console`.
    pub(crate) async fn agent_tail_console(&self, args: &Value) -> Result<String, String> {
        let id = req_i64(args, "server_id")?;
        let cursor = parse_cursor(args.get("after"))?;
        let wait = cap_secs(arg_f64(args, "wait_secs"), TAIL_WAIT, copilot_call());
        let max = arg_count(args, "max_lines", 100, 1000);
        let filter = arg_pattern(args, "filter")?;
        let redact = self.ai_redactor();
        let (w, why) = self.agent_console(id).await?;
        if let Some(why) = why {
            return self
                .tail_from_rest(id, max, filter.as_ref(), redact, &why)
                .await;
        }
        let mut notes: Vec<String> = Vec::new();
        let last = w.with(|b| b.last_seq());
        let start = resolve_cursor(cursor.as_ref(), &w.epoch, last)?;
        let mut out = json!({ "server_id": id, "source": "websocket" });
        match start {
            Start::Latest | Start::Reset => {
                if start == Start::Reset {
                    notes.push("curseur d'une connexion console précédente (fermée depuis) : reprise sur les dernières lignes ; des lignes ont pu manquer entre-temps".into());
                    out["cursor_reset"] = json!(true);
                }
                let slice = w.with(|b| b.since(None))?;
                let (lines, history, omitted) =
                    pick_latest(&slice.lines, filter.as_ref(), max, redact);
                if omitted {
                    out["older_omitted"] = json!(true);
                }
                out["cursor"] = json!(w.cursor(slice.last_seq));
                if history > 0 {
                    out["history"] = json!(history);
                }
                out["lines"] = json!(lines);
            }
            Start::After(from) => {
                let deadline = Instant::now() + wait;
                let mut rx = w.changes.subscribe();
                let mut picker = Picker::new(filter.as_ref(), max, redact, from);
                let mut missed = 0;
                let mut gaps: Vec<Gap> = Vec::new();
                loop {
                    w.touch();
                    let slice = w.with(|b| b.since(Some(picker.cursor)))?;
                    missed += slice.missed;
                    gaps.extend(slice.gaps);
                    picker.feed(&slice.lines);
                    let left = deadline.saturating_duration_since(Instant::now());
                    if !picker.lines.is_empty()
                        || picker.truncated
                        || left.is_zero()
                        || mcp_ctx::cancelled()
                        || !w.alive()
                    {
                        break;
                    }
                    let _ = tokio::time::timeout(left.min(POLL), rx.changed()).await;
                }
                for g in gaps.iter().filter(|g| g.after_seq < picker.cursor) {
                    if !notes.contains(&g.reason) {
                        notes.push(g.reason.clone());
                    }
                }
                out["cursor"] = json!(w.cursor(picker.cursor));
                if missed > 0 {
                    out["missed"] = json!(missed);
                    notes.push(format!("{missed} ligne(s) perdue(s) : sorties du tampon ({BUFFER_LINES} lignes) avant cette lecture ; lire plus souvent"));
                }
                if picker.truncated {
                    out["more"] = json!(true);
                }
                if picker.history > 0 {
                    out["history"] = json!(picker.history);
                }
                out["lines"] = json!(picker.lines);
            }
        }
        let (state, phase) = w.with(|b| (b.state().map(str::to_string), b.phase.clone()));
        out["state"] = json!(state);
        if phase != "open" && phase != "hibernated" {
            notes.push(format!("console en reconnexion (phase : {phase}) : les lignes les plus récentes arriveront au raccord"));
        }
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        pretty(&out)
    }

    async fn tail_from_rest(
        &self,
        id: i64,
        max: usize,
        filter: Option<&Regex>,
        redact: Redactor,
        why: &str,
    ) -> Result<String, String> {
        let logs = self
            .console_logs(id)
            .await
            .map_err(|e| format!("console indisponible en direct ({why}) et par l'API : {e}"))?;
        let lines: Vec<Line> = logs
            .iter()
            .flat_map(|l| clean_lines(l))
            .enumerate()
            .map(|(i, text)| Line {
                seq: i as u64 + 1,
                text,
                history: true,
            })
            .collect();
        let (lines, _, _) = pick_latest(&lines, filter, max, redact);
        let state = self.rest_state(id).await.ok().map(|s| s.0);
        pretty(&json!({
            "server_id": id,
            "source": "rest",
            "state": state,
            "cursor": null,
            "lines": lines,
            "notes": [format!("console en direct indisponible ({why}) : dernières lignes lues par l'API, sans curseur ; relancer plus tard pour suivre la console")],
        }))
    }

    /// Outil `run_command`.
    pub(crate) async fn agent_run_command(&self, args: &Value) -> Result<String, String> {
        let id = req_i64(args, "server_id")?;
        let command = arg_str(args, "command").ok_or("paramètre `command` (texte) requis")?;
        if command.contains(['\n', '\r']) {
            return Err("`command` : une seule ligne (envoyer les commandes une par une)".into());
        }
        let until = arg_pattern(args, "until")?;
        let timeout = cap_secs(arg_f64(args, "timeout_secs"), RUN_TIMEOUT, copilot_call());
        let max = arg_count(args, "max_lines", 200, 1000);
        let redact = self.ai_redactor();
        let (w, why) = self.agent_console(id).await?;
        if let Some(why) = why {
            return self
                .run_command_rest(id, command, until.as_ref(), timeout, max, redact, &why)
                .await;
        }
        let known = w.with(|b| b.live_state().or(b.state()).map(str::to_string));
        if known.as_deref() == Some("offline") {
            return Err(OFFLINE_REFUSAL.into());
        }
        let from = w.with(|b| b.last_seq());
        // Invariant : envoi par l'API REST, le WebSocket ne sert qu'à lire la réponse.
        self.send_command(id, command)
            .await
            .map_err(|e| format!("commande non envoyée : {e}"))?;
        let deadline = Instant::now() + timeout;
        let quiet = until.is_none().then_some(QUIET);
        let got = collect_after(
            &w,
            from,
            until.as_ref(),
            quiet,
            deadline,
            COLLECT_MAX,
            redact,
        )
        .await;
        let mut picker = Picker::new(None, max, Redactor(false), from);
        picker.feed(&got.lines);
        let cursor = if picker.truncated {
            picker.cursor
        } else {
            got.last_seq
        };
        let timed_out = got.deadline_hit && (until.is_some() || got.lines.is_empty());
        let mut out = json!({
            "server_id": id,
            "sent": true,
            "lines": picker.lines,
            "matched": until.as_ref().map(|_| got.matched.is_some()),
            "timed_out": timed_out,
            "cursor": w.cursor(cursor),
            "state": w.with(|b| b.state().map(str::to_string)),
            "source": "websocket",
        });
        if let Some(m) = &got.matched {
            out["matched_line"] = json!(m.text);
        }
        if picker.truncated {
            out["more"] = json!(true);
        }
        let mut notes: Vec<String> = got.gaps.iter().map(|g| g.reason.clone()).collect();
        notes.dedup();
        if got.missed > 0 {
            out["missed"] = json!(got.missed);
        }
        if got.cancelled {
            notes.push("attente annulée par le client : la commande a été envoyée".into());
        }
        if timed_out && got.lines.is_empty() {
            notes.push("aucune sortie dans le délai : la commande peut ne rien afficher, ou répondre plus tard (tail_console avec ce curseur)".into());
        } else if timed_out {
            notes.push("motif `until` non vu dans le délai : la suite éventuelle se lit avec tail_console à partir de ce curseur".into());
        }
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        pretty(&out)
    }

    /// `run_command` sans console en direct : réponse lue par l'API (dernières lignes), raccordée
    /// à celles d'avant l'envoi.
    #[allow(clippy::too_many_arguments)]
    async fn run_command_rest(
        &self,
        id: i64,
        command: &str,
        until: Option<&Regex>,
        timeout: Duration,
        max: usize,
        redact: Redactor,
        why: &str,
    ) -> Result<String, String> {
        let state = self.rest_state(id).await.ok().map(|s| s.0);
        if state.as_deref() == Some("offline") {
            return Err(OFFLINE_REFUSAL.into());
        }
        let clean = |logs: Vec<String>| -> Vec<String> {
            logs.iter().flat_map(|l| clean_lines(l)).collect()
        };
        let before = clean(self.console_logs(id).await.unwrap_or_default());
        self.send_command(id, command)
            .await
            .map_err(|e| format!("commande non envoyée : {e}"))?;
        let deadline = Instant::now() + timeout;
        let mut new: Vec<String> = Vec::new();
        let mut matched: Option<String> = None;
        loop {
            tokio::time::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(1500)),
            )
            .await;
            let after = clean(self.console_logs(id).await.unwrap_or_default());
            let tail: Vec<&str> = before.iter().map(String::as_str).collect();
            let start = new_lines_start(&tail, &after).unwrap_or(0);
            let fresh: Vec<String> = after[start.min(after.len())..]
                .iter()
                .map(|l| redact.apply(l))
                .collect();
            let grew = fresh.len() > new.len();
            new = fresh;
            if let Some(re) = until {
                if let Some(pos) = new.iter().position(|l| re.is_match(l)) {
                    matched = Some(new[pos].clone());
                    new.truncate(pos + 1);
                    break;
                }
            } else if !new.is_empty() && !grew {
                break;
            }
            if Instant::now() >= deadline || mcp_ctx::cancelled() {
                break;
            }
        }
        let timed_out = matched.is_none() && (until.is_some() || new.is_empty());
        let lines: Vec<String> = new
            .iter()
            .take(max)
            .map(|l| crate::util::truncate_on_boundary(l, OUT_LINE_BYTES, "…"))
            .collect();
        let mut out = json!({
            "server_id": id,
            "sent": true,
            "lines": lines,
            "matched": until.map(|_| matched.is_some()),
            "timed_out": timed_out,
            "cursor": null,
            "state": state,
            "source": "rest",
            "notes": [format!("console en direct indisponible ({why}) : réponse lue par l'API (100 dernières lignes), raccord approximatif")],
        });
        if let Some(m) = matched {
            out["matched_line"] = json!(m);
        }
        pretty(&out)
    }

    /// Outil `wait_for`.
    pub(crate) async fn agent_wait_for(&self, args: &Value) -> Result<String, String> {
        let id = req_i64(args, "server_id")?;
        let timeout = cap_secs(arg_f64(args, "timeout_secs"), WAIT_TIMEOUT, copilot_call());
        let state = arg_str(args, "state").map(normalize_state);
        let pattern = arg_pattern(args, "pattern")?;
        let job = arg_i64(args, "job_id");
        let conditions = usize::from(state.is_some())
            + usize::from(pattern.is_some())
            + usize::from(job.is_some());
        if conditions != 1 {
            return Err("une condition, et une seule, parmi `state` (running | offline | starting | stopping), `pattern` ou `job_id`".into());
        }
        if let Some(job_id) = job {
            return self.wait_job(id, job_id, timeout).await;
        }
        if let Some(target) = state {
            if !matches!(
                target.as_str(),
                "running" | "offline" | "starting" | "stopping"
            ) {
                return Err(format!(
                    "`state` inconnu : « {target} » (running, offline, starting ou stopping)"
                ));
            }
            return self.wait_state_tool(id, &target, timeout).await;
        }
        let re = pattern.ok_or("paramètre `pattern` requis")?;
        self.wait_pattern(id, &re, parse_cursor(args.get("after"))?, timeout)
            .await
    }

    async fn wait_job(&self, id: i64, job_id: i64, timeout: Duration) -> Result<String, String> {
        let started = Instant::now();
        let deadline = started + timeout;
        let mut failures = 0;
        let mut last = None;
        let reason = loop {
            match self.backup_job(job_id).await {
                Ok(job) => {
                    failures = 0;
                    let done = job.finished();
                    mcp_ctx::progress(
                        job.progress as f64,
                        Some(100.0),
                        &format!("tâche {job_id} : {} ({} %)", job.status, job.progress),
                    )
                    .await;
                    last = Some(job);
                    if done {
                        break None;
                    }
                }
                Err(e) => {
                    failures += 1;
                    if failures >= 3 {
                        return Err(format!("suivi de la tâche {job_id} impossible : {e}"));
                    }
                }
            }
            if Instant::now() >= deadline {
                break Some("timeout");
            }
            if !sleep_or_cancel(JOB_POLL.min(deadline.saturating_duration_since(Instant::now())))
                .await
            {
                break Some("cancelled");
            }
        };
        let reached = last.as_ref().is_some_and(|j| j.status == "completed");
        let state = self.rest_state(id).await.ok().map(|s| s.0);
        let mut out = json!({
            "server_id": id,
            "condition": { "job_id": job_id },
            "reached": reached,
            "elapsed_secs": secs(started.elapsed()),
            "state": state,
            "job": last,
        });
        if let Some(r) = reason {
            out["reason"] = json!(r);
        } else if !reached {
            out["reason"] = json!("job_failed");
        }
        pretty(&out)
    }

    /// Attend l'état `target` : WebSocket si fiable, sinon l'API (sondée toutes les 4 s).
    async fn wait_state(
        &self,
        id: i64,
        w: Option<&Watcher>,
        target: &str,
        timeout: Duration,
        mark_ms: Option<i64>,
        label: &str,
    ) -> StateWait {
        let started = Instant::now();
        let since_ms = now_ms();
        let deadline = started + timeout;
        let mut rx = w.map(|w| w.changes.subscribe());
        let start_seq = w.map(|w| w.with(|b| b.last_seq()));
        let mut rest: Option<(Instant, String, Option<i64>)> = None;
        let mut rest_changes: Vec<StateChange> = Vec::new();
        let mut initial: Option<String> = None;
        let mut last_progress: Option<Instant> = None;
        let from_ms = mark_ms.map_or(since_ms, |m| m.min(since_ms));
        let finish = |reached: bool, reason: Option<String>, state: String, via_rest: bool| {
            let starting = w.and_then(|w| {
                w.with(|b| b.changes_since(from_ms))
                    .into_iter()
                    .rev()
                    .find(|c| c.state == "starting")
            });
            StateWait {
                reached,
                reason,
                state,
                elapsed: started.elapsed(),
                startup_from: starting.as_ref().map(|c| c.after_seq).or(start_seq),
                startup_ms: starting.map(|c| now_ms() - c.at_ms),
                via_rest,
            }
        };
        loop {
            if let Some(w) = w {
                w.touch();
            }
            let ws = w.filter(|w| w.alive()).and_then(|w| {
                w.with(|b| {
                    b.live_state()
                        .map(|s| (s.to_string(), b.changes_since(from_ms), b.process_start_ms))
                })
            });
            let (state, changes, process_start, via_rest) = match ws {
                Some((s, c, p)) => (s, c, p, false),
                None => {
                    if rest.as_ref().is_none_or(|r| r.0.elapsed() >= REST_POLL) {
                        if let Ok((s, uptime)) = self.rest_state(id).await {
                            if rest.as_ref().is_none_or(|r| r.1 != s) {
                                rest_changes.push(StateChange {
                                    after_seq: 0,
                                    state: s.clone(),
                                    at_ms: now_ms(),
                                });
                            }
                            let start = uptime.filter(|u| *u > 0).map(|u| now_ms() - u * 1000);
                            rest = Some((Instant::now(), s, start));
                        }
                    }
                    match &rest {
                        Some((_, s, p)) => (s.clone(), rest_changes.clone(), *p, true),
                        None => ("unknown".to_string(), Vec::new(), None, true),
                    }
                }
            };
            let init = initial.get_or_insert_with(|| state.clone()).clone();
            let obs = Observation {
                state: &state,
                initial: &init,
                changes: &changes,
                process_start_ms: process_start,
                mark_ms,
                since_ms,
            };
            match judge_state(target, &obs) {
                Judgement::Reached => return finish(true, None, state, via_rest),
                Judgement::Failed(why) => {
                    return finish(false, Some(why.to_string()), state, via_rest)
                }
                Judgement::Pending => {}
            }
            if Instant::now() >= deadline {
                return finish(false, Some("timeout".into()), state, via_rest);
            }
            if mcp_ctx::cancelled() {
                return finish(false, Some("cancelled".into()), state, via_rest);
            }
            if last_progress.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) {
                last_progress = Some(Instant::now());
                let el = started.elapsed();
                mcp_ctx::progress(
                    el.as_secs_f64(),
                    Some(timeout.as_secs_f64()),
                    &format!("{label} : état {state} ({} s)", el.as_secs()),
                )
                .await;
            }
            let pause = deadline
                .saturating_duration_since(Instant::now())
                .min(if via_rest {
                    Duration::from_secs(1)
                } else {
                    POLL
                });
            match rx.as_mut() {
                Some(rx) if !via_rest => {
                    let _ = tokio::time::timeout(pause, rx.changed()).await;
                }
                _ => tokio::time::sleep(pause).await,
            }
        }
    }

    /// Lignes du démarrage (après `from`), anonymisées plus tard.
    fn startup_lines(w: &Watcher, from: Option<u64>) -> Vec<String> {
        w.with(|b| {
            b.since(from)
                .map(|s| s.lines.into_iter().map(|l| l.text).collect())
                .unwrap_or_default()
        })
    }

    async fn wait_state_tool(
        &self,
        id: i64,
        target: &str,
        timeout: Duration,
    ) -> Result<String, String> {
        let redact = self.ai_redactor();
        let (w, why) = self.agent_console(id).await?;
        let mark = if target == "running" {
            self.recent_power_mark(id)
        } else {
            None
        };
        let watcher = why.is_none().then_some(&*w);
        let res = self
            .wait_state(
                id,
                watcher,
                target,
                timeout,
                mark,
                &format!("attente de {target}"),
            )
            .await;
        let mut out = json!({
            "server_id": id,
            "condition": { "state": target },
            "reached": res.reached,
            "elapsed_secs": secs(res.elapsed),
            "state": res.state,
            "source": if res.via_rest { "rest" } else { "websocket" },
        });
        if let Some(r) = &res.reason {
            out["reason"] = json!(r);
        }
        if why.is_none() {
            out["cursor"] = json!(w.cursor(w.with(|b| b.last_seq())));
        }
        let mut notes: Vec<String> = Vec::new();
        if let Some(why) = &why {
            notes.push(format!(
                "console en direct indisponible ({why}) : état sondé par l'API toutes les {} s",
                REST_POLL.as_secs()
            ));
        }
        if mark.is_some() && target == "running" {
            notes.push("arrêt ou redémarrage demandé récemment : « running » n'est retenu qu'après le redémarrage effectif".into());
        }
        if !res.reached
            && res.state == "offline"
            && target != "offline"
            && res.reason.as_deref() == Some("timeout")
        {
            notes.push(
                "le serveur est resté éteint pendant toute l'attente : power_action start ?".into(),
            );
        }
        // Démarrage (ou plantage pendant le démarrage) : résumé de ce que la console a montré.
        if target == "running"
            && why.is_none()
            && (res.reached || res.reason.as_deref() != Some("cancelled"))
        {
            let lines = Self::startup_lines(&w, res.startup_from);
            let s = startup_digest(&lines);
            let mut startup = json!({
                "secs": res.startup_ms.map_or(secs(res.elapsed), |ms| (ms as f64 / 100.0).round() / 10.0),
                "lines_examined": lines.len(),
                "errors": groups_json(&s.errors, redact),
                "warnings": groups_json(&s.warnings, redact),
            });
            if let Some(d) = &s.done_line {
                startup["done_line"] = json!(redact.apply(d));
            }
            if let Some(r) = s.reported_secs {
                startup["server_reported_secs"] = json!(r);
            }
            if !res.reached {
                let tail: Vec<String> = lines
                    .iter()
                    .rev()
                    .take(20)
                    .rev()
                    .map(|l| redact.apply(l))
                    .collect();
                startup["console_tail"] = json!(tail);
            }
            out["startup"] = startup;
        }
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        pretty(&out)
    }

    async fn wait_pattern(
        &self,
        id: i64,
        re: &Regex,
        after: Option<Cursor>,
        timeout: Duration,
    ) -> Result<String, String> {
        let redact = self.ai_redactor();
        let started = Instant::now();
        let deadline = started + timeout;
        let (w, why) = self.agent_console(id).await?;
        if let Some(why) = why {
            return self.wait_pattern_rest(id, re, deadline, redact, &why).await;
        }
        let mut notes = Vec::new();
        let last = w.with(|b| b.last_seq());
        let from = match resolve_cursor(after.as_ref(), &w.epoch, last)? {
            Start::After(s) => s,
            Start::Latest => last,
            Start::Reset => {
                notes.push("curseur d'une connexion console précédente : attente des lignes à venir seulement".to_string());
                last
            }
        };
        let label = "attente du motif";
        let progress = async {
            // Avancement signalé pendant l'attente (sans effet hors transport MCP).
            loop {
                tokio::time::sleep(PROGRESS_EVERY).await;
                let el = started.elapsed();
                mcp_ctx::progress(
                    el.as_secs_f64(),
                    Some(timeout.as_secs_f64()),
                    &format!("{label} ({} s)", el.as_secs()),
                )
                .await;
            }
        };
        let got = tokio::select! {
            got = collect_after(&w, from, Some(re), None, deadline, 0, redact) => got,
            _ = progress => unreachable!("boucle d'avancement sans fin"),
        };
        let mut out = json!({
            "server_id": id,
            "condition": { "pattern": re.as_str() },
            "reached": got.matched.is_some(),
            "elapsed_secs": secs(started.elapsed()),
            "state": w.with(|b| b.state().map(str::to_string)),
            "lines_seen": got.seen,
            "cursor": w.cursor(got.matched.as_ref().map_or(got.last_seq, |m| m.seq)),
            "source": "websocket",
        });
        if let Some(m) = &got.matched {
            out["matched_line"] = json!(crate::util::truncate_on_boundary(
                &m.text,
                OUT_LINE_BYTES,
                "…"
            ));
        } else {
            out["reason"] = json!(if got.cancelled {
                "cancelled"
            } else {
                "timeout"
            });
        }
        if got.missed > 0 {
            out["missed"] = json!(got.missed);
        }
        notes.extend(got.gaps.into_iter().map(|g| g.reason));
        notes.dedup();
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        pretty(&out)
    }

    async fn wait_pattern_rest(
        &self,
        id: i64,
        re: &Regex,
        deadline: Instant,
        redact: Redactor,
        why: &str,
    ) -> Result<String, String> {
        let started = Instant::now();
        let clean = |logs: Vec<String>| -> Vec<String> {
            logs.iter().flat_map(|l| clean_lines(l)).collect()
        };
        let mut previous = clean(self.console_logs(id).await.unwrap_or_default());
        let mut matched = None;
        while matched.is_none() && Instant::now() < deadline && !mcp_ctx::cancelled() {
            tokio::time::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_secs(3)),
            )
            .await;
            let current = clean(self.console_logs(id).await.unwrap_or_default());
            let tail: Vec<&str> = previous.iter().map(String::as_str).collect();
            let start = new_lines_start(&tail, &current)
                .unwrap_or(0)
                .min(current.len());
            matched = current[start..]
                .iter()
                .map(|l| redact.apply(l))
                .find(|l| re.is_match(l));
            previous = current;
        }
        let state = self.rest_state(id).await.ok().map(|s| s.0);
        let mut out = json!({
            "server_id": id,
            "condition": { "pattern": re.as_str() },
            "reached": matched.is_some(),
            "elapsed_secs": secs(started.elapsed()),
            "state": state,
            "cursor": null,
            "source": "rest",
            "notes": [format!("console en direct indisponible ({why}) : console sondée par l'API toutes les 3 s (100 dernières lignes)")],
        });
        match matched {
            Some(m) => out["matched_line"] = json!(m),
            None => out["reason"] = json!("timeout"),
        }
        pretty(&out)
    }

    /// Outil `deploy_and_verify` : envoi d'un jar local, redémarrage, attente, verdict.
    pub(crate) async fn agent_deploy(&self, args: &Value) -> Result<String, String> {
        const STEPS: f64 = 6.0;
        let id = req_i64(args, "server_id")?;
        let local_path = arg_str(args, "local_path")
            .ok_or("paramètre `local_path` (chemin du .jar local) requis")?;
        let restart = match arg_str(args, "restart").unwrap_or("restart") {
            "restart" => true,
            "none" => false,
            other => return Err(format!("`restart` inconnu : « {other} » (restart ou none)")),
        };
        let replace_previous = arg_bool(args, "replace_previous").unwrap_or(false);
        let timeout = cap_secs(
            arg_f64(args, "timeout_secs"),
            DEPLOY_TIMEOUT,
            copilot_call(),
        );
        let redact = self.ai_redactor();
        let mut notes: Vec<String> = Vec::new();
        let mut warnings_out: Vec<String> = Vec::new();

        // 1. Jar local et son descripteur.
        mcp_ctx::progress(0.0, Some(STEPS), "lecture du jar local").await;
        let file = std::path::Path::new(local_path)
            .file_name()
            .and_then(|f| f.to_str())
            .map(str::to_string)
            .ok_or_else(|| format!("`local_path` sans nom de fichier : « {local_path} »"))?;
        let file = sftp::safe_file_name(&file).map_err(|e| e.to_string())?;
        let bytes = tokio::fs::read(local_path)
            .await
            .map_err(|e| format!("lecture du fichier local « {local_path} » : {e}"))?;
        if !file.to_ascii_lowercase().ends_with(".jar") {
            warnings_out.push(format!(
                "« {file} » n'a pas l'extension .jar : le serveur ne le chargera sans doute pas"
            ));
        }
        let meta = match jar_meta_of(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                warnings_out.push(format!("jar illisible ({e}) : descripteur inconnu"));
                None
            }
        };
        let local_size = bytes.len() as u64;
        drop(bytes);
        let token = self.token().map_err(|e| e.to_string())?;
        let mc = self.api.minecraft_version(&token, id).await.ok();
        let accepted = mc.as_ref().and_then(|m| {
            crate::plugin_deps::platforms_from_source(
                &m.source,
                Some(m.version.as_str()).filter(|v| !v.is_empty()),
            )
        });
        let remote_arg = arg_str(args, "remote_path");
        let folder_hint = remote_arg.map(|r| {
            let r = r.to_ascii_lowercase();
            !(r.starts_with("mods/") || r.contains("/mods/") || r.ends_with("/mods"))
        });
        let descriptor = meta
            .as_ref()
            .and_then(|m| pick_descriptor(m, accepted.as_ref().map(|a| &a.platforms), folder_hint));
        let ident = descriptor.map(|d| Identity {
            name: d.name.clone(),
            id: d.id.clone(),
            version: d.version.clone(),
            platform: Some(d.platform),
            file: file.clone(),
        });
        if let (Some(d), Some(acc)) = (descriptor, accepted.as_ref()) {
            if !acc.vanilla && !acc.platforms.is_empty() && !acc.platforms.contains(&d.platform) {
                let names: Vec<&str> = acc.platforms.iter().map(|p| p.label()).collect();
                warnings_out.push(format!(
                    "ce jar vise {} ; le serveur charge {}",
                    d.platform.label(),
                    names.join(" + ")
                ));
            }
        }
        let default_dir = match (descriptor, accepted.as_ref()) {
            (Some(d), _) => {
                if d.platform.is_plugin() {
                    "/plugins"
                } else {
                    "/mods"
                }
            }
            (None, Some(acc))
                if !acc.platforms.is_empty() && acc.platforms.iter().all(|p| !p.is_plugin()) =>
            {
                "/mods"
            }
            (None, _) if remote_arg.is_none() => {
                return Err(format!("aucun descripteur de plugin ou de mod reconnu dans « {file} » : préciser `remote_path`"));
            }
            _ => "/plugins",
        };
        let remote = resolve_remote(remote_arg, &file, default_dir);
        let remote_dir = remote
            .rsplit_once('/')
            .map_or("/", |(d, _)| if d.is_empty() { "/" } else { d })
            .to_string();
        let remote_file = remote.rsplit('/').next().unwrap_or(&file).to_string();
        if mcp_ctx::cancelled() {
            return Err("annulé avant l'envoi : rien n'a été modifié".into());
        }

        // 2. Autre jar du même plugin sous un autre nom.
        mcp_ctx::progress(1.0, Some(STEPS), "recherche d'une version précédente").await;
        let (siblings, taken) = match &ident {
            Some(ident) => self
                .same_plugin_jars(id, &remote_dir, &remote_file, ident)
                .await
                .map_err(|e| e.to_string())?,
            None => (Vec::new(), Vec::new()),
        };
        if taken.iter().any(|n| n.eq_ignore_ascii_case(&remote_file)) {
            notes.push(format!("« {remote_file} » existait déjà : remplacé"));
        }

        // 3. Console suivie AVANT le redémarrage : ses lignes de démarrage seront dans le tampon.
        let console = if restart {
            Some(self.agent_console(id).await?)
        } else {
            None
        };
        let watcher = console
            .as_ref()
            .and_then(|(w, why)| why.is_none().then_some(&**w));
        if let Some((_, Some(why))) = &console {
            notes.push(format!("console en direct indisponible ({why}) : état sondé par l'API, analyse sur les 100 dernières lignes"));
        }

        // 4. Envoi.
        mcp_ctx::progress(2.0, Some(STEPS), &format!("envoi vers {remote}")).await;
        // Envoi et mise de côté par `mcp::dispatch` : la garde des agents copie le jar remplacé
        // et défait le renommage (rollback_changeset).
        let upload = json!({ "server_id": id, "local_path": local_path, "remote_path": remote });
        if let Some(note) = crate::agent_guard::guarded_write(self, "upload_file", upload)
            .await
            .map_err(|e| format!("envoi SFTP de « {local_path} » vers « {remote} » : {e}"))?
        {
            notes.push(note);
        }
        let uploaded = local_size;
        let mut previous = Vec::new();
        for s in &siblings {
            if replace_previous {
                let to = format!("{remote_dir}/{}", old_name(&s.name, &taken)).replace("//", "/");
                let rename = json!({ "server_id": id, "from": s.path, "to": to });
                match crate::agent_guard::guarded_write(self, "rename_path", rename).await {
                    Ok(note) => {
                        notes.extend(note);
                        previous.push(json!({ "path": s.path, "action": "renamed", "to": to }))
                    }
                    Err(e) => {
                        warnings_out.push(format!(
                            "« {} » (même plugin) n'a pas pu être mis de côté : {e}",
                            s.path
                        ));
                        previous.push(json!({ "path": s.path, "action": "kept" }));
                    }
                }
            } else {
                warnings_out.push(format!(
                    "« {} » contient aussi ce plugin : deux versions seront chargées (ou une seule, au hasard) ; relancer avec replace_previous=true pour le mettre de côté",
                    s.path
                ));
                previous.push(json!({ "path": s.path, "action": "kept" }));
            }
        }
        let ident_json = |out: &mut Value| {
            out["plugin"] = json!(ident.as_ref().map(|i| i.name.clone()));
            out["version"] = json!(ident.as_ref().map(|i| i.version.clone()));
            out["platform"] = json!(ident.as_ref().and_then(|i| i.platform).map(|p| p.label()));
        };
        let mut out = json!({ "server_id": id, "remote_path": remote, "uploaded_bytes": uploaded });
        ident_json(&mut out);
        if !previous.is_empty() {
            out["previous_jars"] = json!(previous);
        }
        if !restart {
            out["ok"] = json!(true);
            out["restarted"] = json!(false);
            out["enabled"] = Value::Null;
            notes.push(
                "pas de redémarrage : le jar sera chargé au prochain démarrage (non vérifié)"
                    .into(),
            );
            out["deploy_warnings"] = json!(warnings_out);
            out["notes"] = json!(notes);
            mcp_ctx::progress(STEPS, Some(STEPS), "envoyé").await;
            return pretty(&out);
        }
        if mcp_ctx::cancelled() {
            out["ok"] = json!(false);
            out["restarted"] = json!(false);
            out["notes"] = json!(["annulé après l'envoi : jar en place, serveur non redémarré"]);
            return pretty(&out);
        }

        // 5. Redémarrage (démarrage s'il est éteint) et attente de `running`.
        mcp_ctx::progress(3.0, Some(STEPS), "redémarrage").await;
        let current = match watcher.and_then(|w| w.with(|b| b.live_state().map(str::to_string))) {
            Some(s) => Some(s),
            None => self.rest_state(id).await.ok().map(|s| s.0),
        };
        let action = if current.as_deref() == Some("offline") {
            "start"
        } else {
            "restart"
        };
        let mark = now_ms();
        let rest_before = if watcher.is_none() {
            self.console_logs(id).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        self.power_action(id, action)
            .await
            .map_err(|e| format!("jar envoyé ({uploaded} octets), mais {action} refusé : {e}"))?;
        mcp_ctx::progress(4.0, Some(STEPS), "attente du démarrage").await;
        let res = self
            .wait_state(id, watcher, "running", timeout, Some(mark), "démarrage")
            .await;
        if res.reached {
            sleep_or_cancel(SETTLE).await;
        }

        // 6. Analyse.
        mcp_ctx::progress(5.0, Some(STEPS), "analyse de la console").await;
        let lines: Vec<String> = match watcher {
            Some(w) => Self::startup_lines(w, res.startup_from),
            None => {
                let before: Vec<String> = rest_before.iter().flat_map(|l| clean_lines(l)).collect();
                let after: Vec<String> = self
                    .console_logs(id)
                    .await
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|l| clean_lines(l))
                    .collect();
                let tail: Vec<&str> = before.iter().map(String::as_str).collect();
                let start = new_lines_start(&tail, &after).unwrap_or(0).min(after.len());
                after[start..].to_vec()
            }
        };
        let known = ident.is_some();
        let ident = ident.unwrap_or_else(|| Identity {
            name: String::new(),
            id: String::new(),
            version: String::new(),
            platform: None,
            file: remote_file.clone(),
        });
        let check = check_deploy(&lines, &ident, res.reached);
        let ok = check.ok(&ident, res.reached, known);
        out["ok"] = json!(ok);
        out["restarted"] = json!(action);
        out["state"] = json!(res.state);
        out["startup_secs"] = if res.reached {
            json!(res
                .startup_ms
                .map_or(secs(res.elapsed), |ms| (ms as f64 / 100.0).round() / 10.0))
        } else {
            Value::Null
        };
        if let Some(r) = check.reported_secs {
            out["server_reported_secs"] = json!(r);
        }
        if let Some(r) = &res.reason {
            out["reason"] = json!(r);
        }
        out["enabled"] = json!(check.enabled);
        if let Some(e) = &check.evidence {
            out["evidence"] = json!(redact.apply(e));
        } else if check.deduced {
            out["evidence"] = json!("déduit : démarrage complet sans erreur liée au mod (un mod qui ne charge pas arrête le serveur)");
        }
        if let Some(v) = &check.loaded_version {
            out["loaded_version"] = json!(v);
        }
        if let Some(v) = check.version_mismatch(&ident) {
            notes.push(format!("version chargée {v} ≠ version envoyée {} : un autre jar du même plugin a été chargé à sa place", ident.version));
        }
        if let Some(d) = &check.duplicate {
            notes.push(format!("deux jars pour ce plugin : {}", redact.apply(d)));
        }
        if known && ident.announced() && check.enabled.is_none() && res.reached {
            notes.push(
                "activation non confirmée : aucune ligne « Enabling … » vue pour ce plugin".into(),
            );
        }
        out["errors"] = groups_json(&check.errors, redact);
        out["warnings"] = groups_json(&check.warnings, redact);
        if !warnings_out.is_empty() {
            out["deploy_warnings"] = json!(warnings_out);
        }
        if let Some(d) = &check.done_line {
            out["done_line"] = json!(redact.apply(d));
        }
        if !check.other_errors.is_empty() {
            out["other_errors"] = json!({
                "count": check.other_errors.len(),
                "groups": groups_json(&check.other_errors[..check.other_errors.len().min(3)], redact),
            });
        }
        let tail: Vec<String> = lines
            .iter()
            .rev()
            .take(DEPLOY_TAIL)
            .rev()
            .map(|l| crate::util::truncate_on_boundary(&redact.apply(l), 400, "…"))
            .collect();
        out["console_tail"] = json!(tail);
        if let Some(w) = watcher {
            out["cursor"] = json!(w.cursor(w.with(|b| b.last_seq())));
        }
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        mcp_ctx::progress(
            STEPS,
            Some(STEPS),
            if ok {
                "déployé et vérifié"
            } else {
                "déployé : à vérifier"
            },
        )
        .await;
        pretty(&out)
    }

    /// Jars actifs de `dir` (hors `target`) qui déclarent le même plugin, et les noms présents.
    async fn same_plugin_jars(
        &self,
        id: i64,
        dir: &str,
        target: &str,
        ident: &Identity,
    ) -> crate::Result<(Vec<SftpEntry>, Vec<String>)> {
        use futures_util::stream::{self, StreamExt};
        let entries = match self.sftp_list(id, dir).await {
            Ok(e) => e,
            Err(e) if sftp::is_session_dead(&e) => return Err(e),
            // Dossier absent : rien à comparer (il sera créé à l'envoi).
            Err(_) => return Ok((Vec::new(), Vec::new())),
        };
        let taken: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        let candidates: Vec<SftpEntry> = entries
            .into_iter()
            .filter(|e| {
                !e.is_dir
                    && e.name.to_ascii_lowercase().ends_with(".jar")
                    && !e.name.eq_ignore_ascii_case(target)
            })
            .take(MAX_SIBLINGS)
            .collect();
        if candidates.is_empty() {
            return Ok((Vec::new(), taken));
        }
        let conn = self.sftp.ensure(&self.api, &self.token()?, id).await?;
        // Futures construites d'abord (voir plugin_deps.rs : appelant `Send`).
        let futures: Vec<_> = candidates
            .iter()
            .map(|c| Box::pin(remote_jar_meta(&conn, &c.path, c.size)))
            .collect();
        let metas: Vec<crate::Result<Option<JarMeta>>> =
            stream::iter(futures).buffered(4).collect().await;
        let same = |m: &JarMeta| {
            m.descriptors.iter().any(|d| {
                norm_id(&d.id) == norm_id(&ident.id)
                    && Some(d.platform.is_plugin()) == ident.platform.map(Platform::is_plugin)
            })
        };
        let siblings = candidates
            .into_iter()
            .zip(metas)
            .filter(|(_, m)| matches!(m, Ok(Some(m)) if same(m)))
            .map(|(c, _)| c)
            .collect();
        Ok((siblings, taken))
    }
}

/// Attend `d` en surveillant l'annulation. `false` si le client a annulé.
async fn sleep_or_cancel(d: Duration) -> bool {
    let deadline = Instant::now() + d;
    loop {
        if mcp_ctx::cancelled() {
            return false;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        tokio::time::sleep(left.min(POLL)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{ConsoleConnection, ConsoleOutput, ConsoleStats, ConsoleStatus};

    fn texts(lines: &[Line]) -> Vec<&str> {
        lines.iter().map(|l| l.text.as_str()).collect()
    }

    /// Tampon connecté (rejeu terminé par les stats puis le silence) avec `history`.
    fn connected(capacity: usize, history: &[&str]) -> ConsoleBuffer {
        let mut b = ConsoleBuffer::new(capacity);
        b.on_phase("connecting", 0);
        b.on_phase("open", 10);
        b.on_status("running", 20);
        for (i, l) in history.iter().enumerate() {
            b.on_line(l, 30 + i as i64);
        }
        b.on_stats("running", 60_000, 100);
        assert!(!b.ready(), "pas prêt avant le silence");
        assert!(b.on_tick(100 + REPLAY_QUIET_MS));
        assert!(b.ready());
        b
    }

    // --- Tampon -------------------------------------------------------------------------

    #[test]
    fn replayed_lines_are_history_then_live_lines_follow() {
        let mut b = connected(
            100,
            &[
                "[12:00:00 INFO]: ancien 1",
                "\u{1b}[33m[12:00:01 WARN]: ancien 2\u{1b}[0m",
            ],
        );
        b.on_line("[12:00:02 INFO]: nouveau", 500);
        let s = b.since(None).unwrap();
        assert_eq!(
            texts(&s.lines),
            [
                "[12:00:00 INFO]: ancien 1",
                "[12:00:01 WARN]: ancien 2",
                "[12:00:02 INFO]: nouveau"
            ]
        );
        assert_eq!(
            s.lines
                .iter()
                .map(|l| (l.seq, l.history))
                .collect::<Vec<_>>(),
            [(1, true), (2, true), (3, false)]
        );
        assert_eq!((s.missed, s.last_seq), (0, 3));
        // Depuis un curseur : seulement les suivantes.
        let s = b.since(Some(2)).unwrap();
        assert_eq!(texts(&s.lines), ["[12:00:02 INFO]: nouveau"]);
        assert!(b.since(Some(3)).unwrap().lines.is_empty());
        assert_eq!(b.state(), Some("running"));
        assert_eq!(b.live_state(), Some("running"));
    }

    #[test]
    fn replay_ends_after_grace_without_stats() {
        let mut b = ConsoleBuffer::new(10);
        b.on_phase("open", 0);
        b.on_line("a", 5);
        assert!(!b.on_tick(REPLAY_GRACE_MS - 1));
        assert!(b.on_tick(REPLAY_GRACE_MS));
        assert!(b.ready());
        assert!(b.since(None).unwrap().lines[0].history);
    }

    #[test]
    fn early_periodic_stats_do_not_cut_the_replay() {
        // Des stats périodiques arrivent avant le rejeu : il n'est terminé qu'après le silence.
        let mut b = ConsoleBuffer::new(10);
        b.on_phase("open", 0);
        b.on_stats("running", 1000, 10);
        b.on_line("rejouée 1", 40);
        assert!(!b.on_tick(200), "le rejeu continue");
        b.on_line("rejouée 2", 250);
        assert!(!b.on_tick(400));
        assert!(b.on_tick(250 + REPLAY_QUIET_MS));
        assert!(b.since(None).unwrap().lines.iter().all(|l| l.history));
    }

    #[test]
    fn overflow_is_reported_as_missed() {
        let mut b = connected(3, &[]);
        for i in 1..=5 {
            b.on_line(&format!("l{i}"), 1000);
        }
        let s = b.since(Some(0)).unwrap();
        assert_eq!(texts(&s.lines), ["l3", "l4", "l5"]);
        assert_eq!(s.missed, 2);
        let s = b.since(Some(1)).unwrap();
        assert_eq!(s.missed, 1);
        let s = b.since(Some(3)).unwrap();
        assert_eq!((texts(&s.lines), s.missed), (vec!["l4", "l5"], 0));
        // Sans curseur : ce qui reste, sans perte signalée.
        assert_eq!(b.since(None).unwrap().missed, 0);
    }

    #[test]
    fn cursor_beyond_the_last_line_is_invalid() {
        let mut b = connected(10, &["a"]);
        b.on_line("b", 500);
        assert!(b.since(Some(2)).is_ok());
        let e = b.since(Some(3)).unwrap_err();
        assert!(e.contains("curseur invalide"), "{e}");
    }

    #[test]
    fn multi_line_frames_are_split_and_cleaned() {
        let mut b = connected(10, &[]);
        b.on_line("un\r\ndeux\n", 500);
        b.on_line("progression 10%\rprogression 90%", 501);
        assert_eq!(
            texts(&b.since(None).unwrap().lines),
            ["un", "deux", "progression 90%"]
        );
        let long = "x".repeat(LINE_MAX_BYTES + 10);
        b.on_line(&long, 502);
        assert!(b.since(Some(3)).unwrap().lines[0].text.len() <= LINE_MAX_BYTES + "…".len());
    }

    #[test]
    fn reconnection_recovers_lines_without_duplicates() {
        let mut b = connected(100, &["a", "b", "c"]);
        b.on_line("d", 500);
        b.on_phase("reconnecting", 600);
        b.on_phase("connecting", 700);
        b.on_phase("open", 800);
        b.on_status("running", 810);
        // Wings rejoue ses dernières lignes, dont celles déjà reçues, et deux nouvelles.
        for (i, l) in ["b", "c", "d", "e", "f"].iter().enumerate() {
            b.on_line(l, 820 + i as i64);
        }
        b.on_stats("running", 1, 830);
        assert!(b.on_tick(830 + REPLAY_QUIET_MS));
        let s = b.since(Some(4)).unwrap();
        assert_eq!(texts(&s.lines), ["e", "f"]);
        assert!(s.lines.iter().all(|l| !l.history));
        assert!(s.gaps.is_empty());
    }

    #[test]
    fn reconnection_without_overlap_leaves_a_gap() {
        let mut b = connected(100, &["a", "b", "c"]);
        b.on_phase("reconnecting", 600);
        b.on_phase("open", 800);
        for l in ["x", "y", "z"] {
            b.on_line(l, 810);
        }
        b.on_stats("running", 1, 820);
        b.on_tick(820 + REPLAY_QUIET_MS);
        let s = b.since(Some(3)).unwrap();
        assert_eq!(texts(&s.lines), ["x", "y", "z"]);
        assert_eq!(s.gaps.len(), 1);
        assert_eq!(s.gaps[0].after_seq, 3);
        assert!(s.gaps[0].reason.contains("reconnexion"));
        // Un lecteur déjà au-delà ne la revoit pas.
        assert!(b.since(Some(4)).unwrap().gaps.is_empty());
    }

    #[test]
    fn reconnection_of_a_stopped_server_is_quiet() {
        let mut b = connected(100, &["a"]);
        b.on_status("offline", 300);
        b.on_phase("reconnecting", 600);
        b.on_phase("open", 800);
        b.on_status("offline", 805);
        b.on_stats("offline", 0, 810);
        b.on_tick(810 + REPLAY_QUIET_MS);
        assert!(b.since(Some(1)).unwrap().gaps.is_empty());
        // Il tournait : rien de rejoué = sa fin a pu être perdue.
        let mut b = connected(100, &["a"]);
        b.on_phase("reconnecting", 600);
        b.on_phase("open", 800);
        b.on_stats("offline", 0, 810);
        b.on_tick(810 + REPLAY_QUIET_MS);
        assert_eq!(b.since(Some(0)).unwrap().gaps.len(), 1);
    }

    #[test]
    fn bus_lag_is_reported() {
        let mut b = connected(10, &["a"]);
        b.on_lag(12);
        b.on_line("b", 500);
        let s = b.since(Some(1)).unwrap();
        assert_eq!(s.gaps.len(), 1);
        assert!(s.gaps[0].reason.contains("12"));
    }

    #[test]
    fn overlap_detection() {
        let r = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            new_lines_start(&["a", "b", "c", "d", "e"], &r(&["c", "d", "e", "f", "g"])),
            Some(3)
        );
        assert_eq!(
            new_lines_start(&["x", "a", "b", "c"], &r(&["a", "b", "c"])),
            Some(3)
        );
        // Sortie répétitive : la fenêtre entière lève l'ambiguïté.
        assert_eq!(
            new_lines_start(
                &["p", "q", "A", "B", "C"],
                &r(&["p", "q", "A", "B", "C", "X", "A", "B", "C"])
            ),
            Some(5)
        );
        assert_eq!(new_lines_start(&["a", "b", "c"], &r(&["x", "y"])), None);
        assert_eq!(new_lines_start(&["a", "b", "c"], &r(&[])), None);
        // Un recouvrement d'une seule ligne ne suffit pas quand on en connaît trois.
        assert_eq!(new_lines_start(&["a", "b", "c"], &r(&["c", "d"])), None);
        assert_eq!(new_lines_start(&["c"], &r(&["c", "d"])), Some(1));
        assert_eq!(new_lines_start(&[], &r(&["c"])), Some(0));
    }

    #[test]
    fn hibernation_reads_as_offline_and_ready() {
        let mut b = ConsoleBuffer::new(10);
        b.on_phase("connecting", 0);
        b.on_phase("hibernated", 10);
        assert!(b.ready());
        assert_eq!(b.live_state(), Some("offline"));
        assert!(b.unusable(true).is_none());
        // Les cycles suivants (connecting → hibernated) gardent l'état.
        b.on_phase("connecting", 15_000);
        assert_eq!(b.live_state(), Some("offline"));
    }

    #[test]
    fn suspension_is_terminal_and_unusable() {
        let mut b = ConsoleBuffer::new(10);
        b.on_phase("suspended", 10);
        assert!(b.unusable(false).unwrap().contains("suspendu"));
        let b = ConsoleBuffer::new(10);
        assert!(b.unusable(false).unwrap().contains("non établie"));
    }

    #[test]
    fn empty_replay_is_seeded_once_from_the_api() {
        let mut b = ConsoleBuffer::new(10);
        b.on_phase("open", 0);
        b.on_status("offline", 5);
        b.on_stats("offline", 0, 10);
        b.on_tick(10 + REPLAY_QUIET_MS);
        assert!(b.needs_seed());
        b.seed_history(vec!["[12:00 ERROR]: crash".into(), "fin".into()]);
        let s = b.since(None).unwrap();
        assert_eq!(texts(&s.lines), ["[12:00 ERROR]: crash", "fin"]);
        assert!(s.lines.iter().all(|l| l.history));
        assert!(!b.needs_seed());
        b.seed_history(vec!["encore".into()]);
        assert_eq!(b.since(None).unwrap().lines.len(), 2);
    }

    #[test]
    fn state_changes_are_recorded_with_their_position() {
        let mut b = connected(10, &["a"]);
        b.on_status("stopping", 1000);
        b.on_line("Stopping server", 1001);
        b.on_status("offline", 1100);
        b.on_status("starting", 1200);
        b.on_line("Starting minecraft server", 1201);
        b.on_status("starting", 1250);
        let c = b.changes_since(1000);
        assert_eq!(
            c.iter().map(|c| c.state.as_str()).collect::<Vec<_>>(),
            ["stopping", "offline", "starting"]
        );
        assert_eq!(
            c[2].after_seq, 2,
            "les lignes du démarrage suivent la ligne 2"
        );
        assert_eq!(normalize_state("ONLINE"), "running");
        assert_eq!(normalize_state("stopped"), "offline");
        assert_eq!(normalize_state(""), "unknown");
    }

    // --- Curseurs et motifs ---------------------------------------------------------------

    #[test]
    fn cursors_round_trip_and_reject_garbage() {
        let c = parse_cursor(Some(&json!("k3x9-42"))).unwrap().unwrap();
        assert_eq!(
            c,
            Cursor {
                epoch: Some("k3x9".into()),
                seq: 42
            }
        );
        assert_eq!(
            parse_cursor(Some(&json!(format_cursor("ab12", 7))))
                .unwrap()
                .unwrap()
                .seq,
            7
        );
        assert_eq!(
            parse_cursor(Some(&json!(12))).unwrap().unwrap(),
            Cursor {
                epoch: None,
                seq: 12
            }
        );
        assert_eq!(
            parse_cursor(Some(&json!("12"))).unwrap().unwrap().epoch,
            None
        );
        assert_eq!(parse_cursor(None).unwrap(), None);
        assert_eq!(parse_cursor(Some(&json!(""))).unwrap(), None);
        assert_eq!(parse_cursor(Some(&Value::Null)).unwrap(), None);
        for bad in [
            json!("abc"),
            json!("k3-x"),
            json!("k-3-x"),
            json!("é-3"),
            json!(-1),
            json!(1.5),
            json!([1]),
        ] {
            assert!(parse_cursor(Some(&bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn cursors_are_resolved_against_the_current_buffer() {
        let cur = |e: Option<&str>, seq| Cursor {
            epoch: e.map(str::to_string),
            seq,
        };
        assert_eq!(resolve_cursor(None, "ab", 10), Ok(Start::Latest));
        assert_eq!(
            resolve_cursor(Some(&cur(Some("ab"), 4)), "ab", 10),
            Ok(Start::After(4))
        );
        assert_eq!(
            resolve_cursor(Some(&cur(None, 10)), "ab", 10),
            Ok(Start::After(10))
        );
        // Tampon recréé (autre époque) : reprise, pas d'erreur.
        assert_eq!(
            resolve_cursor(Some(&cur(Some("zz"), 4000)), "ab", 10),
            Ok(Start::Reset)
        );
        assert!(resolve_cursor(Some(&cur(Some("ab"), 11)), "ab", 10).is_err());
        assert!(resolve_cursor(Some(&cur(None, 11)), "ab", 10).is_err());
    }

    #[test]
    fn epochs_are_distinct() {
        let a = new_epoch();
        let b = new_epoch();
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_eq!(base36(0), "0");
        assert_eq!(base36(35), "z");
        assert_eq!(base36(36), "10");
    }

    #[test]
    fn patterns_are_literal_by_default_and_case_insensitive() {
        let re = compile_pattern("Done (", false, "until").unwrap();
        assert!(re.is_match("[12:00 INFO]: done (3.2s)!"));
        let re = compile_pattern(r"There are \d+ of a max", true, "until").unwrap();
        assert!(re.is_match("There are 3 of a max of 20 players online"));
        assert!(!compile_pattern(r"There are \d+", false, "until")
            .unwrap()
            .is_match("There are 3"));
        assert!(compile_pattern("(", true, "until")
            .unwrap_err()
            .contains("until"));
        assert!(compile_pattern("  ", false, "filter")
            .unwrap_err()
            .contains("filter"));
        assert!(arg_pattern(&json!({ "filter": 3 }), "filter").is_err());
        assert!(arg_pattern(&json!({ "filter": "" }), "filter")
            .unwrap()
            .is_none());
        assert!(
            arg_pattern(&json!({ "filter": "a.b", "regex": true }), "filter")
                .unwrap()
                .unwrap()
                .is_match("axb")
        );
    }

    #[test]
    fn delays_are_capped() {
        let d = |r: Option<f64>, l: Limits, c: bool| cap_secs(r, l, c).as_secs_f64();
        assert_eq!(d(None, RUN_TIMEOUT, false), 5.0);
        assert_eq!(d(Some(120.0), RUN_TIMEOUT, false), 60.0);
        assert_eq!(d(Some(2.5), RUN_TIMEOUT, true), 2.5);
        assert_eq!(d(None, WAIT_TIMEOUT, false), 60.0);
        assert_eq!(d(Some(900.0), WAIT_TIMEOUT, false), 600.0);
        assert_eq!(
            d(Some(900.0), WAIT_TIMEOUT, true),
            120.0,
            "Copilote : plafond plus bas"
        );
        assert_eq!(d(Some(-3.0), WAIT_TIMEOUT, false), 60.0);
        assert_eq!(d(Some(f64::NAN), TAIL_WAIT, false), 0.0);
        assert_eq!(d(Some(45.0), TAIL_WAIT, true), 30.0);
        assert_eq!(d(None, DEPLOY_TIMEOUT, false), 180.0);
        assert_eq!(
            arg_count(&json!({ "max_lines": 0 }), "max_lines", 100, 1000),
            1
        );
        assert_eq!(
            arg_count(&json!({ "max_lines": 5000 }), "max_lines", 100, 1000),
            1000
        );
        assert_eq!(arg_count(&json!({}), "max_lines", 100, 1000), 100);
    }

    // --- Sélection des lignes ----------------------------------------------------------------

    fn lines(v: &[&str]) -> Vec<Line> {
        v.iter()
            .enumerate()
            .map(|(i, t)| Line {
                seq: i as u64 + 1,
                text: t.to_string(),
                history: i == 0,
            })
            .collect()
    }

    #[test]
    fn picker_filters_caps_and_keeps_the_cursor_on_the_last_consumed_line() {
        let ls = lines(&["a ERROR", "b", "c ERROR", "d ERROR"]);
        let re = compile_pattern("error", false, "filter").unwrap();
        let mut p = Picker::new(Some(&re), 2, Redactor(false), 0);
        p.feed(&ls);
        assert_eq!(p.lines, ["a ERROR", "c ERROR"]);
        assert!(p.truncated);
        assert_eq!(p.cursor, 3, "reprise après la dernière ligne rendue");
        assert_eq!(p.history, 1);
        // Sans filtre ni plafond atteint : curseur à la dernière ligne.
        let mut p = Picker::new(None, 10, Redactor(false), 0);
        p.feed(&ls);
        assert_eq!((p.lines.len(), p.cursor, p.truncated), (4, 4, false));
        // Anonymisation avant le filtre (le modèle filtre ce qu'il voit).
        let ls = lines(&["connexion de 192.168.1.20"]);
        let ip = compile_pattern("192.168", false, "filter").unwrap();
        let mut p = Picker::new(Some(&ip), 10, Redactor(true), 0);
        p.feed(&ls);
        assert!(p.lines.is_empty() && p.cursor == 1);
    }

    #[test]
    fn latest_lines_respect_the_budget() {
        let big = "x".repeat(OUT_LINE_BYTES + 50);
        let many: Vec<&str> = std::iter::repeat_n(big.as_str(), 60).collect();
        let (out, _, omitted) = pick_latest(&lines(&many), None, 1000, Redactor(false));
        assert!(omitted && out.len() < 60);
        assert!(out.iter().map(String::len).sum::<usize>() <= OUT_BUDGET + OUT_LINE_BYTES);
        let (out, history, omitted) =
            pick_latest(&lines(&["a", "b", "c"]), None, 2, Redactor(false));
        assert_eq!(
            (out, history, omitted),
            (vec!["b".to_string(), "c".to_string()], 0, true)
        );
    }

    // --- Attentes ----------------------------------------------------------------------------

    fn change(state: &str, at_ms: i64) -> StateChange {
        StateChange {
            after_seq: 0,
            state: state.into(),
            at_ms,
        }
    }

    fn obs<'a>(
        state: &'a str,
        initial: &'a str,
        changes: &'a [StateChange],
        start: Option<i64>,
        mark: Option<i64>,
    ) -> Observation<'a> {
        Observation {
            state,
            initial,
            changes,
            process_start_ms: start,
            mark_ms: mark,
            since_ms: 10_000,
        }
    }

    #[test]
    fn running_right_after_a_restart_is_stale() {
        // Pas d'action récente : « running » suffit.
        assert_eq!(
            judge_state("running", &obs("running", "running", &[], None, None)),
            Judgement::Reached
        );
        // Redémarrage demandé à 9 s : le « running » d'avant ne compte pas…
        let mark = Some(9_000);
        assert_eq!(
            judge_state(
                "running",
                &obs("running", "running", &[], Some(1_000), mark)
            ),
            Judgement::Pending
        );
        // … mais oui après un passage par un autre état…
        let seen = [
            change("stopping", 9_500),
            change("offline", 12_000),
            change("starting", 13_000),
            change("running", 30_000),
        ];
        assert_eq!(
            judge_state("running", &obs("running", "running", &seen, None, mark)),
            Judgement::Reached
        );
        // … ou si le processus a démarré après la demande (redémarrage fini avant l'attente).
        assert_eq!(
            judge_state(
                "running",
                &obs("running", "running", &[], Some(20_000), mark)
            ),
            Judgement::Reached
        );
        // Un changement antérieur à la demande ne compte pas.
        let old = [change("starting", 1_000)];
        assert_eq!(
            judge_state(
                "running",
                &obs("running", "running", &old, Some(1_000), mark)
            ),
            Judgement::Pending
        );
    }

    #[test]
    fn a_crash_during_startup_fails_fast() {
        let seen = [change("starting", 11_000), change("offline", 20_000)];
        assert!(matches!(
            judge_state("running", &obs("offline", "offline", &seen, None, None)),
            Judgement::Failed(_)
        ));
        // Déjà en démarrage au début de l'attente, puis éteint.
        let seen = [change("offline", 20_000)];
        assert!(matches!(
            judge_state("running", &obs("offline", "starting", &seen, None, None)),
            Judgement::Failed(_)
        ));
        // Arrêt DEMANDÉ après le démarrage : pas un plantage, on attend.
        let seen = [
            change("starting", 11_000),
            change("stopping", 15_000),
            change("offline", 20_000),
        ];
        assert_eq!(
            judge_state("running", &obs("offline", "offline", &seen, None, None)),
            Judgement::Pending
        );
        // Arrêt du redémarrage (avant « starting ») : normal.
        let seen = [change("stopping", 11_000), change("offline", 12_000)];
        assert_eq!(
            judge_state(
                "running",
                &obs("offline", "running", &seen, None, Some(9_000))
            ),
            Judgement::Pending
        );
    }

    #[test]
    fn other_targets() {
        assert_eq!(
            judge_state("offline", &obs("offline", "running", &[], None, None)),
            Judgement::Reached
        );
        assert_eq!(
            judge_state("offline", &obs("stopping", "running", &[], None, None)),
            Judgement::Pending
        );
        // État transitoire franchi entre deux observations : atteint quand même.
        let seen = [change("starting", 11_000), change("running", 12_000)];
        assert_eq!(
            judge_state("starting", &obs("running", "offline", &seen, None, None)),
            Judgement::Reached
        );
        assert_eq!(
            judge_state("stopping", &obs("running", "running", &[], None, None)),
            Judgement::Pending
        );
    }

    fn watcher_with(lines: &[&str]) -> Watcher {
        let w = Watcher::new(7, 0);
        let conn = w.conn_id.clone();
        w.apply(
            &CoreEvent::ConsoleConnection(ConsoleConnection {
                conn_id: conn.clone(),
                phase: "open".into(),
            }),
            0,
        );
        w.apply(
            &CoreEvent::ConsoleStatus(ConsoleStatus {
                conn_id: conn.clone(),
                state: "running".into(),
            }),
            1,
        );
        for l in lines {
            w.apply(
                &CoreEvent::ConsoleOutput(ConsoleOutput {
                    conn_id: conn.clone(),
                    line: l.to_string(),
                }),
                2,
            );
        }
        let stats = ConsoleStats {
            conn_id: conn,
            cpu_absolute: 0.0,
            memory_bytes: 0,
            memory_limit_bytes: 0,
            disk_bytes: 0,
            uptime: 5000,
            state: "running".into(),
        };
        w.apply(&CoreEvent::ConsoleStats(stats), 3);
        w.with(|b| b.on_tick(3 + REPLAY_QUIET_MS));
        w
    }

    #[test]
    fn events_of_other_connections_are_ignored() {
        let w = watcher_with(&["a"]);
        let other = CoreEvent::ConsoleOutput(ConsoleOutput {
            conn_id: "tab-1".into(),
            line: "intrus".into(),
        });
        assert_eq!(w.apply(&other, 10), Flow::Ignored);
        assert_eq!(w.with(|b| b.last_seq()), 1);
        assert_eq!(w.with(|b| b.process_start_ms), Some(3 - 5000));
        let closed = CoreEvent::ConsoleConnection(ConsoleConnection {
            conn_id: w.conn_id.clone(),
            phase: "closed".into(),
        });
        assert_eq!(w.apply(&closed, 20), Flow::Stop);
        let mine = CoreEvent::ConsoleConnection(ConsoleConnection {
            conn_id: "agent:7:autre".into(),
            phase: "closed".into(),
        });
        assert_eq!(w.apply(&mine, 20), Flow::Ignored);
    }

    #[tokio::test]
    async fn run_command_collects_until_the_pattern() {
        let w = Arc::new(watcher_with(&["ancienne"]));
        let feeder = {
            let w = w.clone();
            tokio::spawn(async move {
                for l in [
                    "There are 2 of a max of 20 players online: Alex, Steve",
                    "après",
                ] {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    let conn = w.conn_id.clone();
                    w.apply(
                        &CoreEvent::ConsoleOutput(ConsoleOutput {
                            conn_id: conn,
                            line: l.into(),
                        }),
                        now_ms(),
                    );
                    w.bump();
                }
            })
        };
        let until = compile_pattern(r"There are \d+ of a max", true, "until").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let got = collect_after(&w, 1, Some(&until), None, deadline, 100, Redactor(false)).await;
        feeder.await.unwrap();
        assert_eq!(got.matched.as_ref().map(|m| m.seq), Some(2));
        assert_eq!(
            texts(&got.lines),
            ["There are 2 of a max of 20 players online: Alex, Steve"]
        );
        assert!(!got.deadline_hit);
        // La ligne suivante reste à lire après le curseur rendu.
        assert_eq!(
            texts(&w.with(|b| b.since(Some(2)).unwrap().lines)),
            ["après"]
        );
    }

    #[tokio::test]
    async fn collection_stops_on_silence_or_deadline() {
        let w = watcher_with(&[]);
        // Sans sortie : échéance.
        let t = Instant::now();
        let got = collect_after(
            &w,
            0,
            None,
            Some(QUIET),
            Instant::now() + Duration::from_millis(300),
            100,
            Redactor(false),
        )
        .await;
        assert!(got.deadline_hit && got.lines.is_empty());
        assert!(t.elapsed() >= Duration::from_millis(250));
        // Une ligne puis le silence : rend la main avant l'échéance.
        w.with(|b| b.on_line("Unknown command. Type \"/help\" for help.", now_ms()));
        let t = Instant::now();
        let got = collect_after(
            &w,
            0,
            None,
            Some(Duration::from_millis(100)),
            Instant::now() + Duration::from_secs(5),
            100,
            Redactor(false),
        )
        .await;
        assert!(!got.deadline_hit && got.lines.len() == 1);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    // --- Démarrage et déploiement ------------------------------------------------------------

    fn owned(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn ident(name: &str, id: &str, version: &str, platform: Platform, file: &str) -> Identity {
        Identity {
            name: name.into(),
            id: id.into(),
            version: version.into(),
            platform: Some(platform),
            file: file.into(),
        }
    }

    const PAPER_OK: &str = "\
[12:00:00 INFO]: Starting minecraft server version 1.21.1
[12:00:01 INFO]: [MyPlugin] Loading server plugin MyPlugin v1.2.3
[12:00:02 WARN]: [OtherPlugin] Config option deprecated
[12:00:03 INFO]: [MyPlugin] Enabling MyPlugin v1.2.3
[12:00:04 WARN]: [MyPlugin] No database configured, using SQLite
[12:00:05 INFO]: Done (4.512s)! For help, type \"help\"";

    #[test]
    fn paper_plugin_enabled() {
        let id = ident(
            "MyPlugin",
            "MyPlugin",
            "1.2.3",
            Platform::Paper,
            "MyPlugin-1.2.3.jar",
        );
        let c = check_deploy(&owned(PAPER_OK), &id, true);
        assert_eq!(c.enabled, Some(true));
        assert!(c
            .evidence
            .as_deref()
            .unwrap()
            .contains("Enabling MyPlugin v1.2.3"));
        assert_eq!(c.loaded_version.as_deref(), Some("1.2.3"));
        assert!(c.errors.is_empty());
        assert_eq!(c.warnings.len(), 1, "seul l'avertissement de MyPlugin");
        assert_eq!(c.reported_secs, Some(4.512));
        assert!(c.ok(&id, true, true));
        // Démarrage non atteint : jamais ok.
        assert!(!c.ok(&id, false, true));
    }

    #[test]
    fn spigot_missing_dependency() {
        let log = "\
[12:00:01 INFO]: [Essentials] Loading Essentials v2.20.1
[12:00:01 ERROR]: Could not load 'plugins/MyPlugin-1.2.3.jar' in folder 'plugins'
org.bukkit.plugin.UnknownDependencyException: Unknown dependency Vault. Please download and install Vault to run this plugin.
\tat org.bukkit.plugin.SimplePluginManager.loadPlugins(SimplePluginManager.java:286) ~[spigot-api-1.20.4.jar:?]
\tat org.bukkit.craftbukkit.v1_20_R3.CraftServer.loadPlugins(CraftServer.java:435) ~[spigot-1.20.4.jar:?]
[12:00:03 INFO]: [Essentials] Enabling Essentials v2.20.1*
[12:00:05 INFO]: Done (6.100s)! For help, type \"help\"";
        let id = ident(
            "MyPlugin",
            "MyPlugin",
            "1.2.3",
            Platform::Bukkit,
            "MyPlugin-1.2.3.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert_eq!(c.enabled, Some(false));
        assert!(c.evidence.as_deref().unwrap().contains("Could not load"));
        assert_eq!(c.errors.len(), 1, "{:#?}", c.errors);
        assert!(c.other_errors.is_empty());
        assert!(!c.ok(&id, true, true));
        // Essentials : version marquée `*` (sans api-version) lue sans l'astérisque.
        let ess = ident(
            "Essentials",
            "Essentials",
            "2.20.1",
            Platform::Bukkit,
            "EssentialsX-2.20.1.jar",
        );
        let c = check_deploy(&owned(log), &ess, true);
        assert_eq!(
            (c.enabled, c.loaded_version.as_deref()),
            (Some(true), Some("2.20.1"))
        );
        assert!(
            c.errors.is_empty(),
            "l'erreur de MyPlugin n'est pas celle d'Essentials"
        );
        assert_eq!(c.other_errors.len(), 1);
    }

    #[test]
    fn paper_enable_error_with_stack_trace() {
        let log = "\
[12:00:03 INFO]: [MyPlugin] Enabling MyPlugin v1.2.3
[12:00:03 ERROR]: Error occurred while enabling MyPlugin v1.2.3 (Is it up to date?)
java.lang.NullPointerException: Cannot invoke \"org.bukkit.World.getName()\" because \"world\" is null
\tat com.example.myplugin.MyPlugin.onEnable(MyPlugin.java:42) ~[MyPlugin-1.2.3.jar:?]
\tat org.bukkit.plugin.java.JavaPlugin.setEnabled(JavaPlugin.java:288) ~[paper-api-1.21.1.jar:?]
[12:00:03 INFO]: [MyPlugin] Disabling MyPlugin v1.2.3
[12:00:05 INFO]: Done (4.000s)! For help, type \"help\"";
        let id = ident(
            "MyPlugin",
            "MyPlugin",
            "1.2.3",
            Platform::Paper,
            "MyPlugin-1.2.3.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert_eq!(c.enabled, Some(false));
        assert_eq!(c.errors.len(), 1);
        assert_eq!(
            c.errors[0]
                .probable_source
                .as_ref()
                .map(|s| s.name.as_str()),
            Some("MyPlugin")
        );
        assert!(!c.ok(&id, true, true));
    }

    #[test]
    fn duplicate_jar_and_old_version_loaded() {
        let log = "\
[12:00:01 ERROR]: Ambiguous plugin name `MyPlugin' for files `plugins/MyPlugin-1.2.3.jar' and `plugins/myplugin-old.jar' in `plugins'
[12:00:03 INFO]: [MyPlugin] Enabling MyPlugin v1.0.0
[12:00:05 INFO]: Done (4.000s)! For help, type \"help\"";
        let id = ident(
            "MyPlugin",
            "MyPlugin",
            "1.2.3",
            Platform::Paper,
            "MyPlugin-1.2.3.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert!(c.duplicate.is_some());
        assert_eq!(c.enabled, Some(true));
        assert_eq!(c.version_mismatch(&id).as_deref(), Some("1.0.0"));
        assert!(!c.ok(&id, true, true));
    }

    #[test]
    fn plugin_without_any_line_is_not_confirmed() {
        let log = "[12:00:05 INFO]: Done (4.000s)! For help, type \"help\"";
        let id = ident(
            "MyPlugin",
            "MyPlugin",
            "1.2.3",
            Platform::Paper,
            "MyPlugin-1.2.3.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert_eq!(c.enabled, None);
        assert!(!c.ok(&id, true, true));
        // Jar sans descripteur : rien à confirmer, seul le démarrage compte.
        let unknown = Identity {
            name: String::new(),
            id: String::new(),
            version: String::new(),
            platform: None,
            file: "lib.jar".into(),
        };
        assert!(check_deploy(&owned(log), &unknown, true).ok(&unknown, true, false));
    }

    #[test]
    fn fabric_mod_listed_and_errors_of_others_ignored() {
        let log = "\
[12:00:00] [main/INFO]: Loading Minecraft 1.20.4 with Fabric Loader 0.15.7
[12:00:00] [main/INFO]: Loading 4 mods:
\t- fabric-api 0.96.4+1.20.4
\t   |-- fabric-api-base 0.4.37+78d798af4f
\t- java 17
\t- minecraft 1.20.4
\t- mymod 1.0.0
[12:00:05] [Server thread/ERROR]: [lithium] Failed to apply patch
[12:00:06] [Server thread/INFO]: Done (6.000s)! For help, type \"help\"";
        let id = ident(
            "My Mod",
            "mymod",
            "1.0.0",
            Platform::Fabric,
            "mymod-1.0.0.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert_eq!(c.enabled, Some(true));
        assert!(!c.deduced);
        assert_eq!(c.loaded_version.as_deref(), Some("1.0.0"));
        assert!(c.errors.is_empty());
        assert_eq!(c.other_errors.len(), 1);
        assert!(c.ok(&id, true, true));
    }

    #[test]
    fn fabric_incompatible_mod_set_is_attributed() {
        let log = "\
[12:00:00] [main/INFO]: Loading Minecraft 1.20.4 with Fabric Loader 0.15.7
[12:00:01] [main/ERROR]: Incompatible mods found!
net.fabricmc.loader.impl.FormattedException: Some of your mods are incompatible with the game or each other!
A potential solution has been determined:
\t - Install fabric-api, version 0.92.0 or later.
More details:
\t - Mod 'My Mod' (mymod) 1.0.0 requires version 0.92.0 or later of fabric-api, which is missing!";
        let id = ident(
            "My Mod",
            "mymod",
            "1.0.0",
            Platform::Fabric,
            "mymod-1.0.0.jar",
        );
        let c = check_deploy(&owned(log), &id, false);
        assert_eq!(c.enabled, Some(false));
        assert_eq!(c.errors.len(), 1, "{:#?}", c.errors);
        assert!(!c.ok(&id, false, true));
    }

    #[test]
    fn forge_mod_is_deduced_when_the_server_starts() {
        let log = "\
[12:00:00] [main/INFO] [net.minecraftforge.fml.loading.FMLLoader/CORE]: ModLauncher running
[12:00:10] [modloading-worker-0/INFO] [mymod/]: Hello from My Mod
[12:00:12] [Server thread/ERROR] [create/]: Belt network desync
[12:00:20] [Server thread/INFO] [minecraft/DedicatedServer]: Done (20.000s)! For help, type \"help\"";
        let id = ident(
            "My Mod",
            "mymod",
            "1.0.0",
            Platform::Forge,
            "mymod-1.20.1-1.0.0.jar",
        );
        let c = check_deploy(&owned(log), &id, true);
        assert_eq!(c.enabled, Some(true));
        assert!(c.deduced);
        assert!(
            c.errors.is_empty(),
            "l'erreur de Create n'est pas celle du mod"
        );
        assert_eq!(c.other_errors.len(), 1);
        assert!(c.ok(&id, true, true));
        // Le serveur n'a pas démarré : rien n'est déduit.
        assert_eq!(check_deploy(&owned(log), &id, false).enabled, None);
    }

    #[test]
    fn forge_failed_mod_instance_is_attributed() {
        let log = "\
[12:00:05] [main/ERROR] [net.minecraftforge.fml.javafmlmod.FMLModContainer/LOADING]: Failed to create mod instance. ModID: mymod, class com.example.mymod.MyMod
java.lang.NoClassDefFoundError: com/example/missing/Api
\tat TRANSFORMER/mymod@1.0.0/com.example.mymod.MyMod.<init>(MyMod.java:20) ~[mymod-1.20.1-1.0.0.jar%23190!/:1.0.0]";
        let id = ident(
            "My Mod",
            "mymod",
            "1.0.0",
            Platform::Forge,
            "mymod-1.20.1-1.0.0.jar",
        );
        let c = check_deploy(&owned(log), &id, false);
        assert_eq!(c.enabled, Some(false));
        assert_eq!(c.errors.len(), 1);
    }

    #[test]
    fn proxies_announce_their_plugins() {
        let id = ident(
            "MyProxy",
            "myproxy",
            "2.0",
            Platform::Velocity,
            "myproxy-2.0.jar",
        );
        let c = check_deploy(
            &owned("[12:00:01 INFO]: Loaded plugin myproxy 2.0 by Moi"),
            &id,
            true,
        );
        assert_eq!(
            (c.enabled, c.loaded_version.as_deref()),
            (Some(true), Some("2.0"))
        );
        let id = ident(
            "MyBungee",
            "MyBungee",
            "3.1",
            Platform::Bungee,
            "MyBungee.jar",
        );
        let c = check_deploy(
            &owned("12:00:01 [INFO] Enabled plugin MyBungee version 3.1 by Moi"),
            &id,
            true,
        );
        assert_eq!(
            (c.enabled, c.loaded_version.as_deref()),
            (Some(true), Some("3.1"))
        );
    }

    #[test]
    fn startup_summary_groups_errors_and_reads_done() {
        let s = startup_digest(&owned(
            "[12:00:01 ERROR]: [Foo] boom\n[12:00:02 ERROR]: [Foo] boom\n[12:00:03 WARN]: lent\n[12:00:09 INFO]: Done (8,25s)! For help, type \"help\"",
        ));
        assert_eq!(s.errors.len(), 1);
        assert_eq!(s.errors[0].count, 2);
        assert_eq!(s.warnings.len(), 1);
        assert_eq!(s.reported_secs, Some(8.25));
        assert!(s.done_line.unwrap().contains("Done"));
        let s = startup_digest(&[]);
        assert!(s.done_line.is_none() && s.errors.is_empty());
    }

    #[test]
    fn matching_helpers() {
        let id = ident(
            "Core Protect",
            "CoreProtect",
            "22.4",
            Platform::Bukkit,
            "CoreProtect-22.4.jar",
        );
        assert!(key_match("CoreProtect", &id.keys()));
        assert!(key_match("net.coreprotect", &id.keys()));
        assert!(!key_match("Essentials", &id.keys()));
        assert!(id.named("core_protect") && id.named("COREPROTECT"));
        assert!(contains_token(
            "at com.example.coreprotect.api",
            "coreprotect"
        ));
        assert!(!contains_token("mycoreprotectx", "coreprotect"));
        assert_eq!(
            split_version("My Plugin v1.0*"),
            ("My Plugin", Some("1.0".to_string()))
        );
        assert_eq!(
            split_version("SuperVanish vdev-12"),
            ("SuperVanish", Some("dev-12".to_string()))
        );
        assert_eq!(split_version("Alone"), ("Alone", None));
        assert_eq!(
            untag("[MyPlugin] Enabling MyPlugin v1"),
            "Enabling MyPlugin v1"
        );
        assert_eq!(untag("Enabling"), "Enabling");
    }

    // --- Jars --------------------------------------------------------------------------------

    #[test]
    fn local_jar_descriptor_and_destination() {
        use crate::jar_meta::tests::make_jar;
        let bytes = make_jar(&[(
            "plugin.yml",
            "name: MyPlugin\nversion: 1.2.3\nmain: com.example.MyPlugin\n".as_bytes(),
            true,
        )]);
        let meta = jar_meta_of(&bytes).unwrap();
        let d = pick_descriptor(&meta, None, None).unwrap();
        assert_eq!(
            (d.name.as_str(), d.version.as_str(), d.platform),
            ("MyPlugin", "1.2.3", Platform::Bukkit)
        );
        assert!(jar_meta_of(b"pas un zip").is_err());
        // Jar universel : le descripteur de la plateforme du serveur l'emporte.
        let bytes = make_jar(&[
            (
                "plugin.yml",
                "name: Uni\nversion: 1\nmain: a.B\n".as_bytes(),
                true,
            ),
            (
                "fabric.mod.json",
                r#"{"schemaVersion":1,"id":"uni","version":"1"}"#.as_bytes(),
                true,
            ),
        ]);
        let meta = jar_meta_of(&bytes).unwrap();
        let fabric: std::collections::BTreeSet<Platform> = [Platform::Fabric].into_iter().collect();
        assert_eq!(
            pick_descriptor(&meta, Some(&fabric), None)
                .unwrap()
                .platform,
            Platform::Fabric
        );
        assert!(pick_descriptor(&meta, None, Some(true))
            .unwrap()
            .platform
            .is_plugin());

        assert_eq!(resolve_remote(None, "a.jar", "/plugins"), "/plugins/a.jar");
        assert_eq!(
            resolve_remote(Some("mods/"), "a.jar", "/plugins"),
            "/mods/a.jar"
        );
        assert_eq!(
            resolve_remote(Some("/plugins/b.jar"), "a.jar", "/plugins"),
            "/plugins/b.jar"
        );
        assert_eq!(old_name("X.jar", &[]), "X.jar.old");
        assert_eq!(old_name("X.jar", &["x.jar.old".into()]), "X.jar.2.old");
    }
}
