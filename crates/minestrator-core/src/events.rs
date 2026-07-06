//! Events métier diffusés par le cœur (console temps réel, et à terme superviseur…).
//!
//! Le cœur publie des `CoreEvent` sur un canal `broadcast`. Chaque frontend s'abonne :
//! - Tauri les relaie au webview ;
//! - un daemon les enverrait sur un socket / les journaliserait ;
//! - une CLI les afficherait.
//!
//! Les payloads sont `Serialize` pour être transmis tels quels par n'importe quel frontend.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleOutput {
    pub conn_id: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleStatus {
    pub conn_id: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleConnection {
    pub conn_id: String,
    /// connecting | open | reconnecting | closed | hibernated
    pub phase: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsoleStats {
    pub conn_id: String,
    pub cpu_absolute: f64,
    pub memory_bytes: u64,
    pub memory_limit_bytes: u64,
    pub disk_bytes: u64,
    pub uptime: u64,
    pub state: String,
}

/// Ligne de console classée WARN/ERROR, repérée par une connexion **monitor** du
/// superviseur. Interne : sert de déclencheur au Copilote (jamais relayée au webview).
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleLog {
    pub server_id: i64,
    /// `error` | `warn`
    pub level: String,
    pub line: String,
}

/// Alerte émise par le superviseur (crash, seuils, expiration…).
#[derive(Debug, Clone, Serialize)]
pub struct Alert {
    /// ID serveur, ou ID MyBox pour une alerte d'expiration.
    pub server_id: i64,
    pub server_name: String,
    /// `crash` | `cpu` | `ram` | `disk` | `expiry`
    pub kind: String,
    /// `warning` | `critical`
    pub severity: String,
    pub message: String,
    pub ts: i64,
}

/// Une action concrète proposée par le Copilote, applicable via la couche d'outils.
/// L'exécution dépend du niveau d'autonomie choisi (jamais exécutée en mode « suggérer »).
#[derive(Debug, Clone, Serialize)]
pub struct ProposedAction {
    /// Nom d'outil (`power_action`, `send_command`, `write_file`, …).
    pub tool: String,
    /// Arguments de l'outil (inclut `server_id`).
    pub args: Value,
    /// Description lisible de l'action.
    pub label: String,
    /// `safe` (réversible/sans perte) | `caution` (modifie une config) | `danger` (destructif).
    pub risk: String,
}

/// Émis au **lancement** d'un diagnostic (avant l'appel LLM) : permet à l'UI d'afficher un
/// indicateur « analyse en cours ». L'`id` correspond à celui du [`Diagnosis`] final.
#[derive(Debug, Clone, Serialize)]
pub struct CopilotStarted {
    pub id: String,
    pub server_id: i64,
    pub server_name: String,
    /// `crash` | `cpu` | `ram` | `disk` | `error` | `warn` | `manual` | `selection` | `performance`
    pub trigger: String,
}

/// Étape d'avancement d'une analyse en cours (pour le log/détail dans l'UI). `id` = celui du
/// [`CopilotStarted`]/[`Diagnosis`] correspondant.
#[derive(Debug, Clone, Serialize)]
pub struct CopilotProgress {
    pub id: String,
    pub phase: String,
}

/// Fragment de texte de réponse de l'assistant, émis en direct (streaming). `id` = session_id
/// (id d'onglet). Le front accumule ces fragments dans la bulle en cours ; la réponse finale
/// (renvoyée par `chat_send`) reste la source d'autorité.
#[derive(Debug, Clone, Serialize)]
pub struct ChatDelta {
    pub id: String,
    pub text: String,
}

/// Rapport de diagnostic produit par le Copilote (agent Claude) suite à un incident.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnosis {
    /// Identifiant unique (`{server_id}-{ts}`).
    pub id: String,
    pub server_id: i64,
    pub server_name: String,
    /// `crash` | `cpu` | `ram` | `disk` | `manual`
    pub trigger: String,
    /// `warning` | `critical`
    pub severity: String,
    /// Résumé en une phrase.
    pub summary: String,
    /// Cause probable (markdown).
    pub cause: String,
    /// Correctif détaillé (markdown).
    pub suggested_fix: String,
    /// Actions concrètes proposées (0 à quelques-unes).
    pub actions: Vec<ProposedAction>,
    pub ts: i64,
}

/// Progression d'un transfert SFTP (upload / download / zip), pour le gestionnaire de transferts.
/// `id` = identifiant du transfert (généré côté front pour corréler). `done`/`total` en octets.
#[derive(Debug, Clone, Serialize)]
pub struct SftpProgress {
    pub id: String,
    /// Nom lisible (fichier, ou nom du .zip pour un téléchargement groupé).
    pub name: String,
    /// `up` (téléversement) | `down` (téléchargement).
    pub direction: String,
    pub done: u64,
    pub total: u64,
    /// `active` | `done` | `error`.
    pub status: String,
    /// Message si `status = error`.
    pub error: Option<String>,
}

/// Événement métier. Les frontends font le mapping vers leur canal de sortie.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    ConsoleOutput(ConsoleOutput),
    ConsoleStatus(ConsoleStatus),
    ConsoleConnection(ConsoleConnection),
    ConsoleStats(ConsoleStats),
    ConsoleLog(ConsoleLog),
    Alert(Alert),
    CopilotStarted(CopilotStarted),
    CopilotProgress(CopilotProgress),
    ChatDelta(ChatDelta),
    Diagnosis(Diagnosis),
    SftpProgress(SftpProgress),
}
