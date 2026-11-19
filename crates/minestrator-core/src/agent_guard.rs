//! Garde des agents MCP : journal de leurs actions, annulation de leurs propres modifications et
//! droits selon l'origine des serveurs.
//!
//! Trois mécanismes, branchés dans `mcp.rs` :
//!
//! - **Journal** ([`protocol_dispatch`], appelé par `mcp::call`) : chaque appel d'outil reçu par
//!   le protocole MCP (clients externes, agents CLI du Copilote) est consigné dans
//!   `agent_guard.db` : horodatage, session, client, outil, serveurs visés, arguments résumés et
//!   **anonymisés** (`redact.rs`, contenus remplacés par leur taille), résultat et durée. Lectures
//!   et écritures sont journalisées ; les lectures sont plus courtes et gardées moins longtemps
//!   (7 jours contre 30). Les refus de la politique générale qui précèdent la garde dans `call()`
//!   (outil inconnu, lecture seule, liste blanche d'env, portée du Copilote) n'y figurent pas.
//! - **Changesets** ([`before_write`] / [`after_write`], appelés par `mcp::dispatch` pour tout
//!   outil modifiant, donc aussi pour le Copilote de l'app) : avant l'écriture, la garde copie de
//!   quoi revenir en arrière ; après, elle constate l'état et enregistre une **étape** avec ses
//!   opérations inverses. Les étapes d'une même session forment un changeset (session MCP, ou
//!   `copilote:<serveur>:<jour>` pour le Copilote). Capture ratée ou plafond dépassé : l'écriture
//!   a lieu, l'étape est « non annulable » et la sortie de l'outil le dit.
//! - **Mode constructeur** ([`admit`], appelé par `mcp::call` avant le journal) : en portée
//!   `builder`, les outils modifiants ne sont permis que sur les serveurs créés pendant la session
//!   MCP ou listés dans `allowed_servers` (voir [`scope`]).
//!
//! Point d'extension pour un nouvel outil modifiant : une arme dans [`undo::plan_for`] ; pour un
//! outil qui crée un serveur : [`record_created_server`] après succès et une règle
//! `Targets::Create` dans [`scope::write_targets`].

mod scope;
mod store;
// Visible dans la crate : `Meta`, `sha_hex` et le faux serveur des tests servent aussi aux outils
// qui envoient depuis l'ordinateur (`local_sync`, `world_upload`).
pub(crate) mod undo;

pub use scope::{WriteScope, MCP_ALLOWED_SERVERS_ENV, MCP_WRITE_SCOPE_ENV};

use crate::error::{Error, Result};
use crate::mcp::McpConfig;
use crate::mcp_ctx::{self, Confirmation};
use crate::store::now_secs;
use crate::Core;
use scope::Targets;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use store::{GuardStore, NewAction, NewStep};
use undo::{Budget, Capture, Meta, Plan, Pre, Remote, UndoError, UndoOp};

/// Délai maximal d'une capture (avant) ou d'un constat (après) : au-delà, étape non annulable.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

// --- Types exposés -----------------------------------------------------------------------------

/// Résultat d'un appel journalisé.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ActionOutcome {
    Ok,
    Error,
    /// Refusé par la garde (mode constructeur).
    Denied,
}

impl ActionOutcome {
    fn as_str(self) -> &'static str {
        match self {
            ActionOutcome::Ok => "ok",
            ActionOutcome::Error => "error",
            ActionOutcome::Denied => "denied",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "ok" => ActionOutcome::Ok,
            "denied" => ActionOutcome::Denied,
            _ => ActionOutcome::Error,
        }
    }
}

/// Un appel d'outil reçu par le serveur MCP, tel que journalisé.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentAction {
    pub id: i64,
    /// Début de l'appel (secondes Unix).
    pub ts: i64,
    /// Session (et changeset) de l'appel : `mcp:…` ou `copilote:<serveur>:<jour>`.
    pub session: String,
    /// Nom du client MCP, s'il est connu.
    pub client: Option<String>,
    pub tool: String,
    /// Outil modifiant.
    pub write: bool,
    /// Serveurs désignés par les arguments.
    pub servers: Vec<i64>,
    /// Arguments résumés et anonymisés (valeurs tronquées, contenus remplacés par leur taille).
    pub args: Value,
    pub outcome: ActionOutcome,
    /// Motif court d'une erreur ou d'un refus (anonymisé).
    pub detail: Option<String>,
    pub duration_ms: i64,
    /// Étape de changeset créée par l'appel, s'il y en a une.
    pub changeset: Option<String>,
    pub step: Option<i64>,
}

/// Filtre du journal (tous les champs sont facultatifs).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AgentActionQuery {
    #[serde(default)]
    pub server_id: Option<i64>,
    #[serde(default)]
    pub session: Option<String>,
    /// Depuis (secondes Unix).
    #[serde(default)]
    pub since: Option<i64>,
    #[serde(default)]
    pub only_writes: bool,
    /// Lignes renvoyées (défaut 50, max 500).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Origine d'un changeset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChangeOrigin {
    /// Client MCP externe.
    Mcp,
    /// Copilote (dans l'app, ou ses agents CLI).
    Copilot,
}

impl ChangeOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ChangeOrigin::Mcp => "mcp",
            ChangeOrigin::Copilot => "copilot",
        }
    }
    fn parse(s: &str) -> Self {
        if s == "copilot" {
            ChangeOrigin::Copilot
        } else {
            ChangeOrigin::Mcp
        }
    }
}

/// État d'une étape vis-à-vis de l'annulation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    /// En place.
    Active,
    /// Annulée.
    Undone,
    /// Passée lors d'une annulation (non annulable).
    Skipped,
}

impl StepState {
    fn as_str(self) -> &'static str {
        match self {
            StepState::Active => "active",
            StepState::Undone => "undone",
            StepState::Skipped => "skipped",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "undone" => StepState::Undone,
            "skipped" => StepState::Skipped,
            _ => StepState::Active,
        }
    }
}

/// Une modification d'agent, dans son changeset.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ChangeStep {
    /// Numéro d'ordre dans le changeset (1 = la plus ancienne).
    pub seq: i64,
    pub ts: i64,
    pub tool: String,
    pub server_id: Option<i64>,
    /// Cible lisible (chemin, réglage…), vide si l'outil n'en a pas.
    pub target: String,
    pub undoable: bool,
    /// Pourquoi l'étape n'est pas annulable, ou avertissement (annulation partielle).
    pub note: Option<String>,
    /// L'action a échoué en ayant modifié une partie de ses cibles.
    pub failed: bool,
    pub state: StepState,
    pub undone_at: Option<i64>,
}

/// Modifications d'une session d'agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Changeset {
    pub id: String,
    pub origin: ChangeOrigin,
    pub client: Option<String>,
    pub created: i64,
    pub updated: i64,
    pub servers: Vec<i64>,
    /// Octets de copies gardées pour l'annulation.
    pub bytes: i64,
    pub steps: Vec<ChangeStep>,
}

/// Issue de l'annulation d'une étape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RollbackStatus {
    Undone,
    /// Non annulable : laissée en place.
    Skipped,
    /// Cible changée depuis l'action : rien touché (sauf `force`).
    Refused,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RollbackStep {
    pub seq: i64,
    pub tool: String,
    pub target: String,
    pub server_id: Option<i64>,
    pub status: RollbackStatus,
    pub message: String,
}

/// Rapport d'annulation, étape par étape (de la plus récente à la plus ancienne).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RollbackReport {
    pub changeset: String,
    pub steps: Vec<RollbackStep>,
    /// Arrêtée avant la fin (étape refusée ou en échec) : les plus anciennes restent en place.
    pub stopped: bool,
}

// --- Stockage partagé --------------------------------------------------------------------------

#[cfg(not(test))]
fn open_store() -> GuardStore {
    // Même dossier que le reste des données de l'app (voir `Core::new`).
    let dir = crate::util::data_dir().unwrap_or_else(|| std::env::temp_dir().join("minestrator"));
    let _ = std::fs::create_dir_all(&dir);
    GuardStore::open(&dir)
}

#[cfg(test)]
fn open_store() -> GuardStore {
    GuardStore::in_memory()
}

fn store() -> &'static GuardStore {
    static STORE: OnceLock<GuardStore> = OnceLock::new();
    STORE.get_or_init(open_store)
}

// --- Contexte d'un appel -----------------------------------------------------------------------

struct CallCtx {
    session: String,
    client: Option<String>,
    origin: ChangeOrigin,
    /// Étape enregistrée pendant l'appel, reliée à sa ligne du journal.
    step: Mutex<Option<(String, i64)>>,
}

tokio::task_local! {
    /// Appel reçu par le protocole MCP en cours (posé par [`protocol_dispatch`]).
    static CALL: CallCtx;
}

/// Identifiant de ce processus, pour des sessions uniques entre processus MCP.
fn process_tag() -> &'static str {
    static TAG: OnceLock<String> = OnceLock::new();
    TAG.get_or_init(|| format!("p{}-{:x}", std::process::id(), now_secs() & 0xff_ffff))
}

fn clean_id(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(48)
        .collect()
}

/// Session d'un client MCP : par processus, et par connexion quand le transport la donne.
fn mcp_session_key(sid: Option<&str>) -> String {
    match sid.map(clean_id).filter(|s| !s.is_empty()) {
        Some(s) => format!("mcp:{}:{s}", process_tag()),
        None => format!("mcp:{}", process_tag()),
    }
}

/// Changeset du Copilote : un par serveur et par jour (heure locale).
fn copilot_session_key(server: Option<i64>, day: &str) -> String {
    let server = server.map_or_else(|| "?".to_string(), |s| s.to_string());
    format!("copilote:{server}:{day}")
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Serveur du Copilote quand ce processus MCP a été lancé pour l'un de ses agents CLI.
fn copilot_process() -> Option<Option<i64>> {
    std::env::var(crate::config::MCP_COPILOT_SERVER_ENV)
        .ok()
        .map(|v| v.trim().parse::<i64>().ok())
}

/// Session, origine et client d'un appel reçu par le protocole.
fn protocol_identity() -> (String, ChangeOrigin, Option<String>) {
    if let Some(server) = copilot_process() {
        let client = mcp_ctx::client_name().unwrap_or_else(|| "agent CLI du Copilote".into());
        return (copilot_session_key(server, &today()), ChangeOrigin::Copilot, Some(client));
    }
    (mcp_session_key(mcp_ctx::session_id().as_deref()), ChangeOrigin::Mcp, mcp_ctx::client_name())
}

/// Session d'une écriture passée par `mcp::dispatch` : celle de l'appel MCP en cours, sinon le
/// Copilote de l'app (diagnostic autonome, chat, « Appliquer »).
fn write_identity(args: &Value) -> (String, ChangeOrigin, Option<String>) {
    if let Ok(v) = CALL.try_with(|c| (c.session.clone(), c.origin, c.client.clone())) {
        return v;
    }
    if let Some(sid) = mcp_ctx::session_id() {
        return (mcp_session_key(Some(&sid)), ChangeOrigin::Mcp, mcp_ctx::client_name());
    }
    let server = args.get("server_id").and_then(Value::as_i64);
    (copilot_session_key(server, &today()), ChangeOrigin::Copilot, None)
}

/// Session (changeset) de l'appel en cours, s'il y en a un — pour un outil qui enregistre
/// lui-même un serveur créé.
pub(crate) fn current_session() -> Option<String> {
    CALL.try_with(|c| c.session.clone()).ok()
}

// --- Serveurs créés pendant une session (mode constructeur) -----------------------------------

fn created() -> &'static Mutex<HashMap<String, HashSet<i64>>> {
    static CREATED: OnceLock<Mutex<HashMap<String, HashSet<i64>>>> = OnceLock::new();
    CREATED.get_or_init(Default::default)
}

/// Enregistre un serveur créé par la session `session` (clé renvoyée par [`current_session`]) :
/// en mode constructeur, il devient modifiable par cette session. Appelé après un
/// `mybox_create_server` réussi ; à appeler aussi pour toute autre création (`clone_server`).
pub fn record_created_server(session: &str, server_id: i64) {
    created().lock().unwrap().entry(session.to_string()).or_default().insert(server_id);
}

/// [`record_created_server`] pour la session de l'appel en cours (sans effet hors appel MCP).
pub(crate) fn record_created_server_here(server_id: i64) {
    if let Some(session) = current_session() {
        record_created_server(&session, server_id);
    }
}

fn created_in(session: &str) -> HashSet<i64> {
    created().lock().unwrap().get(session).cloned().unwrap_or_default()
}

// --- Mode constructeur -------------------------------------------------------------------------

/// Refus d'une écriture hors de la portée permise (`None` : permise). Ne vérifie pas que l'outil
/// est modifiant (l'appelant le sait). Les sessions du Copilote ne sont pas concernées : elles
/// sont déjà bornées au serveur de la conversation.
async fn scope_violation(core: &Core, cfg: &McpConfig, name: &str, args: &Value) -> Option<String> {
    if copilot_process().is_some() {
        return None;
    }
    let env = |k: &str| std::env::var(k).ok();
    let (scope, allowed) = scope::effective(
        cfg.write_scope,
        &cfg.allowed_servers,
        env(MCP_WRITE_SCOPE_ENV).as_deref(),
        env(MCP_ALLOWED_SERVERS_ENV).as_deref(),
    );
    if scope == WriteScope::All {
        return None;
    }
    let targets = scope::write_targets(name, args);
    let resolved = match &targets {
        Targets::MyBox(id) => core
            .mybox_details(*id)
            .await
            .ok()
            .map(|d| d.servers.iter().map(|s| s.id).collect::<Vec<_>>()),
        // Changeset inconnu : l'outil le dira, il n'y a rien à protéger.
        Targets::Changeset(cs) => store().changeset_servers(cs).ok().map(Option::unwrap_or_default),
        // Base réinitialisée : tous les serveurs de sa MyBox, plus celui dont la config est écrite.
        Targets::Database { server, mybox } => {
            let mybox = match mybox {
                Some(m) => Some(*m),
                None => core.list_servers().await.ok().and_then(|o| {
                    o.servers.iter().find(|s| s.id == *server).map(|s| s.mybox_id).filter(|m| *m > 0)
                }),
            };
            match mybox {
                Some(m) => core.mybox_details(m).await.ok().map(|d| {
                    let mut ids: Vec<i64> = d.servers.iter().map(|s| s.id).collect();
                    ids.push(*server);
                    ids
                }),
                None => None,
            }
        }
        _ => None,
    };
    let made = created_in(&protocol_identity().0);
    builder_refusal(scope, &allowed, &made, &targets, resolved.as_deref())
}

/// Décision une fois tout résolu : en mode constructeur, un serveur est permis s'il a été créé par
/// la session ou s'il est autorisé.
fn builder_refusal(
    scope: WriteScope,
    allowed: &[i64],
    made: &HashSet<i64>,
    targets: &Targets,
    resolved: Option<&[i64]>,
) -> Option<String> {
    if scope == WriteScope::All {
        return None;
    }
    scope::check(targets, resolved, |id| allowed.contains(&id) || made.contains(&id)).err()
}

fn journal_refusal(name: &str, args: &Value, refus: &str) {
    let (session, _, client) = protocol_identity();
    let servers = scope::servers_in(args).unwrap_or_default();
    let summary = summarize_args(name, args, true);
    if let Err(e) = store().insert_action(&NewAction {
        ts: now_secs(),
        session: &session,
        client: client.as_deref(),
        tool: name,
        write: true,
        servers: &servers,
        args: &summary,
        outcome: ActionOutcome::Denied,
        detail: Some(&clip(refus, 300)),
        duration_ms: 0,
        step: None,
    }) {
        tracing::warn!("{e}");
    }
}

/// Garde du protocole, avant tout outil : mode constructeur. `Some(refus)` : l'appel est refusé
/// (et journalisé comme tel).
pub(crate) async fn admit(core: &Core, cfg: &McpConfig, name: &str, args: &Value) -> Option<String> {
    if !crate::mcp::is_write_tool(name) {
        return None;
    }
    let refus = scope_violation(core, cfg, name, args).await?;
    journal_refusal(name, args, &refus);
    Some(refus)
}

/// Même garde pour un outil **modifiant** du MCP officiel relayé par la passerelle
/// (`official_relay::relay_call`).
pub(crate) async fn relay_write_scope(core: &Core, cfg: &McpConfig, name: &str, args: &Value) -> Option<String> {
    let refus = scope_violation(core, cfg, name, args).await?;
    journal_refusal(name, args, &refus);
    Some(refus)
}

// --- Journal du protocole ---------------------------------------------------------------------

/// Texte anonymisé, sur une ligne, tronqué.
fn clip(s: &str, max: usize) -> String {
    let flat = crate::redact::redact(s).replace(['\r', '\n'], " ⏎ ");
    crate::util::truncate_on_boundary(&flat, max, "…")
}

/// Commande console : `redact` ne reconnaît une commande d'authentification qu'avec sa barre
/// (`/login`), or la console de l'API s'en passe (`login motdepasse`) : on l'ajoute le temps du
/// masquage.
fn clip_command(cmd: &str, max: usize) -> String {
    let trimmed = cmd.trim_start();
    if trimmed.starts_with('/') {
        return clip(trimmed, max);
    }
    let masked = clip(&format!("/{trimmed}"), max + 1);
    masked.strip_prefix('/').map(str::to_string).unwrap_or(masked)
}

fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "api_key", "apikey"].iter().any(|h| k.contains(h))
}

fn summarize_value(key: &str, v: &Value, max: usize, depth: usize) -> Value {
    if secret_key(key) && !v.is_null() {
        return json!("[SECRET]");
    }
    match v {
        Value::String(s) => json!(clip(s, max)),
        Value::Array(items) => {
            let mut out: Vec<Value> = items.iter().take(10).map(|x| summarize_value("", x, max.min(80), depth + 1)).collect();
            if items.len() > 10 {
                out.push(json!(format!("… (+{})", items.len() - 10)));
            }
            Value::Array(out)
        }
        Value::Object(m) if depth < 2 => Value::Object(
            m.iter()
                .take(20)
                .map(|(k, x)| (k.clone(), summarize_value(k, x, max.min(60), depth + 1)))
                .collect(),
        ),
        Value::Object(_) => json!(clip(&v.to_string(), 60)),
        other => other.clone(),
    }
}

/// Résumé anonymisé des arguments d'un appel (JSON) : jamais de contenu de fichier ni de secret,
/// valeurs passées par `redact.rs` et tronquées (lectures plus court que écritures).
pub(crate) fn summarize_args(name: &str, args: &Value, write: bool) -> String {
    let Some(map) = args.as_object() else {
        return "{}".into();
    };
    let secret_prop = name == "set_property"
        && args.get("property").and_then(Value::as_str).is_some_and(crate::models::is_secret_property);
    let base = if write { 160 } else { 80 };
    let mut out = serde_json::Map::new();
    for (k, v) in map {
        let s = match k.as_str() {
            // Contenus de fichiers : seulement leur taille.
            "content" => json!(format!("‹{} octets›", v.as_str().map_or(0, str::len))),
            "value" if secret_prop => json!("[SECRET]"),
            "confirm_name" => json!("‹fourni›"),
            "command" | "payload" if v.is_string() => json!(clip_command(v.as_str().unwrap_or_default(), base)),
            "find" | "replace" => summarize_value(k, v, 60, 0),
            // Arguments relayés par `apply_to_servers` : mêmes règles que l'appel direct de l'outil
            // (contenus de fichiers remplacés par leur taille, commandes masquées…).
            "arguments" if name == "apply_to_servers" && v.is_object() => {
                let tool = args.get("tool").and_then(Value::as_str).unwrap_or_default();
                serde_json::from_str(&summarize_args(tool, v, write)).unwrap_or(Value::Null)
            }
            _ => summarize_value(k, v, base, 0),
        };
        out.insert(k.clone(), s);
    }
    let text = Value::Object(out).to_string();
    let cap = if write { 800 } else { 300 };
    if text.len() <= cap {
        text
    } else {
        json!({ "résumé": crate::util::truncate_on_boundary(&text, cap, "…") }).to_string()
    }
}

/// Exécute un appel reçu par le protocole MCP : pose le contexte de la session (pour regrouper
/// ses modifications), appelle `mcp::dispatch`, relève un serveur créé et journalise l'appel.
/// Même signature que `mcp::dispatch` : `mcp::call` l'utilise à sa place.
pub(crate) async fn protocol_dispatch(core: &Core, name: &str, args: Value) -> std::result::Result<String, String> {
    let (session, origin, client) = protocol_identity();
    let write = crate::mcp::is_write_tool(name);
    let mut servers = scope::servers_in(&args).unwrap_or_default();
    let summary = summarize_args(name, &args, write);
    let ctx = CallCtx { session: session.clone(), client: client.clone(), origin, step: Mutex::new(None) };
    let ts = now_secs();
    let started = Instant::now();
    let (out, step) = CALL
        .scope(ctx, async move {
            let out = crate::mcp::dispatch(core, name, args).await;
            let step = CALL.with(|c| c.step.lock().unwrap().take());
            (out, step)
        })
        .await;
    if name == "mybox_create_server" {
        if let Some(id) = out.as_ref().ok().and_then(|t| scope::created_server_id(t)) {
            record_created_server(&session, id);
            servers.push(id);
        }
    }
    let detail = out.as_ref().err().map(|e| clip(e, 300));
    if let Err(e) = store().insert_action(&NewAction {
        ts,
        session: &session,
        client: client.as_deref(),
        tool: name,
        write,
        servers: &servers,
        args: &summary,
        outcome: if out.is_ok() { ActionOutcome::Ok } else { ActionOutcome::Error },
        detail: detail.as_deref(),
        duration_ms: started.elapsed().as_millis() as i64,
        step: step.as_ref().map(|(c, s)| (c.as_str(), *s)),
    }) {
        tracing::warn!("{e}");
    }
    out
}

// --- Capture autour des écritures (mcp::dispatch) ---------------------------------------------

/// Serveur réel d'une étape : SFTP et API du compte actif.
pub(crate) struct CoreRemote<'a> {
    core: &'a Core,
    id: i64,
}

fn es(e: Error) -> String {
    e.to_string()
}

fn tree_meta(e: crate::sftp::TreeEntry) -> Meta {
    use crate::sftp::EntryKind;
    Meta {
        is_dir: e.kind == EntryKind::Dir,
        special: e.kind == EntryKind::Other,
        size: e.size,
        mtime: e.modified,
        path: e.path,
    }
}

impl Remote for CoreRemote<'_> {
    async fn probe(&self, path: &str) -> std::result::Result<Option<Meta>, String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::probe(&conn, path).await })
            .await
            .map(|o| o.map(tree_meta))
            .map_err(es)
    }
    async fn read(&self, path: &str, cap: u64) -> std::result::Result<Vec<u8>, String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::read_bytes(&conn, path, cap).await })
            .await
            .map_err(es)
    }
    async fn write(&self, path: &str, data: &[u8]) -> std::result::Result<(), String> {
        let parent = undo::parent_dir(path);
        let parent = parent.as_str();
        self.core
            .with_sftp(self.id, |conn| async move {
                if parent != "/" {
                    crate::sftp::ensure_dir(&conn, parent).await;
                }
                crate::sftp::write_bytes(&conn, path, data).await
            })
            .await
            .map_err(es)
    }
    async fn remove(&self, path: &str, is_dir: bool) -> std::result::Result<(), String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::remove(&conn, path, is_dir).await })
            .await
            .map_err(es)
    }
    async fn remove_tree(&self, path: &str) -> std::result::Result<(), String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::remove_recursive(&conn, path).await })
            .await
            .map_err(es)
    }
    async fn rename(&self, from: &str, to: &str) -> std::result::Result<(), String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::rename(&conn, from, to).await })
            .await
            .map_err(es)
    }
    async fn mkdir_all(&self, path: &str) -> std::result::Result<(), String> {
        self.core
            .with_sftp(self.id, |conn| async move {
                crate::sftp::ensure_dir(&conn, path).await;
                Ok::<(), Error>(())
            })
            .await
            .map_err(es)?;
        match self.probe(path).await? {
            Some(m) if m.is_dir => Ok(()),
            _ => Err(format!("dossier « {path} » non créé")),
        }
    }
    async fn walk(&self, root: &str, recursive: bool, max: usize) -> std::result::Result<(Vec<Meta>, bool), String> {
        self.core
            .with_sftp(self.id, |conn| async move { crate::sftp::walk_tree(&conn, root, recursive, max).await })
            .await
            .map(|(entries, truncated)| (entries.into_iter().map(tree_meta).collect(), truncated))
            .map_err(es)
    }
    async fn property(&self, key: &str) -> std::result::Result<Option<String>, String> {
        let props = self.core.server_properties(self.id).await.map_err(es)?;
        Ok(props.properties.get(key).cloned())
    }
    async fn set_property(&self, key: &str, value: &str) -> std::result::Result<(), String> {
        self.core.set_server_property(self.id, key, value).await.map_err(es)
    }
    async fn startup(&self) -> std::result::Result<String, String> {
        self.core.get_startup(self.id).await.map(|s| s.command).map_err(es)
    }
    async fn set_startup(&self, command: &str) -> std::result::Result<(), String> {
        self.core.set_startup_params(self.id, command).await.map_err(es)
    }
}

/// Étape en cours de capture, entre [`before_write`] et [`after_write`].
pub(crate) struct PendingStep {
    session: String,
    origin: ChangeOrigin,
    client: Option<String>,
    tool: String,
    server: Option<i64>,
    target: String,
    ts: i64,
    kind: PendingKind,
}

enum PendingKind {
    /// Non annulable par nature (raison) : rien capturé.
    Fixed(&'static str),
    /// Arguments non reconnus par la garde.
    Unreadable,
    Captured(Capture),
}

/// Cible lisible d'une étape non annulable par nature (commande console, action…).
fn fixed_target(name: &str, args: &Value) -> String {
    let text = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default();
    match name {
        "send_command" | "broadcast_command" | "run_command" => clip_command(text("command"), 80),
        "power_action" | "player_action" => text("action").to_string(),
        _ => String::new(),
    }
}

/// Mises à jour de mods : fichiers remplacés d'après la dernière vérification.
async fn resolve_plan(core: &Core, plan: Plan) -> std::result::Result<Plan, String> {
    let Plan::ModUpdates { server, kind, keys } = plan else {
        return Ok(plan);
    };
    let current = core
        .content_updates(server, false)
        .await
        .map_err(|e| format!("mises à jour illisibles : {e}"))?;
    let (files, datapacks) = undo::mod_files(&current, kind, &keys);
    Ok(Plan::ModFiles {
        server,
        files,
        note: datapacks.then(|| "datapacks non couverts par l'annulation".to_string()),
    })
}

/// Sous-étape modifiante d'un outil composite, passée par `mcp::dispatch` pour que la garde des
/// agents la capture (une étape annulable par sous-action). Rend la mention de la garde, s'il y en
/// a une. Future en boîte `dyn … + Send` : casse le cycle de types dispatch → outil → dispatch.
pub(crate) fn guarded_write<'a>(
    core: &'a Core,
    tool: &'a str,
    args: Value,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<Option<String>, String>> + Send + 'a>>
{
    Box::pin(async move {
        let out = crate::mcp::dispatch(core, tool, args).await?;
        Ok(out
            .lines()
            .find(|l| l.starts_with("(Garde des agents"))
            .map(str::to_string))
    })
}

/// Avant un outil modifiant (`mcp::dispatch`) : copie de quoi l'annuler. `None` : pas d'étape
/// (annulation elle-même, outil composite). Ne bloque jamais l'outil.
pub(crate) async fn before_write(core: &Core, name: &str, args: &Value) -> Option<PendingStep> {
    let plan = undo::plan_for(name, args);
    if plan == Plan::Skip {
        return None;
    }
    let (session, origin, client) = write_identity(args);
    let server = plan.server().or_else(|| args.get("server_id").and_then(Value::as_i64));
    let target = match &plan {
        Plan::Fixed(_) => fixed_target(name, args),
        p => p.target(),
    };
    let kind = match plan {
        Plan::Skip => return None,
        Plan::Fixed(reason) => PendingKind::Fixed(reason),
        Plan::Unreadable => PendingKind::Unreadable,
        plan => {
            let id = plan.server().unwrap_or_default();
            let used = store().changeset_bytes(&session).max(0) as u64;
            let mut budget = Budget(undo::STEP_CAP.min(undo::CHANGESET_CAP.saturating_sub(used)));
            let remote = CoreRemote { core, id };
            let refused = |reason: String| Capture { pre: Pre::Nothing, blobs: Vec::new(), refused: Some(reason), note: None };
            let capture = match resolve_plan(core, plan).await {
                Err(reason) => refused(reason),
                Ok(plan) => tokio::time::timeout(CAPTURE_TIMEOUT, undo::capture(&remote, &plan, &mut budget))
                    .await
                    .unwrap_or_else(|_| refused("copie avant l'action trop longue".into())),
            };
            PendingKind::Captured(capture)
        }
    };
    Some(PendingStep { session, origin, client, tool: name.to_string(), server, target, ts: now_secs(), kind })
}

/// Les opérations utilisent-elles des copies ?
fn uses_blobs(ops: &[UndoOp]) -> bool {
    ops.iter().any(|op| match op {
        UndoOp::File { before, .. } => before.is_some(),
        UndoOp::Tree { files, .. } => !files.is_empty(),
        UndoOp::Rename { overwritten, .. } => overwritten.is_some(),
        _ => false,
    })
}

/// Ce qu'une étape enregistre, et s'il faut le signaler dans la sortie de l'outil.
struct StepRecord {
    /// Opérations inverses ; `None` : non annulable.
    undo: Option<Vec<UndoOp>>,
    blobs: Vec<Vec<u8>>,
    note: Option<String>,
    announce: bool,
}

impl StepRecord {
    fn not_undoable(reason: String, announce: bool) -> Self {
        Self { undo: None, blobs: Vec::new(), note: Some(reason), announce }
    }
}

/// Après l'outil modifiant : constat, enregistrement de l'étape, et mention dans la sortie si la
/// modification n'est pas (entièrement) annulable alors qu'elle aurait dû l'être.
pub(crate) async fn after_write(
    core: &Core,
    pending: Option<PendingStep>,
    out: std::result::Result<String, String>,
) -> std::result::Result<String, String> {
    let Some(p) = pending else {
        return out;
    };
    let ok = out.is_ok();
    let StepRecord { undo, blobs, note, announce } = match p.kind {
        // Un échec sans capture n'a rien laissé d'annulable : pas d'étape.
        PendingKind::Fixed(_) | PendingKind::Unreadable if !ok => return out,
        PendingKind::Fixed(reason) => StepRecord::not_undoable(reason.to_string(), false),
        PendingKind::Unreadable => StepRecord::not_undoable("arguments non reconnus par la garde".into(), true),
        PendingKind::Captured(c) => match c.refused {
            Some(_) if !ok => return out,
            Some(reason) => StepRecord::not_undoable(reason, true),
            None => {
                let remote = CoreRemote { core, id: p.server.unwrap_or_default() };
                match tokio::time::timeout(CAPTURE_TIMEOUT, undo::finalize(&remote, &c.pre)).await {
                    // Échec sans effet constaté : pas d'étape.
                    Ok(Ok((ops, _))) if !ok && ops.is_empty() => return out,
                    Ok(Ok((ops, after_note))) => {
                        let note = match (c.note, after_note) {
                            (Some(a), Some(b)) => Some(format!("{a} ; {b}")),
                            (a, b) => a.or(b),
                        };
                        let blobs = if uses_blobs(&ops) { c.blobs } else { Vec::new() };
                        let announce = note.is_some();
                        StepRecord { undo: Some(ops), blobs, note, announce }
                    }
                    Ok(Err(e)) => StepRecord::not_undoable(format!("état après l'action illisible : {e}"), true),
                    Err(_) => StepRecord::not_undoable("constat après l'action trop long".into(), true),
                }
            }
        },
    };
    let saved = store().add_step(&NewStep {
        changeset: &p.session,
        origin: p.origin,
        client: p.client.as_deref(),
        ts: p.ts,
        tool: &p.tool,
        server_id: p.server,
        target: &p.target,
        undo: undo.as_deref(),
        note: note.as_deref(),
        failed: !ok,
        blobs: &blobs,
    });
    let recorded = match saved {
        Ok(seq) => {
            let _ = CALL.try_with(|c| *c.step.lock().unwrap() = Some((p.session.clone(), seq)));
            true
        }
        Err(e) => {
            tracing::warn!("{e}");
            false
        }
    };
    let non_undoable = |m: &str| format!("(Garde des agents : modification NON annulable — {m}.)");
    let message = match (recorded, undo.is_some(), announce, note) {
        (true, true, true, Some(m)) => Some(format!("(Garde des agents : {m}.)")),
        (false, true, _, _) => Some(non_undoable("étape non enregistrée, journal des agents indisponible")),
        (_, false, true, Some(m)) => Some(non_undoable(&m)),
        _ => None,
    };
    match (out, message) {
        (Ok(text), Some(m)) => Ok(format!("{text}\n{m}")),
        (out, _) => out,
    }
}

// --- Annulation --------------------------------------------------------------------------------

/// Annule les `last` dernières étapes actives d'un changeset (toutes par défaut), de la plus
/// récente à la plus ancienne. S'arrête à la première étape refusée (cible changée depuis, sauf
/// `force`) ou en échec : les plus anciennes restent en place.
async fn rollback_with<R: Remote>(
    store: &GuardStore,
    remotes: &HashMap<i64, R>,
    changeset: &str,
    last: Option<usize>,
    force: bool,
    now: i64,
) -> std::result::Result<RollbackReport, String> {
    let mut steps = store.active_steps(changeset)?;
    if let Some(n) = last {
        steps.truncate(n);
    }
    let mut report = RollbackReport { changeset: changeset.to_string(), steps: Vec::new(), stopped: false };
    for st in steps {
        let mut push = |status, message: String| {
            report.steps.push(RollbackStep {
                seq: st.seq,
                tool: st.tool.clone(),
                target: st.target.clone(),
                server_id: st.server_id,
                status,
                message,
            })
        };
        let Some(ops) = &st.undo else {
            store.set_state(changeset, st.seq, StepState::Skipped, now)?;
            push(RollbackStatus::Skipped, format!("non annulable ({})", st.note.as_deref().unwrap_or("aucune copie")));
            continue;
        };
        if ops.is_empty() {
            store.set_state(changeset, st.seq, StepState::Undone, now)?;
            push(RollbackStatus::Undone, "rien à défaire (aucun changement constaté)".into());
            continue;
        }
        let Some(remote) = st.server_id.and_then(|id| remotes.get(&id)) else {
            push(RollbackStatus::Failed, "serveur de l'étape inconnu".into());
            report.stopped = true;
            break;
        };
        let blobs = store.blobs(changeset, st.seq)?;
        match undo::undo_ops(remote, ops, &blobs, force).await {
            Ok(remarks) => {
                store.set_state(changeset, st.seq, StepState::Undone, now)?;
                let msg = if remarks.is_empty() { "annulée".to_string() } else { format!("annulée ; {}", remarks.join(" ; ")) };
                push(RollbackStatus::Undone, msg);
            }
            Err(UndoError::Refused(m)) => {
                push(RollbackStatus::Refused, m);
                report.stopped = true;
                break;
            }
            Err(UndoError::Failed(m)) => {
                push(RollbackStatus::Failed, m);
                report.stopped = true;
                break;
            }
        }
    }
    Ok(report)
}

async fn rollback_core(core: &Core, changeset: &str, last: Option<usize>, force: bool) -> std::result::Result<RollbackReport, String> {
    let servers = store().changeset_servers(changeset)?.unwrap_or_default();
    let remotes: HashMap<i64, CoreRemote> = servers.into_iter().map(|id| (id, CoreRemote { core, id })).collect();
    let report = rollback_with(store(), &remotes, changeset, last, force, now_secs()).await;
    // Fichiers et réglages ont pu changer : les lectures en cache ne valent plus.
    core.cache().clear();
    report
}

fn when(ts: i64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_opt(ts, 0)
        .single()
        .map_or_else(|| ts.to_string(), |d| d.format("%Y-%m-%d %H:%M:%S").to_string())
}

fn step_label(tool: &str, target: &str, server: Option<i64>) -> String {
    let mut s = tool.to_string();
    if !target.is_empty() {
        let _ = write!(s, " {target}");
    }
    if let Some(id) = server {
        let _ = write!(s, ", serveur {id}");
    }
    s
}

fn render_report(r: &RollbackReport) -> String {
    if r.steps.is_empty() {
        return format!("Rien à annuler dans « {} » (changeset inconnu, ou déjà annulé).", r.changeset);
    }
    let mut out = format!("Annulation du changeset « {} » :", r.changeset);
    for s in &r.steps {
        let status = match s.status {
            RollbackStatus::Undone => "annulée",
            RollbackStatus::Skipped => "passée",
            RollbackStatus::Refused => "REFUSÉE",
            RollbackStatus::Failed => "ÉCHEC",
        };
        let _ = write!(out, "\n- étape {} ({}) : {status} — {}", s.seq, step_label(&s.tool, &s.target, s.server_id), s.message);
    }
    if r.stopped {
        out.push_str("\nAnnulation arrêtée : les étapes plus anciennes restent en place.");
    }
    out
}

// --- Outils MCP --------------------------------------------------------------------------------

/// Portée des lectures de la garde pour le Copilote : seulement le serveur de sa conversation
/// (agent CLI : imposé par l'env ; Copilote de l'app : `server_id` exigé, vérifié par sa portée).
fn copilot_read_scope(args: &Value) -> std::result::Result<Option<i64>, String> {
    if let Some(server) = copilot_process() {
        return server.map(Some).ok_or_else(|| "Refusé : portée du Copilote illisible.".into());
    }
    if CALL.try_with(|_| ()).is_ok() || mcp_ctx::session_id().is_some() {
        return Ok(None);
    }
    args.get("server_id")
        .and_then(Value::as_i64)
        .map(Some)
        .ok_or_else(|| "`server_id` requis : le Copilote ne voit que le serveur de la conversation.".into())
}

/// `since` : horodatage Unix, durée relative (`30m`, `12h`, `7d`, `90s`) ou date `AAAA-MM-JJ`
/// (minuit, heure locale).
fn parse_since(v: Option<&Value>, now: i64) -> std::result::Result<Option<i64>, String> {
    let bad = |s: &str| format!("`since` illisible : « {s} » (ex. 30m, 12h, 7d, 2026-11-19 ou horodatage Unix)");
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(n)) => {
            let n = n.as_i64().ok_or_else(|| bad(&n.to_string()))?;
            Ok(Some(if n > 1_000_000_000 { n } else { now - n.max(0) }))
        }
        Some(Value::String(s)) => {
            let t = s.trim();
            if t.is_empty() {
                return Ok(None);
            }
            if let Ok(d) = chrono::NaiveDate::parse_from_str(t, "%Y-%m-%d") {
                use chrono::TimeZone;
                let midnight = d.and_hms_opt(0, 0, 0).ok_or_else(|| bad(t))?;
                return chrono::Local
                    .from_local_datetime(&midnight)
                    .earliest()
                    .map(|d| Some(d.timestamp()))
                    .ok_or_else(|| bad(t));
            }
            let (num, unit) = t.split_at(t.len() - t.chars().last().map_or(0, char::len_utf8));
            let secs = match unit {
                "s" => 1,
                "m" => 60,
                "h" => 3600,
                "d" | "j" => 86_400,
                _ => return t.parse::<i64>().map(|n| Some(if n > 1_000_000_000 { n } else { now - n.max(0) })).map_err(|_| bad(t)),
            };
            let n: i64 = num.trim().parse().map_err(|_| bad(t))?;
            Ok(Some(now - n.max(0) * secs))
        }
        Some(other) => Err(bad(&other.to_string())),
    }
}

/// Outil `list_agent_actions`.
pub(crate) fn actions_tool(core: &Core, args: &Value) -> std::result::Result<String, String> {
    let forced = copilot_read_scope(args)?;
    let query = AgentActionQuery {
        server_id: forced.or_else(|| args.get("server_id").and_then(Value::as_i64)),
        session: args.get("session").and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty()),
        since: parse_since(args.get("since"), now_secs())?,
        only_writes: args.get("only_writes").and_then(Value::as_bool).unwrap_or(false),
        limit: Some(args.get("limit").and_then(Value::as_u64).unwrap_or(30).clamp(1, 200) as u32),
    };
    let list = store().actions(&query)?;
    if list.is_empty() {
        return Ok("Aucune action d'agent journalisée pour ce filtre.".into());
    }
    let mut out = format!("{} action(s), de la plus récente à la plus ancienne :", list.len());
    for a in &list {
        let outcome = match a.outcome {
            ActionOutcome::Ok => "ok",
            ActionOutcome::Error => "erreur",
            ActionOutcome::Denied => "refusé",
        };
        let servers = if a.servers.is_empty() {
            String::new()
        } else {
            format!(" · serveur(s) {}", a.servers.iter().map(i64::to_string).collect::<Vec<_>>().join(", "))
        };
        let _ = write!(
            out,
            "\n- {} · {}{}{servers} · {outcome} · {} ms · session {}{}",
            when(a.ts),
            a.tool,
            if a.write { " ✎" } else { "" },
            a.duration_ms,
            a.session,
            a.client.as_deref().map(|c| format!(" ({c})")).unwrap_or_default(),
        );
        let _ = write!(out, "\n  arguments : {}", a.args);
        if let Some(d) = &a.detail {
            let _ = write!(out, "\n  motif : {d}");
        }
        if let (Some(cs), Some(seq)) = (&a.changeset, a.step) {
            let _ = write!(out, "\n  étape {seq} du changeset {cs}");
        }
    }
    Ok(core.redact_ai(&out))
}

/// Outil `list_changesets`.
pub(crate) fn changesets_tool(core: &Core, args: &Value) -> std::result::Result<String, String> {
    let forced = copilot_read_scope(args)?;
    let server = forced.or_else(|| args.get("server_id").and_then(Value::as_i64));
    let id = args.get("changeset_id").and_then(Value::as_str).filter(|s| !s.is_empty());
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10).clamp(1, 50) as u32;
    let list = store().changesets(limit, server, id)?;
    if list.is_empty() {
        return Ok("Aucune modification d'agent enregistrée pour ce filtre.".into());
    }
    let mut out = String::new();
    for cs in &list {
        let active: Vec<&ChangeStep> = cs.steps.iter().filter(|s| s.state == StepState::Active).collect();
        let undoable = active.iter().filter(|s| s.undoable).count();
        let origin = match cs.origin {
            ChangeOrigin::Mcp => "client MCP",
            ChangeOrigin::Copilot => "Copilote",
        };
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let _ = write!(
            out,
            "Changeset {} — {origin}{} ; {} étape(s) dont {} encore en place ({undoable} annulable(s)) ; serveur(s) {} ; dernière modification {}.",
            cs.id,
            cs.client.as_deref().map(|c| format!(" ({c})")).unwrap_or_default(),
            cs.steps.len(),
            active.len(),
            cs.servers.iter().map(i64::to_string).collect::<Vec<_>>().join(", "),
            when(cs.updated),
        );
        for s in &cs.steps {
            let state = match (s.state, s.undoable) {
                (StepState::Undone, _) => "annulée".to_string(),
                (StepState::Skipped, _) => "passée (non annulable)".to_string(),
                (StepState::Active, true) => "annulable".to_string(),
                (StepState::Active, false) => "NON annulable".to_string(),
            };
            let _ = write!(out, "\n  #{} {} {} — {state}", s.seq, when(s.ts), step_label(&s.tool, &s.target, s.server_id));
            if s.failed {
                out.push_str(" (action en échec, effet partiel)");
            }
            if let Some(n) = &s.note {
                let _ = write!(out, " ; {n}");
            }
        }
    }
    Ok(core.redact_ai(&out))
}

/// Outil `rollback_changeset` (modifiant, jamais donné au Copilote : `INFRA_TOOLS`).
pub(crate) async fn rollback_tool(core: &Core, args: &Value) -> std::result::Result<String, String> {
    let id = args
        .get("changeset_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("paramètre `changeset_id` (texte, voir list_changesets) requis")?
        .trim()
        .to_string();
    let last = args.get("steps").and_then(Value::as_u64).filter(|n| *n > 0).map(|n| n as usize);
    let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
    let active = store().active_steps(&id)?;
    if active.is_empty() {
        return Ok(render_report(&RollbackReport { changeset: id, steps: Vec::new(), stopped: false }));
    }
    let n = last.map_or(active.len(), |l| l.min(active.len()));
    let question = format!(
        "Annuler {n} modification(s) de l'agent (changeset « {id} ») ? Les fichiers et réglages concernés reviennent à leur état précédent{}.",
        if force { ", même s'ils ont changé depuis" } else { "" }
    );
    // Client sans élicitation : l'annulation a lieu (outil modifiant, soumis à `allow_writes`,
    // jamais au Copilote, et refusée sur toute cible changée depuis sauf `force`).
    if mcp_ctx::confirm(&question, None).await == Confirmation::Declined {
        return Err("Annulation refusée par l'utilisateur : rien n'a été modifié.".into());
    }
    let report = rollback_core(core, &id, last, force).await?;
    Ok(core.redact_ai(&render_report(&report)))
}

/// Pour les tests de `mcp.rs` : « défaut » si l'outil n'a pas d'arme dans `undo::plan_for`.
#[cfg(test)]
pub(crate) fn plan_kind(name: &str) -> &'static str {
    match undo::plan_for(name, &json!({})) {
        Plan::Fixed(reason) if reason == undo::NO_CAPTURE => "défaut",
        _ => "prévu",
    }
}

// --- Façade Core (app) -------------------------------------------------------------------------

impl Core {
    /// Journal des actions des agents MCP, du plus récent au plus ancien.
    pub fn agent_actions(&self, query: AgentActionQuery) -> Result<Vec<AgentAction>> {
        store().actions(&query).map_err(Error::Unexpected)
    }

    /// Changesets récents des agents (le plus récemment modifié d'abord), avec leurs étapes.
    pub fn agent_changesets(&self, limit: u32) -> Result<Vec<Changeset>> {
        store().changesets(limit, None, None).map_err(Error::Unexpected)
    }

    /// Annule les `steps` dernières étapes actives d'un changeset (toutes si `None`), après
    /// confirmation de l'utilisateur dans l'app. Voir [`RollbackReport`].
    pub async fn rollback_changeset(&self, changeset_id: &str, steps: Option<u32>, force: bool) -> Result<RollbackReport> {
        rollback_core(self, changeset_id, steps.map(|n| n as usize), force)
            .await
            .map_err(Error::Unexpected)
    }
}

#[cfg(test)]
mod tests {
    use super::undo::fake::FakeServer;
    use super::undo::{capture, finalize};
    use super::*;

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
    }

    /// Enregistre une étape comme le fait `after_write`, à partir d'un faux serveur.
    async fn record(store: &GuardStore, s: &FakeServer, cs: &str, tool: &str, plan: Plan, act: impl FnOnce(&FakeServer)) {
        let target = plan.target();
        let server = plan.server();
        let cap = match plan {
            Plan::Fixed(_) => None,
            plan => Some(capture(s, &plan, &mut Budget(undo::STEP_CAP)).await),
        };
        act(s);
        let (undo, blobs, note) = match cap {
            None => (None, Vec::new(), Some("commande console : effet non réversible".to_string())),
            Some(c) => {
                let (ops, note) = finalize(s, &c.pre).await.unwrap();
                (Some(ops), c.blobs, note)
            }
        };
        store
            .add_step(&NewStep {
                changeset: cs,
                origin: ChangeOrigin::Mcp,
                client: Some("test"),
                ts: now_secs(),
                tool,
                server_id: server.or(Some(1)),
                target: &target,
                undo: undo.as_deref(),
                note: note.as_deref(),
                failed: false,
                blobs: &blobs,
            })
            .unwrap();
    }

    #[test]
    fn rollback_undoes_a_session_in_reverse_order() {
        block(async {
            let st = GuardStore::in_memory();
            let mut remotes = HashMap::new();
            remotes.insert(1, FakeServer::new());
            let s = &remotes[&1];
            s.put("/server.properties", "max-players=20");
            s.put("/plugins/Old.jar", "jar");
            let orig = s.snapshot();
            let one = |p: &str| Plan::Files { server: 1, paths: vec![p.into()] };
            record(&st, s, "cs", "write_file", one("/server.properties"), |s| s.put("/server.properties", "max-players=50")).await;
            record(&st, s, "cs", "send_command", Plan::Fixed("x"), |_| {}).await;
            record(&st, s, "cs", "create_dir", Plan::CreateDir { server: 1, path: "/plugins/New".into() }, |s| s.dir("/plugins/New")).await;
            record(&st, s, "cs", "write_file", one("/plugins/New/config.yml"), |s| s.put("/plugins/New/config.yml", "a: 1")).await;
            record(&st, s, "cs", "delete_path", Plan::Delete { server: 1, path: "/plugins/Old.jar".into() }, |s| s.delete("/plugins/Old.jar")).await;
            // Les deux dernières seulement.
            let r = rollback_with(&st, &remotes, "cs", Some(2), false, now_secs()).await.unwrap();
            assert_eq!(r.steps.iter().map(|s| s.seq).collect::<Vec<_>>(), vec![5, 4]);
            assert!(!r.stopped && s.exists("/plugins/Old.jar") && !s.exists("/plugins/New/config.yml"));
            // Le reste : le dossier créé est vide, il disparaît ; la commande est passée.
            let r = rollback_with(&st, &remotes, "cs", None, false, now_secs()).await.unwrap();
            let statuses: Vec<RollbackStatus> = r.steps.iter().map(|s| s.status).collect();
            assert_eq!(statuses, vec![RollbackStatus::Undone, RollbackStatus::Skipped, RollbackStatus::Undone]);
            assert_eq!(s.snapshot(), orig);
            // Plus rien d'actif.
            let r = rollback_with(&st, &remotes, "cs", None, false, now_secs()).await.unwrap();
            assert!(r.steps.is_empty());
            assert!(render_report(&r).contains("Rien à annuler"));
            let cs = &st.changesets(5, None, None).unwrap()[0];
            assert!(cs.steps.iter().all(|s| s.state != StepState::Active));
            assert_eq!(cs.steps[1].state, StepState::Skipped);
        });
    }

    #[test]
    fn rollback_stops_on_a_changed_target_unless_forced() {
        block(async {
            let st = GuardStore::in_memory();
            let mut remotes = HashMap::new();
            remotes.insert(1, FakeServer::new());
            let s = &remotes[&1];
            s.put("/a.yml", "v1");
            s.put("/b.yml", "b1");
            record(&st, s, "cs", "write_file", Plan::Files { server: 1, paths: vec!["/a.yml".into()] }, |s| s.put("/a.yml", "v2")).await;
            record(&st, s, "cs", "write_file", Plan::Files { server: 1, paths: vec!["/b.yml".into()] }, |s| s.put("/b.yml", "b2")).await;
            s.put("/b.yml", "b3 (main humaine)");
            let r = rollback_with(&st, &remotes, "cs", None, false, now_secs()).await.unwrap();
            assert!(r.stopped);
            assert_eq!(r.steps.len(), 1);
            assert_eq!(r.steps[0].status, RollbackStatus::Refused);
            assert!(render_report(&r).contains("REFUSÉE") && render_report(&r).contains("arrêtée"));
            assert_eq!(s.text("/a.yml").unwrap(), "v2", "l'étape plus ancienne reste en place");
            let r = rollback_with(&st, &remotes, "cs", None, true, now_secs()).await.unwrap();
            assert!(!r.stopped);
            assert_eq!((s.text("/a.yml").unwrap().as_str(), s.text("/b.yml").unwrap().as_str()), ("v1", "b1"));
        });
    }

    #[test]
    fn args_summary_hides_contents_and_secrets() {
        let args = json!({
            "server_id": 4,
            "path": "/plugins/AuthMe/config.yml",
            "content": "password: hunter2\nhost: 10.0.0.12",
            "find": "x".repeat(200),
        });
        let s = summarize_args("write_file", &args, true);
        assert!(!s.contains("hunter2") && !s.contains("10.0.0.12"), "{s}");
        assert!(s.contains("octets") && s.contains("/plugins/AuthMe/config.yml"), "{s}");
        assert!(s.len() < 400, "{s}");
        let s = summarize_args("set_property", &json!({ "server_id": 1, "property": "rcon.password", "value": "hunter2" }), true);
        assert!(!s.contains("hunter2") && s.contains("[SECRET]"), "{s}");
        let s = summarize_args("send_command", &json!({ "server_id": 1, "command": "authme register joueur MotDePasse1" }), true);
        assert!(!s.contains("MotDePasse1"), "{s}");
        let s = summarize_args(
            "mybox_create_server",
            &json!({ "mybox_id": 1, "name": "Test", "required_fields": { "SERVER_PASSWORD": "abc123", "MAP": "x" }, "confirm_name": "Serveur" }),
            true,
        );
        assert!(!s.contains("abc123") && !s.contains("\"Serveur\"") && s.contains("MAP"), "{s}");
        // Lecture : plus court ; énorme argument : résumé tronqué.
        let s = summarize_args("search_logs", &json!({ "server_id": 1, "query": "y".repeat(500) }), false);
        assert!(s.len() <= 310, "{}", s.len());
        assert_eq!(summarize_args("list_servers", &json!(null), false), "{}");
    }

    #[test]
    fn sessions_are_unique_per_process_and_grouped_per_day_for_the_copilot() {
        let a = mcp_session_key(None);
        assert!(a.starts_with("mcp:p") && a == mcp_session_key(Some("")));
        let b = mcp_session_key(Some("sess/1 é"));
        assert!(b.starts_with(&a) && b.ends_with(":sess1"), "{b}");
        assert_eq!(copilot_session_key(Some(12), "2026-11-19"), "copilote:12:2026-11-19");
        assert_eq!(copilot_session_key(None, "2026-11-19"), "copilote:?:2026-11-19");
        // Hors appel MCP : écriture du Copilote de l'app, groupée par serveur et par jour.
        let (session, origin, client) = write_identity(&json!({ "server_id": 7 }));
        assert_eq!(session, copilot_session_key(Some(7), &today()));
        assert_eq!((origin, client), (ChangeOrigin::Copilot, None));
    }

    #[test]
    fn builder_mode_permits_created_and_allowed_servers_only() {
        let made = HashSet::from([40]);
        let allowed = [7];
        let decide = |name: &str, args: Value, resolved: Option<&[i64]>| {
            builder_refusal(WriteScope::Builder, &allowed, &made, &scope::write_targets(name, &args), resolved)
        };
        // Serveur créé par la session, ou autorisé : permis.
        assert_eq!(decide("write_file", json!({ "server_id": 40, "path": "/a" }), None), None);
        assert_eq!(decide("set_property", json!({ "server_id": 7, "property": "pvp" }), None), None);
        // Autre serveur : refus explicite.
        let refus = decide("delete_path", json!({ "server_id": 12, "path": "/world" }), None).unwrap();
        assert!(refus.contains("mode constructeur") && refus.contains("12"), "{refus}");
        // Création permise ; infrastructure sur un serveur non permis refusée.
        assert_eq!(decide("mybox_create_server", json!({ "mybox_id": 3 }), None), None);
        assert!(decide("mybox_delete_server", json!({ "mybox_id": 3, "server_id": 12 }), None).is_some());
        // Copie vers un serveur créé depuis un serveur quelconque : seule la cible compte.
        assert_eq!(decide("copy_between_servers", json!({ "source_server_id": 12, "target_server_id": 40 }), None), None);
        assert!(decide("copy_between_servers", json!({ "source_server_id": 40, "target_server_id": 12 }), None).is_some());
        // MyBox entière : tous ses serveurs doivent être permis.
        assert_eq!(decide("rename_mybox", json!({ "mybox_id": 3 }), Some(&[40, 7])), None);
        assert!(decide("rename_mybox", json!({ "mybox_id": 3 }), Some(&[40, 12])).is_some());
        // Commande groupée : liste permise, « tous les démarrés » jamais.
        assert_eq!(decide("broadcast_command", json!({ "server_ids": [40, 7], "command": "say" }), None), None);
        assert!(decide("broadcast_command", json!({ "all_running": true, "command": "say" }), None).is_some());
        // Portée « tout le compte » : rien n'est refusé.
        let targets = scope::write_targets("delete_path", &json!({ "server_id": 12 }));
        assert_eq!(builder_refusal(WriteScope::All, &[], &HashSet::new(), &targets, None), None);
    }

    #[test]
    fn created_servers_are_tracked_per_session() {
        record_created_server("mcp:test-a", 41);
        record_created_server("mcp:test-a", 42);
        assert_eq!(created_in("mcp:test-a"), HashSet::from([41, 42]));
        assert!(created_in("mcp:test-b").is_empty());
        assert_eq!(current_session(), None, "hors appel MCP");
    }

    #[test]
    fn since_accepts_relative_dates_and_timestamps() {
        let now = 2_000_000_000;
        assert_eq!(parse_since(None, now), Ok(None));
        assert_eq!(parse_since(Some(&json!("30m")), now), Ok(Some(now - 1800)));
        assert_eq!(parse_since(Some(&json!("12h")), now), Ok(Some(now - 43_200)));
        assert_eq!(parse_since(Some(&json!("7d")), now), Ok(Some(now - 7 * 86_400)));
        assert_eq!(parse_since(Some(&json!(1_900_000_000)), now), Ok(Some(1_900_000_000)));
        assert_eq!(parse_since(Some(&json!(60)), now), Ok(Some(now - 60)));
        assert!(parse_since(Some(&json!("2026-11-19")), now).unwrap().is_some());
        assert!(parse_since(Some(&json!("hier")), now).is_err());
        assert!(parse_since(Some(&json!(true)), now).is_err());
    }

    #[test]
    fn copilot_reads_are_bound_to_its_server() {
        // Hors appel MCP (Copilote de l'app) : `server_id` exigé.
        assert!(copilot_read_scope(&json!({})).is_err());
        assert_eq!(copilot_read_scope(&json!({ "server_id": 3 })), Ok(Some(3)));
    }

    #[test]
    fn fixed_steps_are_described_without_secrets() {
        assert_eq!(fixed_target("power_action", &json!({ "action": "restart" })), "restart");
        let t = fixed_target("send_command", &json!({ "command": "login MotDePasse1" }));
        assert!(!t.contains("MotDePasse1"), "{t}");
        assert_eq!(fixed_target("create_snapshot", &json!({})), "");
    }
}
