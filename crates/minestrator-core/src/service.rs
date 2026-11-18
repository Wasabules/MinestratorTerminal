//! Service d'arrière-plan (supervision « machine ») : tout ce que l'app et le service partagent.
//!
//! - **Protocole IPC** app ↔ service : une requête JSON par ligne, une réponse JSON par ligne, sur
//!   un socket Unix (Linux/macOS) ou un canal nommé (Windows). Le serveur vit dans
//!   `minestrator-daemon` ; le client est ici.
//! - **Décision « qui supervise »** ([`decide`]) : l'app met son propre superviseur en veille dès
//!   que le service surveille le même compte, pour éviter double collecte et doubles alertes.
//! - **État du service côté OS** (installé / arrêté / démarré), sans privilège.
//! - **Élévation** des commandes d'administration (`minestrator-daemon enable|uninstall`) :
//!   UAC sous Windows, `pkexec` sous Linux, `osascript` sous macOS.
//! - **Lien côté app** : tâche de fond qui relaie les alertes du service sur le bus d'events du
//!   `Core` (notifications du tray, Copilote) et fusionne son historique de métriques, de
//!   disponibilité et d'automatisations. Quand le service prend la main, l'app lui transmet ses
//!   automatisations et ses destinations d'alertes (avec leurs URL, par IPC uniquement).
//!
//! Conception complète : `docs/SERVICE.md`. Rien ici ne dépend d'une UI.

use crate::automation::{AutomationConfig, AutomationRun};
use crate::error::{Error, Result};
use crate::events::{Alert, CoreEvent};
use crate::persist::PersistedConfig;
use crate::store::{now_secs, MetricSample, StoredAlert};
use crate::supervisor::{SupervisorConfig, SupervisorHeartbeat};
use crate::uptime::{self, UptimeReport, UptimeSpan};
use crate::webhook::WebhookConfig;
use crate::Core;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Notify;

// --- Constantes partagées app ↔ service --------------------------------------------------

/// Version du protocole IPC. Une app et un service de versions différentes ne coopèrent pas :
/// l'app garde alors son propre superviseur.
///
/// 2 : automatisations (`set_automations`, `automation_runs`), webhooks (`set_webhooks`),
/// disponibilité (`uptime`). Un service resté en version 1 ne saurait pas exécuter les
/// automatisations : l'app ne lui délègue donc plus la supervision (et le signale « à mettre à
/// jour ») — elles restent exécutées une fois, par l'app.
pub const PROTOCOL_VERSION: u32 = 2;
/// Nom du binaire du service (livré à côté de l'exécutable de l'app).
pub const DAEMON_BIN: &str = "minestrator-daemon";
/// Nom du service Windows (SCM).
pub const WINDOWS_SERVICE_NAME: &str = "MinestratorDaemon";
/// Unité systemd (Linux).
pub const SYSTEMD_UNIT: &str = "minestrator-daemon.service";
pub const SYSTEMD_UNIT_PATH: &str = "/etc/systemd/system/minestrator-daemon.service";
/// LaunchDaemon (macOS).
pub const LAUNCHD_LABEL: &str = "com.geoffreylecoq.minestratorterminal.daemon";
pub const LAUNCHD_PLIST_PATH: &str =
    "/Library/LaunchDaemons/com.geoffreylecoq.minestratorterminal.daemon.plist";
/// Variable d'environnement qui remplace le point de contact IPC par défaut (dev, tests, daemon
/// lancé hors service).
pub const ENDPOINT_ENV: &str = "MINESTRATOR_DAEMON_SOCKET";
/// Taille maximale d'une requête (le service refuse au-delà). `set_automations` peut porter des
/// dizaines de tâches et leurs commandes.
pub const MAX_REQUEST_BYTES: u64 = 1024 * 1024;
/// Taille maximale d'une réponse (un historique de 14 jours fait quelques Mio).
pub const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
/// Au-delà, un service qui n'a pas réussi de tour de poll est jugé bloqué.
pub const STALE_AFTER_S: i64 = 300;

/// Une alerte relayée par le lien est notifiée si elle a moins de 10 min ; plus ancienne, elle
/// rejoint seulement le centre d'alertes (pas de toast ni de diagnostic sur un incident passé).
const LIVE_ALERT_WINDOW_S: i64 = 600;
const LINK_TICK: Duration = Duration::from_secs(10);
const QUICK_TIMEOUT: Duration = Duration::from_secs(3);
const HISTORY_TIMEOUT: Duration = Duration::from_secs(10);
/// `set_key` valide la clé auprès de l'API : laisser le temps au réseau.
const SET_KEY_TIMEOUT: Duration = Duration::from_secs(45);

/// Point de contact IPC par défaut (ou celui de [`ENDPOINT_ENV`]).
pub fn default_endpoint() -> String {
    if let Ok(v) = std::env::var(ENDPOINT_ENV) {
        if !v.trim().is_empty() {
            return v;
        }
    }
    if cfg!(windows) {
        r"\\.\pipe\minestrator-daemon".to_string()
    } else if cfg!(target_os = "macos") {
        "/var/run/minestrator-daemon.sock".to_string()
    } else {
        // `RuntimeDirectory=minestrator-daemon` de l'unité systemd.
        "/run/minestrator-daemon/daemon.sock".to_string()
    }
}

// --- Protocole ---------------------------------------------------------------------------

/// Requête de l'app au service (une par ligne, JSON, champ `op`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    /// Historique de métriques d'un serveur depuis `since_secs` secondes.
    History { server_id: i64, since_secs: i64 },
    /// Alertes consignées d'identifiant `> after_id` et de date `>= since_ts` (au plus `limit`).
    Alerts {
        after_id: Option<i64>,
        since_ts: Option<i64>,
        limit: Option<u32>,
    },
    GetConfig,
    SetConfig { config: SupervisorConfig },
    /// Transmet la clé API : le service la valide auprès de l'API puis la range dans son
    /// stockage machine. C'est la seule voie par laquelle la clé atteint le service.
    SetKey { key: String },
    /// Oublie la clé : le service cesse de superviser (sans se désinstaller).
    ClearKey,
    /// Redémarre le service (via son gestionnaire : systemd, launchd ou SCM).
    Restart,
    /// Remplace les automatisations (tâches planifiées, redémarrage automatique). Le service
    /// n'exécute que celles du compte dont il détient la clé.
    SetAutomations { config: AutomationConfig },
    /// Remplace les destinations d'alertes et leurs URL (secrets : rangées dans le stockage
    /// machine du service ; celles des destinations retirées y sont effacées).
    SetWebhooks {
        config: WebhookConfig,
        urls: BTreeMap<String, String>,
    },
    /// Périodes surveillées d'un serveur (suivi de disponibilité) entre `from` et `to` (secondes
    /// Unix) : `to` est fixé par l'app, pour que les deux chronologies se raccordent.
    Uptime { server_id: i64, from: i64, to: i64 },
    /// Journal des automatisations d'un compte (d'un serveur si `server_id`), récents d'abord.
    AutomationRuns {
        account_id: i64,
        server_id: Option<i64>,
        limit: Option<u32>,
    },
}

impl Request {
    /// Nom de l'opération (pour les journaux : jamais le contenu).
    pub fn op(&self) -> &'static str {
        match self {
            Request::Ping => "ping",
            Request::Status => "status",
            Request::History { .. } => "history",
            Request::Alerts { .. } => "alerts",
            Request::GetConfig => "get_config",
            Request::SetConfig { .. } => "set_config",
            Request::SetKey { .. } => "set_key",
            Request::ClearKey => "clear_key",
            Request::Restart => "restart",
            Request::SetAutomations { .. } => "set_automations",
            Request::SetWebhooks { .. } => "set_webhooks",
            Request::Uptime { .. } => "uptime",
            Request::AutomationRuns { .. } => "automation_runs",
        }
    }
}

/// `Debug` maison : ni la clé API ni les URL de webhook (des secrets) ne doivent finir dans un
/// journal.
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Request::SetKey { .. } => f.write_str("SetKey { key: \"***\" }"),
            Request::SetWebhooks { config, urls } => f
                .debug_struct("SetWebhooks")
                .field("destinations", &config.destinations.len())
                .field("urls", &format_args!("\"***\" ({})", urls.len()))
                .finish(),
            Request::SetAutomations { config } => f
                .debug_struct("SetAutomations")
                .field("tasks", &config.tasks.len())
                .field("restart_rules", &config.restart_rules.len())
                .finish(),
            Request::Uptime { server_id, from, to } => f
                .debug_struct("Uptime")
                .field("server_id", server_id)
                .field("from", from)
                .field("to", to)
                .finish(),
            Request::AutomationRuns { account_id, server_id, limit } => f
                .debug_struct("AutomationRuns")
                .field("account_id", account_id)
                .field("server_id", server_id)
                .field("limit", limit)
                .finish(),
            Request::History { server_id, since_secs } => f
                .debug_struct("History")
                .field("server_id", server_id)
                .field("since_secs", since_secs)
                .finish(),
            Request::Alerts { after_id, since_ts, limit } => f
                .debug_struct("Alerts")
                .field("after_id", after_id)
                .field("since_ts", since_ts)
                .field("limit", limit)
                .finish(),
            other => f.write_str(other.op()),
        }
    }
}

/// Réponse du service (une par requête, JSON, champs `type` + `data`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Response {
    Pong { protocol: u32, version: String },
    Status(ServiceStatus),
    History(Vec<MetricSample>),
    Alerts { items: Vec<StoredAlert>, last_id: i64 },
    Config(SupervisorConfig),
    Uptime(Vec<UptimeSpan>),
    AutomationRuns(Vec<AutomationRun>),
    Done,
    /// `code` stable (`forbidden`, `bad_request`, `invalid_key`, `unsupported`, `internal`…).
    Error { code: String, message: String },
}

/// État du service, tel qu'il le rapporte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ServiceStatus {
    pub protocol: u32,
    pub version: String,
    pub pid: u32,
    pub started_at: i64,
    /// Lancé par le gestionnaire de services (sinon : `minestrator-daemon run` au premier plan).
    pub managed: bool,
    pub has_key: bool,
    /// Compte MineStrator surveillé (résolu après validation de la clé).
    pub account_id: Option<i64>,
    pub account_name: Option<String>,
    pub supervisor_enabled: bool,
    #[serde(flatten)]
    pub heartbeat: SupervisorHeartbeat,
    pub data_dir: String,
    /// Push FCM configuré.
    pub fcm: bool,
}

impl ServiceStatus {
    /// Le service surveille-t-il effectivement (clé, superviseur actif, poll récent) ?
    pub fn is_supervising(&self, now: i64) -> bool {
        self.has_key
            && self.supervisor_enabled
            && !self.heartbeat.suspended
            && self
                .heartbeat
                .last_poll
                .is_some_and(|t| now - t <= STALE_AFTER_S)
    }
}

// --- Trames ------------------------------------------------------------------------------

/// Lit une trame (une ligne JSON) d'au plus `max` octets. `Ok(None)` en fin de flux.
pub async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max: u64,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = (&mut *reader).take(max).read_until(b'\n', &mut buf).await?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        // Coupée par la limite, ou flux interrompu en pleine trame.
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "trame IPC trop longue ou tronquée",
        ));
    }
    buf.pop();
    Ok(Some(buf))
}

/// Écrit une valeur en trame (JSON + saut de ligne).
pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> std::io::Result<()> {
    let mut bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await
}

// --- Client ------------------------------------------------------------------------------

/// Envoie une requête et renvoie la réponse brute (y compris `Response::Error`).
pub async fn request(endpoint: &str, req: &Request, timeout: Duration) -> Result<Response> {
    match tokio::time::timeout(timeout, request_inner(endpoint, req)).await {
        Ok(r) => r,
        Err(_) => Err(Error::service(
            "timeout",
            format!("le service n'a pas répondu à « {} »", req.op()),
        )),
    }
}

/// Comme [`request`], mais une `Response::Error` devient une `Err`.
pub async fn call(endpoint: &str, req: &Request, timeout: Duration) -> Result<Response> {
    match request(endpoint, req, timeout).await? {
        Response::Error { code, message } => Err(Error::service(&code, message)),
        other => Ok(other),
    }
}

async fn request_inner(endpoint: &str, req: &Request) -> Result<Response> {
    #[cfg(unix)]
    {
        let stream = tokio::net::UnixStream::connect(endpoint)
            .await
            .map_err(connect_error)?;
        exchange(stream, req).await
    }
    #[cfg(windows)]
    {
        let pipe = open_pipe(endpoint).await?;
        // N'importe quel compte peut créer un canal nommé libre : sur le nom par défaut, on
        // vérifie que l'autre bout est bien le processus du service enregistré auprès du SCM
        // (sinon un utilisateur local pourrait se faire passer pour lui et recevoir la clé).
        if std::env::var_os(ENDPOINT_ENV).is_none() {
            use std::os::windows::io::AsRawHandle;
            win::verify_pipe_server(pipe.as_raw_handle())?;
        }
        exchange(pipe, req).await
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (endpoint, req);
        Err(Error::service("unsupported", "plateforme sans service d'arrière-plan"))
    }
}

#[cfg(windows)]
async fn open_pipe(endpoint: &str) -> Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    const ERROR_PIPE_BUSY: i32 = 231;
    // Toutes les instances occupées : on réessaie brièvement (le timeout global borne l'attente).
    loop {
        match ClientOptions::new().open(endpoint) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => return Err(connect_error(e)),
        }
    }
}

fn connect_error(e: std::io::Error) -> Error {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::PermissionDenied => Error::service(
            "forbidden",
            "accès au service refusé (compte non autorisé)",
        ),
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
            Error::service("unreachable", "le service ne tourne pas")
        }
        _ => Error::service("unreachable", format!("connexion au service : {e}")),
    }
}

async fn exchange<S>(mut stream: S, req: &Request) -> Result<Response>
where
    S: tokio::io::AsyncRead + AsyncWrite + Unpin,
{
    let io = |e: std::io::Error| Error::service("protocol", format!("échange IPC : {e}"));
    write_frame(&mut stream, req).await.map_err(io)?;
    let mut reader = BufReader::new(stream);
    let frame = read_frame(&mut reader, MAX_RESPONSE_BYTES)
        .await
        .map_err(io)?
        .ok_or_else(|| Error::service("protocol", "le service a fermé la connexion"))?;
    serde_json::from_slice(&frame)
        .map_err(|e| Error::service("protocol", format!("réponse illisible : {e}")))
}

// --- Décision « qui supervise » -----------------------------------------------------------

/// Qui fait tourner la supervision (connexions monitor, alertes, historique).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supervision {
    /// Le superviseur de l'app (mode utilisateur, comportement historique).
    App,
    /// Le service d'arrière-plan ; le superviseur de l'app est en veille.
    Service,
}

impl Supervision {
    pub fn as_str(self) -> &'static str {
        match self {
            Supervision::App => "app",
            Supervision::Service => "service",
        }
    }
}

/// Décide qui supervise, à partir du compte de l'app (`None` = pas de session) et de l'état du
/// service (`None` = injoignable ou accès refusé).
///
/// Le service ne prend la main que s'il surveille **effectivement** (clé, superviseur actif,
/// poll récent) et **le même compte** que l'app. Dans le doute, l'app garde son superviseur :
/// mieux vaut une alerte en double qu'une panne non signalée.
pub fn decide(app_account: Option<i64>, status: Option<&ServiceStatus>, now: i64) -> Supervision {
    let Some(s) = status else {
        return Supervision::App;
    };
    if s.protocol != PROTOCOL_VERSION || !s.is_supervising(now) {
        return Supervision::App;
    }
    match (app_account, s.account_id) {
        (Some(app), Some(svc)) if app == svc => Supervision::Service,
        // Compte du service pas encore connu : on ne peut pas confirmer.
        (Some(_), _) => Supervision::App,
        // Pas de session dans l'app : son superviseur n'a de toute façon pas de clé.
        (None, _) => Supervision::Service,
    }
}

/// Fusionne l'historique local et celui du service (ordre chronologique, sans doublon de date).
pub fn merge_samples(mut local: Vec<MetricSample>, remote: Vec<MetricSample>) -> Vec<MetricSample> {
    local.extend(remote);
    local.sort_by_key(|s| s.ts);
    local.dedup_by_key(|s| s.ts);
    local
}

// --- Identité du propriétaire --------------------------------------------------------------

/// Identifiant de l'utilisateur courant, autorisé sur le canal IPC lors de l'activation :
/// uid numérique (Unix) ou SID (Windows).
pub fn current_owner_id() -> Result<String> {
    #[cfg(unix)]
    {
        // SAFETY : getuid() n'échoue jamais et n'a aucun effet de bord.
        Ok(unsafe { libc::getuid() }.to_string())
    }
    #[cfg(windows)]
    {
        win::current_user_sid()
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(Error::service("unsupported", "plateforme sans service d'arrière-plan"))
    }
}

/// uid Unix : chiffres uniquement (il finit dans une ligne `ExecStart=`).
pub fn is_valid_uid(s: &str) -> bool {
    !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit())
}

/// SID Windows (`S-1-5-21-…`) : chiffres et tirets uniquement (il finit dans une chaîne SDDL,
/// toute autre forme serait une injection d'ACE).
pub fn is_valid_sid(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("S-1-") else {
        return false;
    };
    s.len() <= 184
        && !rest.is_empty()
        && rest
            .split('-')
            .all(|part| !part.is_empty() && part.len() <= 10 && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Identifiant de propriétaire valide pour la plateforme courante.
pub fn is_valid_owner_id(s: &str) -> bool {
    if cfg!(windows) {
        is_valid_sid(s)
    } else {
        is_valid_uid(s)
    }
}

// --- État côté OS --------------------------------------------------------------------------

/// État du service vu du gestionnaire de services de l'OS (sans privilège).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    NotInstalled,
    Stopped,
    Running,
    Unknown,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallState::NotInstalled => "not_installed",
            InstallState::Stopped => "stopped",
            InstallState::Running => "running",
            InstallState::Unknown => "unknown",
        }
    }
}

/// Interroge le gestionnaire de services (bloquant : appeler via `spawn_blocking`).
pub fn query_install_state() -> InstallState {
    #[cfg(windows)]
    {
        win::query_install_state()
    }
    #[cfg(target_os = "linux")]
    {
        if !Path::new(SYSTEMD_UNIT_PATH).exists() {
            return InstallState::NotInstalled;
        }
        match std::process::Command::new("systemctl")
            .args(["is-active", "--quiet", SYSTEMD_UNIT])
            .status()
        {
            Ok(s) if s.success() => InstallState::Running,
            Ok(_) => InstallState::Stopped,
            Err(_) => InstallState::Unknown,
        }
    }
    #[cfg(target_os = "macos")]
    {
        if !Path::new(LAUNCHD_PLIST_PATH).exists() {
            return InstallState::NotInstalled;
        }
        match std::process::Command::new("launchctl")
            .args(["print", &format!("system/{LAUNCHD_LABEL}")])
            .output()
        {
            Ok(o) if o.status.success() => {
                if String::from_utf8_lossy(&o.stdout).contains("state = running") {
                    InstallState::Running
                } else {
                    InstallState::Stopped
                }
            }
            Ok(_) => InstallState::Stopped,
            Err(_) => InstallState::Unknown,
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        InstallState::Unknown
    }
}

/// Binaire du service livré à côté de l'exécutable de l'app (`externalBin` Tauri), s'il existe.
pub fn locate_daemon_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let name = format!("{DAEMON_BIN}{}", std::env::consts::EXE_SUFFIX);
    let candidate = exe.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

/// Clé et valeur de registre (HKCU) où l'installeur Windows note le choix « supervision en
/// arrière-plan » (`desktop/src-tauri/windows/hooks.nsh`). L'installeur ne demande aucun droit :
/// l'activation élevée a lieu dans l'app, une fois l'utilisateur connecté (la clé API est requise).
pub const PENDING_SETUP_KEY: &str = r"Software\MinestratorTerminal";
pub const PENDING_SETUP_VALUE: &str = "ServiceSetupRequested";

/// Consomme le choix fait à l'installation : `true` une seule fois, puis la valeur est effacée.
/// Toujours `false` hors Windows.
pub fn take_pending_setup() -> bool {
    #[cfg(windows)]
    {
        win::take_pending_setup()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// `path` est-il sous l'une des racines (comparaison insensible à la casse, séparateurs `\`/`/`) ?
pub fn is_under_any(path: &str, roots: &[String]) -> bool {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let p = norm(path);
    roots.iter().map(|r| norm(r)).filter(|r| !r.is_empty()).any(|r| {
        p == r || p.starts_with(&format!("{r}\\"))
    })
}

// --- Élévation ------------------------------------------------------------------------------

/// Moyen d'élévation disponible : `uac`, `pkexec`, `osascript` ou `none`.
pub fn elevation_method() -> &'static str {
    if cfg!(windows) {
        "uac"
    } else if cfg!(target_os = "macos") {
        "osascript"
    } else if cfg!(target_os = "linux") {
        let found = std::env::var_os("PATH").is_some_and(|paths| {
            std::env::split_paths(&paths).any(|d| d.join("pkexec").is_file())
        });
        if found {
            "pkexec"
        } else {
            "none"
        }
    } else {
        "none"
    }
}

/// Citation shell POSIX (`'…'`, apostrophes échappées).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Chaîne AppleScript littérale (`"…"`, `\` et `"` échappés).
pub fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', r"\\").replace('"', "\\\""))
}

/// Script `osascript` qui lance la commande avec les droits administrateur (macOS).
pub fn osascript_script(bin: &Path, args: &[&str]) -> String {
    let mut cmd = sh_quote(&bin.to_string_lossy());
    for a in args {
        cmd.push(' ');
        cmd.push_str(&sh_quote(a));
    }
    format!("do shell script {} with administrator privileges", applescript_string(&cmd))
}

/// Citation d'un argument de ligne de commande Windows (règles de `CommandLineToArgvW`).
pub fn windows_quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

/// Commande à lancer soi-même avec les droits administrateur (repli quand l'élévation
/// graphique est indisponible).
pub fn manual_command(bin: &Path, args: &[&str]) -> String {
    if cfg!(windows) {
        let mut cmd = windows_quote_arg(&bin.to_string_lossy());
        for a in args {
            cmd.push(' ');
            cmd.push_str(&windows_quote_arg(a));
        }
        return cmd;
    }
    let quoted_args: Vec<String> = args.iter().map(|a| sh_quote(a)).collect();
    if std::env::var_os("APPIMAGE").is_some() {
        // root ne peut pas lire le montage FUSE d'une AppImage : copier d'abord le binaire.
        return format!(
            "cp {} /tmp/{DAEMON_BIN} && sudo /tmp/{DAEMON_BIN} {}",
            sh_quote(&bin.to_string_lossy()),
            quoted_args.join(" ")
        );
    }
    format!("sudo {} {}", sh_quote(&bin.to_string_lossy()), quoted_args.join(" "))
}

/// Lance `bin args…` avec les droits administrateur et attend la fin.
pub async fn run_elevated(bin: &Path, args: &[&str]) -> Result<()> {
    #[cfg(windows)]
    {
        let bin = bin.to_path_buf();
        let params: Vec<String> = args.iter().map(|a| windows_quote_arg(a)).collect();
        let code = tokio::task::spawn_blocking(move || win::shell_execute_runas(&bin, &params.join(" ")))
            .await
            .map_err(|e| Error::service("elevation_failed", e.to_string()))??;
        exit_code_result(code, "")
    }
    #[cfg(target_os = "linux")]
    {
        let bin = prepare_for_root(bin)?;
        let out = tokio::process::Command::new("pkexec")
            .arg(&bin)
            .args(args)
            .output()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Error::service("elevation_unavailable", "pkexec introuvable")
                } else {
                    Error::service("elevation_failed", e.to_string())
                }
            })?;
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        match out.status.code() {
            // pkexec : 126 = authentification refusée ou annulée, 127 = non autorisé / erreur.
            Some(126) => Err(Error::service("elevation_cancelled", stderr)),
            Some(127) => Err(Error::service("elevation_failed", stderr)),
            Some(code) => exit_code_result(code as u32, &stderr),
            None => Err(Error::service("elevation_failed", "processus interrompu")),
        }
    }
    #[cfg(target_os = "macos")]
    {
        let out = tokio::process::Command::new("osascript")
            .arg("-e")
            .arg(osascript_script(bin, args))
            .output()
            .await
            .map_err(|e| Error::service("elevation_failed", e.to_string()))?;
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if out.status.success() {
            Ok(())
        } else if stderr.contains("-128") {
            // « User canceled » (-128).
            Err(Error::service("elevation_cancelled", stderr))
        } else {
            Err(Error::service("command_failed", stderr))
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (bin, args);
        Err(Error::service("unsupported", "plateforme sans service d'arrière-plan"))
    }
}

/// Codes de sortie de `minestrator-daemon` : 0 = succès, autre = échec (détail dans stderr).
#[cfg(any(windows, target_os = "linux"))]
fn exit_code_result(code: u32, stderr: &str) -> Result<()> {
    match code {
        0 => Ok(()),
        c => Err(Error::service(
            "command_failed",
            format!("code de sortie {c}. {}", stderr.trim()),
        )),
    }
}

/// AppImage : le binaire vit dans un montage FUSE que root ne peut pas lire. On le copie dans un
/// dossier privé de l'utilisateur ; la commande élevée le recopie ensuite en lieu sûr (root).
#[cfg(target_os = "linux")]
fn prepare_for_root(bin: &Path) -> Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if std::env::var_os("APPIMAGE").is_none() {
        return Ok(bin.to_path_buf());
    }
    let err = |e: std::io::Error| Error::service("elevation_failed", format!("copie du binaire : {e}"));
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("minestrator-daemon-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(err)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).map_err(err)?;
    let dest = dir.join(DAEMON_BIN);
    std::fs::copy(bin, &dest).map_err(err)?;
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755)).map_err(err)?;
    Ok(dest)
}

// --- Lien côté app ------------------------------------------------------------------------

/// Curseur des alertes déjà relayées (persisté : une alerte n'est notifiée qu'une fois).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LinkCursor {
    last_alert_id: Option<i64>,
}

/// État du lien app ↔ service, porté par le [`Core`] de l'app.
pub(crate) struct ServiceLink {
    service_mode: AtomicBool,
    /// Le lien tourne (app) : les automatisations vérifient auprès du service, avant d'agir,
    /// qu'il n'a pas pris la main. Faux dans le service lui-même et dans le MCP.
    started: AtomicBool,
    cursor: PersistedConfig<LinkCursor>,
    wake: Notify,
    /// Sérialise les tours (tâche de fond et commandes) : une alerte n'est relayée qu'une fois.
    tick: tokio::sync::Mutex<()>,
    /// Hors de l'app : dernière décision prise à la demande (voir `Core::probe_service_mode`).
    probed_at: AtomicI64,
}

impl ServiceLink {
    pub(crate) fn new(dir: &Path) -> Self {
        Self {
            probed_at: AtomicI64::new(0),
            service_mode: AtomicBool::new(false),
            started: AtomicBool::new(false),
            cursor: PersistedConfig::load(dir, "service_link.json"),
            wake: Notify::new(),
            tick: tokio::sync::Mutex::new(()),
        }
    }

    fn is_service_mode(&self) -> bool {
        self.service_mode.load(Ordering::SeqCst)
    }
}

/// Vue d'ensemble pour la section Réglages « Service d'arrière-plan ».
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ServiceOverview {
    /// OS pris en charge (Windows, Linux systemd, macOS).
    pub supported: bool,
    /// Binaire du service livré avec l'app.
    pub binary_available: bool,
    /// Le service exécute une autre version que l'app : l'app s'est mise à jour, pas la copie
    /// protégée du binaire que le service exécute (sa mise à jour exige une élévation).
    pub outdated: bool,
    /// `not_installed` | `stopped` | `running` | `unknown`
    #[cfg_attr(feature = "ts", ts(type = "'not_installed' | 'stopped' | 'running' | 'unknown'"))]
    pub install_state: String,
    /// `ok` | `unreachable` | `forbidden` | `timeout` | `protocol`
    pub link: String,
    pub status: Option<ServiceStatus>,
    /// Qui supervise : `app` | `service`
    #[cfg_attr(feature = "ts", ts(type = "'app' | 'service'"))]
    pub supervision: String,
    /// Le service surveille-t-il le compte de l'app ? (`None` si l'un des deux est inconnu)
    pub same_account: Option<bool>,
    /// `uac` | `pkexec` | `osascript` | `none`
    #[cfg_attr(feature = "ts", ts(type = "'uac' | 'pkexec' | 'osascript' | 'none'"))]
    pub elevation: String,
    /// Commande d'activation à lancer soi-même (repli sans élévation graphique).
    pub manual_enable: Option<String>,
}

impl Core {
    // --- Côté service (utilisé par `minestrator-daemon`) ---------------------------------

    /// Consigne une alerte dans le journal local (relu par l'app via IPC).
    pub fn record_alert(&self, alert: &Alert) -> Result<i64> {
        self.store.insert_alert(alert)
    }

    /// Alertes consignées (voir [`crate::store::MetricsStore::alerts`]).
    pub fn stored_alerts(
        &self,
        after_id: Option<i64>,
        since_ts: Option<i64>,
        limit: u32,
    ) -> Result<(Vec<StoredAlert>, i64)> {
        self.store.alerts(after_id, since_ts, limit)
    }

    /// Battement de cœur du superviseur.
    pub fn supervisor_heartbeat(&self) -> SupervisorHeartbeat {
        self.sup_state.heartbeat()
    }

    /// Réveille le superviseur (nouvelle clé, clé oubliée…).
    pub fn supervisor_wake(&self) {
        self.sup_state.wake();
    }

    // --- Côté app -----------------------------------------------------------------------

    /// Démarre le lien avec le service d'arrière-plan (dans un runtime tokio) : toutes les 10 s,
    /// décide qui supervise, met le superviseur local en veille ou l'en sort, et relaie les
    /// nouvelles alertes du service sur le bus d'events.
    ///
    /// Le premier tour est fait **avant** de rendre la main : démarrer le superviseur local
    /// ensuite évite qu'il ouvre ses connexions monitor pour rien quand le service supervise déjà.
    pub async fn start_service_link(self: Arc<Self>) {
        self.service_link.started.store(true, Ordering::SeqCst);
        self.service_tick().await;
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(LINK_TICK) => {}
                    _ = self.service_link.wake.notified() => {}
                }
                self.service_tick().await;
            }
        });
    }

    /// Un tour du lien : qui supervise, relais des alertes. Appelé aussi au changement de compte
    /// (`crate::accounts`) : le service garde son compte, le superviseur local reprend si le
    /// nouveau compte n'est pas le sien.
    pub(crate) async fn service_tick(&self) {
        let _guard = self.service_link.tick.lock().await;
        let endpoint = default_endpoint();
        // Injoignable, accès refusé ou réponse inattendue : comme s'il n'y avait pas de service.
        let status = match request(&endpoint, &Request::Status, QUICK_TIMEOUT).await {
            Ok(Response::Status(s)) => Some(s),
            _ => None,
        };
        let account = match self.has_key() {
            Ok(true) => match self.user_id().await {
                Ok(id) => Some(id),
                // Hors-ligne : on ne tranche pas, l'état précédent est conservé.
                Err(_) => return,
            },
            _ => None,
        };
        let decision = decide(account, status.as_ref(), now_secs());
        let was_service = self
            .service_link
            .service_mode
            .swap(decision == Supervision::Service, Ordering::SeqCst);
        if was_service != (decision == Supervision::Service) {
            tracing::info!(supervision = decision.as_str(), "service d'arrière-plan : bascule de supervision");
        }
        self.sup_state.set_suspended(decision == Supervision::Service);
        if decision == Supervision::Service {
            // Le service prend la main : il reçoit les automatisations et destinations d'alertes
            // de l'app, source de vérité (modifiées pendant qu'il ne supervisait pas, par exemple).
            if !was_service {
                self.push_automations_to(&endpoint).await;
            }
            self.pull_service_alerts(&endpoint).await;
        }
    }

    /// Transmet automatisations et destinations d'alertes (URL comprises) au service.
    async fn push_automations_to(&self, endpoint: &str) {
        let req = Request::SetAutomations { config: self.automation_config() };
        if let Err(e) = call(endpoint, &req, QUICK_TIMEOUT).await {
            tracing::warn!("automatisations non transmises au service : {e}");
        }
        let req = Request::SetWebhooks { config: self.webhook_config(), urls: self.webhook_urls() };
        if let Err(e) = call(endpoint, &req, QUICK_TIMEOUT).await {
            tracing::warn!("destinations d'alertes non transmises au service : {e}");
        }
    }

    /// Le service d'arrière-plan supervise-t-il à la place de l'app (dernière décision) ?
    pub(crate) fn service_mode(&self) -> bool {
        self.service_link.is_service_mode()
    }

    /// Ce processus est-il le superviseur actif, celui qui exécute les automatisations ?
    /// Superviseur en veille : non. Dans l'app, une décision fraîche est prise auprès du service
    /// juste avant d'agir (il a pu prendre la main depuis le dernier tour du lien). Dans le
    /// service lui-même : oui.
    pub(crate) async fn is_active_supervisor(&self) -> bool {
        if self.sup_state.is_suspended() {
            return false;
        }
        if !self.service_link.started.load(Ordering::SeqCst) {
            return true;
        }
        self.service_tick().await;
        !self.service_link.is_service_mode()
    }

    /// Transmet les automatisations au service s'il supervise (après une modification).
    pub(crate) async fn sync_automations(&self) {
        if !self.service_link.is_service_mode() {
            return;
        }
        let req = Request::SetAutomations { config: self.automation_config() };
        if let Err(e) = call(&default_endpoint(), &req, QUICK_TIMEOUT).await {
            tracing::warn!("automatisations non transmises au service : {e}");
        }
    }

    /// Transmet les destinations d'alertes (et leurs URL) au service s'il supervise.
    pub(crate) async fn sync_webhooks(&self) {
        if !self.service_link.is_service_mode() {
            return;
        }
        let req = Request::SetWebhooks { config: self.webhook_config(), urls: self.webhook_urls() };
        if let Err(e) = call(&default_endpoint(), &req, QUICK_TIMEOUT).await {
            tracing::warn!("destinations d'alertes non transmises au service : {e}");
        }
    }

    /// Rapport de disponibilité d'un serveur (24 h / 7 j / 30 j, coupures) : historique local,
    /// complété par celui du service quand c'est lui qui supervise, puis par les minutes mesurées
    /// de l'hébergeur là où rien ne surveillait (règle : [`crate::uptime`]).
    pub async fn uptime_report(&self, server_id: i64) -> Result<UptimeReport> {
        let now = now_secs();
        let from = now - uptime::WINDOWS[2] - uptime::MAX_GAP_S;
        let local = self.local_uptime_spans(server_id, from, now)?;
        let remote = if self.service_link.is_service_mode() {
            let req = Request::Uptime { server_id, from, to: now };
            match call(&default_endpoint(), &req, HISTORY_TIMEOUT).await {
                Ok(Response::Uptime(spans)) => spans,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let merged = uptime::merge_spans(local, remote);
        let filled = self.fill_uptime_with_host(server_id, merged, now - uptime::WINDOWS[2], now).await;
        Ok(uptime::report(server_id, &filled, now))
    }

    /// Journal des automatisations d'un serveur (compte courant) : local, complété par celui du
    /// service quand c'est lui qui supervise. Récents d'abord.
    pub async fn automation_runs(&self, server_id: i64, limit: u32) -> Result<Vec<AutomationRun>> {
        let account = self.user_id().await?;
        let limit = limit.clamp(1, 200);
        let mut runs = self.local_automation_runs(account, Some(server_id), limit)?;
        if self.service_link.is_service_mode() {
            let req = Request::AutomationRuns {
                account_id: account,
                server_id: Some(server_id),
                limit: Some(limit),
            };
            if let Ok(Response::AutomationRuns(remote)) = call(&default_endpoint(), &req, QUICK_TIMEOUT).await {
                runs.extend(remote);
            }
        }
        runs.sort_by_key(|r| std::cmp::Reverse(r.started_ts));
        runs.truncate(limit as usize);
        Ok(runs)
    }

    /// Relaie les alertes du service postérieures au curseur. La première fois, positionne
    /// seulement le curseur (l'arriéré passe par [`Core::service_alerts`], sans notification).
    async fn pull_service_alerts(&self, endpoint: &str) {
        const PAGE: u32 = 200;
        let cursor = self.service_link.cursor.get().last_alert_id;
        let req = Request::Alerts {
            after_id: cursor,
            since_ts: None,
            limit: Some(if cursor.is_some() { PAGE } else { 0 }),
        };
        let Ok(Response::Alerts { items, last_id }) = call(endpoint, &req, QUICK_TIMEOUT).await else {
            return;
        };
        let now = now_secs();
        let next = match cursor {
            // Base du service recréée (purge) : on repart de son dernier identifiant.
            Some(c) if last_id < c => last_id,
            Some(_) if items.len() as u32 == PAGE => items.last().map(|a| a.id).unwrap_or(last_id),
            _ => last_id,
        };
        for item in items {
            if now - item.alert.ts <= LIVE_ALERT_WINDOW_S {
                // Même chemin qu'une alerte locale : toast du tray, centre d'alertes, Copilote.
                self.emit(CoreEvent::Alert(item.alert));
            }
        }
        if cursor != Some(next) {
            self.service_link.cursor.set(LinkCursor { last_alert_id: Some(next) });
        }
    }

    /// Alertes récentes du service (arriéré pour le centre d'alertes, sans notification). Vide
    /// si le service ne supervise pas le compte de l'app.
    pub async fn service_alerts(&self, since_secs: i64) -> Result<Vec<Alert>> {
        if !self.service_link.is_service_mode() {
            self.service_tick().await;
        }
        if !self.service_link.is_service_mode() {
            return Ok(Vec::new());
        }
        let req = Request::Alerts {
            after_id: None,
            since_ts: Some(now_secs() - since_secs.max(0)),
            limit: Some(500),
        };
        match call(&default_endpoint(), &req, QUICK_TIMEOUT).await? {
            Response::Alerts { items, .. } => Ok(items.into_iter().map(|i| i.alert).collect()),
            _ => Err(Error::service("protocol", "réponse inattendue")),
        }
    }

    /// Historique de métriques : local, complété par celui du service quand c'est lui qui
    /// supervise (les deux se suivent dans le temps au moment de la bascule).
    pub async fn metrics_history(&self, server_id: i64, since_secs: i64) -> Result<Vec<MetricSample>> {
        let local = self.metrics(server_id, since_secs)?;
        if !self.service_link.is_service_mode() {
            return Ok(local);
        }
        let req = Request::History { server_id, since_secs };
        match call(&default_endpoint(), &req, HISTORY_TIMEOUT).await {
            Ok(Response::History(remote)) => Ok(merge_samples(local, remote)),
            _ => Ok(local),
        }
    }

    /// Enregistre les réglages du superviseur et, si le service supervise, les lui transmet.
    pub async fn set_supervisor_config_synced(&self, cfg: SupervisorConfig) {
        self.set_supervisor_config(cfg.clone());
        if self.service_link.is_service_mode() {
            let req = Request::SetConfig { config: cfg };
            if let Err(e) = call(&default_endpoint(), &req, QUICK_TIMEOUT).await {
                tracing::warn!("réglages non transmis au service : {e}");
            }
        }
    }

    /// Vue d'ensemble du service pour l'UI (aucune action, aucun privilège).
    pub async fn service_overview(&self) -> ServiceOverview {
        let binary = locate_daemon_binary();
        let install_state = tokio::task::spawn_blocking(query_install_state)
            .await
            .unwrap_or(InstallState::Unknown);
        let (link, status) = match request(&default_endpoint(), &Request::Status, QUICK_TIMEOUT).await {
            Ok(Response::Status(s)) => ("ok".to_string(), Some(s)),
            Ok(Response::Error { code, .. }) => (code, None),
            Ok(_) => ("protocol".to_string(), None),
            Err(Error::Service { code, .. }) => (code, None),
            Err(_) => ("unreachable".to_string(), None),
        };
        let account = *self.user_id_cache.lock().unwrap();
        let same_account = status
            .as_ref()
            .and_then(|s| s.account_id)
            .zip(account)
            .map(|(svc, app)| svc == app);
        let manual_enable = binary.as_deref().and_then(|b| {
            current_owner_id()
                .ok()
                .map(|owner| manual_command(b, &["enable", "--owner", &owner]))
        });
        ServiceOverview {
            supported: cfg!(any(windows, target_os = "linux", target_os = "macos")),
            binary_available: binary.is_some(),
            outdated: status
                .as_ref()
                .is_some_and(|s| s.version != env!("CARGO_PKG_VERSION")),
            install_state: install_state.as_str().to_string(),
            link,
            status,
            supervision: if self.service_link.is_service_mode() { "service" } else { "app" }.to_string(),
            same_account,
            elevation: elevation_method().to_string(),
            manual_enable,
        }
    }

    /// Active le service pour l'utilisateur courant : installation/autorisation élevées si besoin
    /// (`minestrator-daemon enable --owner <id>`), puis transmission de la clé API et des
    /// réglages **par IPC** (la clé ne transite ni par la ligne de commande ni par un fichier).
    pub async fn service_enable(&self) -> Result<ServiceOverview> {
        let token = self.token()?;
        let endpoint = default_endpoint();
        let reachable = matches!(
            request(&endpoint, &Request::Ping, QUICK_TIMEOUT).await,
            Ok(Response::Pong { .. })
        );
        if !reachable {
            let bin = locate_daemon_binary().ok_or_else(|| {
                Error::service("binary_missing", "binaire minestrator-daemon absent de l'installation")
            })?;
            let owner = current_owner_id()?;
            run_elevated(&bin, &["enable", "--owner", &owner]).await?;
            wait_reachable(&endpoint, Duration::from_secs(20)).await?;
        }
        call(&endpoint, &Request::SetKey { key: token }, SET_KEY_TIMEOUT).await?;
        let config = Request::SetConfig { config: self.get_supervisor_config() };
        if let Err(e) = call(&endpoint, &config, QUICK_TIMEOUT).await {
            tracing::warn!("réglages non transmis au service : {e}");
        }
        self.push_automations_to(&endpoint).await;
        self.service_tick().await;
        Ok(self.service_overview().await)
    }

    /// Désactive la supervision par le service (il oublie la clé) ; l'app reprend la main.
    /// Ne demande aucun privilège : le service reste installé.
    pub async fn service_disable(&self) -> Result<ServiceOverview> {
        call(&default_endpoint(), &Request::ClearKey, QUICK_TIMEOUT).await?;
        self.service_tick().await;
        Ok(self.service_overview().await)
    }

    /// Désinstalle le service (élévation) : arrêt, suppression, effacement de la clé machine ;
    /// `purge` efface aussi son historique et ses journaux.
    pub async fn service_uninstall(&self, purge: bool) -> Result<ServiceOverview> {
        let bin = locate_daemon_binary().ok_or_else(|| {
            Error::service("binary_missing", "binaire minestrator-daemon absent de l'installation")
        })?;
        let mut args = vec!["uninstall"];
        if purge {
            args.push("--purge");
        }
        run_elevated(&bin, &args).await?;
        if purge {
            self.service_link.cursor.set(LinkCursor::default());
        }
        self.service_tick().await;
        Ok(self.service_overview().await)
    }

    /// Met à jour le service (élévation) : réinstallation depuis le binaire livré avec l'app — copie
    /// protégée rafraîchie, propriétaires conservés, redémarrage. Nécessaire après une mise à jour
    /// de l'app, qui ne touche pas au binaire que le service exécute.
    pub async fn service_update(&self) -> Result<ServiceOverview> {
        let bin = locate_daemon_binary().ok_or_else(|| {
            Error::service("binary_missing", "binaire minestrator-daemon absent de l'installation")
        })?;
        run_elevated(&bin, &["install"]).await?;
        wait_reachable(&default_endpoint(), Duration::from_secs(20)).await?;
        self.service_tick().await;
        Ok(self.service_overview().await)
    }

    /// Choix « supervision en arrière-plan » fait à l'installation (Windows), consommé une fois.
    pub fn service_take_pending_setup(&self) -> bool {
        take_pending_setup()
    }

    /// Redémarre le service via son gestionnaire (aucun privilège : un propriétaire suffit).
    pub async fn service_restart(&self) -> Result<()> {
        call(&default_endpoint(), &Request::Restart, QUICK_TIMEOUT).await?;
        self.service_link.wake.notify_one();
        Ok(())
    }

    // --- Journaux et historique : lectures hors de l'app ---------------------------------

    /// Le service d'arrière-plan supervise-t-il le compte courant ? Dans l'app : la décision du
    /// lien. Hors de l'app (MCP headless, `--mcp`), le lien ne tourne pas : décision prise à la
    /// demande, au plus une fois par [`PROBE_EVERY_S`], pour que les lectures d'historique
    /// (disponibilité, incidents) incluent ce qu'a vu le service. Aucun autre effet : ni veille
    /// d'un superviseur, ni relais d'alertes, ni curseur, ni envoi de réglages.
    pub(crate) async fn probe_service_mode(&self) -> bool {
        if self.service_link.started.load(Ordering::SeqCst) {
            return self.service_link.is_service_mode();
        }
        let now = now_secs();
        let last = self.service_link.probed_at.load(Ordering::SeqCst);
        if last > 0 && now - last < PROBE_EVERY_S {
            return self.service_link.is_service_mode();
        }
        let status = match request(&default_endpoint(), &Request::Status, QUICK_TIMEOUT).await {
            Ok(Response::Status(s)) => Some(s),
            _ => None,
        };
        let account = match (&status, self.has_key()) {
            (Some(_), Ok(true)) => match self.user_id().await {
                Ok(id) => Some(id),
                // Hors-ligne : on ne tranche pas, la décision précédente est gardée.
                Err(_) => return self.service_link.is_service_mode(),
            },
            _ => None,
        };
        let service = decide(account, status.as_ref(), now) == Supervision::Service;
        self.service_link.service_mode.store(service, Ordering::SeqCst);
        self.service_link.probed_at.store(now, Ordering::SeqCst);
        service
    }

    /// Dernières alertes (au plus [`SERVICE_ALERT_PAGE`]) consignées par le service depuis
    /// `since_ts`, s'il supervise le compte courant ; `None` sinon (absent, injoignable, autre
    /// compte). Lecture seule : le curseur de relais de l'app n'est pas touché.
    pub(crate) async fn service_alert_log(&self, since_ts: i64) -> Option<Vec<Alert>> {
        if !self.probe_service_mode().await {
            return None;
        }
        let endpoint = default_endpoint();
        // Le service rend les alertes par identifiant croissant : on part des plus récentes.
        let head = Request::Alerts { after_id: None, since_ts: Some(since_ts), limit: Some(0) };
        let Ok(Response::Alerts { last_id, .. }) = call(&endpoint, &head, QUICK_TIMEOUT).await else {
            return None;
        };
        let req = Request::Alerts {
            after_id: Some((last_id - i64::from(SERVICE_ALERT_PAGE)).max(0)),
            since_ts: Some(since_ts),
            limit: Some(SERVICE_ALERT_PAGE),
        };
        match call(&endpoint, &req, HISTORY_TIMEOUT).await {
            Ok(Response::Alerts { items, .. }) => Some(items.into_iter().map(|a| a.alert).collect()),
            _ => None,
        }
    }
}

/// Hors de l'app, intervalle minimal entre deux décisions « le service supervise-t-il ? ».
const PROBE_EVERY_S: i64 = 60;
/// Alertes du service relues au plus par `list_incidents` (le service plafonne à 1 000).
const SERVICE_ALERT_PAGE: u32 = 1000;

async fn wait_reachable(endpoint: &str, total: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + total;
    loop {
        match request(endpoint, &Request::Ping, QUICK_TIMEOUT).await {
            Ok(Response::Pong { .. }) => return Ok(()),
            Ok(Response::Error { code, message }) if code == "forbidden" => {
                return Err(Error::service(&code, message));
            }
            _ if tokio::time::Instant::now() >= deadline => {
                return Err(Error::service(
                    "unreachable",
                    "le service ne répond pas après son activation",
                ));
            }
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

// --- Windows ----------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use super::InstallState;
    use crate::error::{Error, Result};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, ERROR_CANCELLED, HANDLE};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE,
    };
    use windows_sys::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SEE_MASK_NO_CONSOLE, SHELLEXECUTEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// Lit puis efface la valeur DWORD `PENDING_SETUP_VALUE` de HKCU (voir `take_pending_setup`).
    pub fn take_pending_setup() -> bool {
        use windows_sys::Win32::System::Registry::{
            RegDeleteKeyValueW, RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD,
        };
        let key = wide(std::ffi::OsStr::new(super::PENDING_SETUP_KEY));
        let value = wide(std::ffi::OsStr::new(super::PENDING_SETUP_VALUE));
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        // SAFETY : chaînes UTF-16 terminées par zéro, tampon de 4 octets pour un REG_DWORD.
        let rc = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut data as *mut u32).cast(),
                &mut size,
            )
        };
        if rc != 0 {
            return false;
        }
        // SAFETY : mêmes chaînes ; échec sans conséquence (le choix serait juste reproposé).
        unsafe {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr());
        }
        data != 0
    }

    /// SID (forme `S-1-5-21-…`) de l'utilisateur du processus courant.
    pub fn current_user_sid() -> Result<String> {
        let fail = |what: &str| {
            Error::service(
                "identity",
                format!("{what} : {}", std::io::Error::last_os_error()),
            )
        };
        // SAFETY : appels Win32 documentés ; chaque handle/mémoire allouée est libéré ci-dessous.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(fail("OpenProcessToken"));
            }
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            // Tampon aligné (u64) : TOKEN_USER contient des pointeurs.
            let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(fail("GetTokenInformation"));
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut wsid: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wsid) == 0 {
                return Err(fail("ConvertSidToStringSidW"));
            }
            let mut n = 0usize;
            while *wsid.add(n) != 0 {
                n += 1;
            }
            let sid = String::from_utf16_lossy(std::slice::from_raw_parts(wsid, n));
            LocalFree(wsid.cast());
            Ok(sid)
        }
    }

    /// Lance `bin params` élevé (UAC, verbe `runas`), fenêtre masquée ; renvoie son code de sortie.
    pub fn shell_execute_runas(bin: &Path, params: &str) -> Result<u32> {
        let verb = wide(std::ffi::OsStr::new("runas"));
        let file = wide(bin.as_os_str());
        let params = wide(std::ffi::OsStr::new(params));
        // SAFETY : la structure est initialisée à zéro puis remplie ; les chaînes vivent jusqu'à
        // la fin de l'appel ; le handle de processus est fermé après l'attente.
        unsafe {
            let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NO_CONSOLE;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = params.as_ptr();
            info.nShow = SW_HIDE;
            if ShellExecuteExW(&mut info) == 0 {
                let e = std::io::Error::last_os_error();
                return Err(if e.raw_os_error() == Some(ERROR_CANCELLED as i32) {
                    Error::service("elevation_cancelled", "élévation refusée")
                } else {
                    Error::service("elevation_failed", e.to_string())
                });
            }
            if info.hProcess.is_null() {
                return Err(Error::service("elevation_failed", "aucun processus lancé"));
            }
            WaitForSingleObject(info.hProcess, INFINITE);
            let mut code = 1u32;
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
            Ok(code)
        }
    }

    /// Vérifie que le serveur du canal nommé est le processus du service (PID rapporté par le SCM).
    pub fn verify_pipe_server(handle: std::os::windows::io::RawHandle) -> Result<()> {
        use windows_service::service::ServiceAccess;
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        let untrusted = |detail: &str| Error::service("untrusted", detail.to_string());
        let mut pid = 0u32;
        // SAFETY : `handle` est le handle valide d'un client de canal nommé ouvert.
        if unsafe { GetNamedPipeServerProcessId(handle as HANDLE, &mut pid) } == 0 {
            return Err(untrusted("PID du serveur IPC introuvable"));
        }
        let expected = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
            .and_then(|m| m.open_service(super::WINDOWS_SERVICE_NAME, ServiceAccess::QUERY_STATUS))
            .and_then(|s| s.query_status())
            .ok()
            .and_then(|s| s.process_id);
        if expected == Some(pid) {
            Ok(())
        } else {
            Err(untrusted("le canal IPC n'est pas tenu par le service installé"))
        }
    }

    pub fn query_install_state() -> InstallState {
        use windows_service::service::{ServiceAccess, ServiceState};
        use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
        const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
        let Ok(manager) = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        else {
            return InstallState::Unknown;
        };
        match manager.open_service(super::WINDOWS_SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
            Ok(service) => match service.query_status() {
                Ok(s) if matches!(s.current_state, ServiceState::Running | ServiceState::StartPending) => {
                    InstallState::Running
                }
                Ok(_) => InstallState::Stopped,
                Err(_) => InstallState::Unknown,
            },
            Err(windows_service::Error::Winapi(e))
                if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
            {
                InstallState::NotInstalled
            }
            Err(_) => InstallState::Unknown,
        }
    }
}

// --- Tests -------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn status(account: Option<i64>, last_poll: Option<i64>) -> ServiceStatus {
        ServiceStatus {
            protocol: PROTOCOL_VERSION,
            version: "0.0.0".into(),
            pid: 1,
            started_at: 0,
            managed: true,
            has_key: true,
            account_id: account,
            account_name: Some("joueur".into()),
            supervisor_enabled: true,
            heartbeat: SupervisorHeartbeat { last_poll, monitored: 2, suspended: false },
            data_dir: "/var/lib/minestrator-daemon".into(),
            fcm: false,
        }
    }

    #[test]
    fn requetes_serialisees_avec_op() {
        let v = serde_json::to_value(Request::SetKey { key: "secret".into() }).unwrap();
        assert_eq!(v, serde_json::json!({ "op": "set_key", "key": "secret" }));
        let v = serde_json::to_value(Request::History { server_id: 3, since_secs: 60 }).unwrap();
        assert_eq!(v, serde_json::json!({ "op": "history", "server_id": 3, "since_secs": 60 }));
        let r: Request = serde_json::from_str(r#"{"op":"ping"}"#).unwrap();
        assert_eq!(r.op(), "ping");
        let r: Request = serde_json::from_str(r#"{"op":"alerts","after_id":4}"#).unwrap();
        assert!(matches!(r, Request::Alerts { after_id: Some(4), since_ts: None, limit: None }));
        assert!(serde_json::from_str::<Request>(r#"{"op":"rm_rf"}"#).is_err());
    }

    #[test]
    fn la_cle_napparait_jamais_dans_debug() {
        let dbg = format!("{:?}", Request::SetKey { key: "ma-cle-api".into() });
        assert!(!dbg.contains("ma-cle-api"));
        assert!(dbg.contains("***"));
    }

    fn webhooks_request() -> Request {
        use crate::webhook::{WebhookDestination, WebhookEvent, WebhookKind};
        Request::SetWebhooks {
            config: WebhookConfig {
                destinations: vec![WebhookDestination {
                    id: "wh-1".into(),
                    name: "Salon staff".into(),
                    kind: WebhookKind::Discord,
                    enabled: true,
                    events: vec![WebhookEvent::Crash],
                }],
            },
            urls: BTreeMap::from([(
                "wh-1".to_string(),
                "https://discord.com/api/webhooks/123/jeton-secret".to_string(),
            )]),
        }
    }

    #[test]
    fn les_url_de_webhook_napparaissent_jamais_dans_debug() {
        let dbg = format!("{:?}", webhooks_request());
        assert!(!dbg.contains("jeton-secret"), "{dbg}");
        assert!(!dbg.contains("discord.com"), "{dbg}");
        assert!(dbg.contains("***") && dbg.contains("destinations: 1"), "{dbg}");
        assert_eq!(webhooks_request().op(), "set_webhooks");
    }

    #[test]
    fn nouvelles_requetes_serialisees() {
        use crate::automation::{RestartRule, Schedule, ScheduledTask, TaskAction};
        let task = ScheduledTask {
            id: "task-1".into(),
            account_id: 7,
            server_id: 3,
            name: "Nuit".into(),
            enabled: true,
            action: TaskAction::Snapshot { name: String::new() },
            schedule: Schedule::Daily { hour: 4, minute: 0 },
            updated_at: 10,
        };
        let req = Request::SetAutomations {
            config: AutomationConfig {
                restart_rules: vec![RestartRule::disabled(7, 3)],
                tasks: vec![task.clone()],
            },
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["op"], "set_automations");
        assert_eq!(v["config"]["tasks"][0]["schedule"]["kind"], "daily");
        let back: Request = serde_json::from_value(v).unwrap();
        assert!(matches!(back, Request::SetAutomations { ref config } if config.tasks == vec![task.clone()]));
        assert_eq!(format!("{req:?}"), "SetAutomations { tasks: 1, restart_rules: 1 }");

        let v = serde_json::to_value(webhooks_request()).unwrap();
        assert_eq!(v["op"], "set_webhooks");
        assert_eq!(v["urls"]["wh-1"], "https://discord.com/api/webhooks/123/jeton-secret");
        assert!(matches!(serde_json::from_value::<Request>(v).unwrap(), Request::SetWebhooks { .. }));

        let v = serde_json::to_value(Request::Uptime { server_id: 3, from: 1, to: 2 }).unwrap();
        assert_eq!(v, serde_json::json!({ "op": "uptime", "server_id": 3, "from": 1, "to": 2 }));
        let r: Request = serde_json::from_str(r#"{"op":"automation_runs","account_id":7}"#).unwrap();
        assert!(matches!(r, Request::AutomationRuns { account_id: 7, server_id: None, limit: None }));

        // Réponses.
        let span = UptimeSpan { start: 1, end: 2, up: true, cause: None, ended: false, host: false };
        let json = serde_json::to_string(&Response::Uptime(vec![span.clone()])).unwrap();
        assert!(json.starts_with(r#"{"type":"uptime","data":["#));
        assert!(matches!(serde_json::from_str::<Response>(&json).unwrap(), Response::Uptime(ref s) if s == &vec![span.clone()]));
        let json = serde_json::to_string(&Response::AutomationRuns(Vec::new())).unwrap();
        assert_eq!(json, r#"{"type":"automation_runs","data":[]}"#);
    }

    #[test]
    fn un_service_en_protocole_1_nest_pas_delegue() {
        // Un service resté en version précédente ne sait pas exécuter les automatisations :
        // l'app garde la supervision (elles s'exécutent alors une fois, chez elle).
        let mut s = status(Some(1), Some(9_970));
        s.protocol = 1;
        assert_eq!(decide(Some(1), Some(&s), 10_000), Supervision::App);
    }

    #[test]
    fn reponses_aller_retour() {
        let s = status(Some(7), Some(100));
        let json = serde_json::to_string(&Response::Status(s.clone())).unwrap();
        assert!(json.contains(r#""type":"status""#));
        // Le battement de cœur est aplati dans l'état.
        assert!(json.contains(r#""last_poll":100"#));
        match serde_json::from_str::<Response>(&json).unwrap() {
            Response::Status(back) => assert_eq!(back, s),
            other => panic!("inattendu : {other:?}"),
        }
        let json = serde_json::to_string(&Response::Done).unwrap();
        assert_eq!(json, r#"{"type":"done"}"#);
        let err: Response =
            serde_json::from_str(r#"{"type":"error","data":{"code":"forbidden","message":"non"}}"#)
                .unwrap();
        assert!(matches!(err, Response::Error { ref code, .. } if code == "forbidden"));
    }

    #[test]
    fn decision_qui_supervise() {
        let now = 10_000;
        let fresh = Some(now - 30);
        // Service injoignable → l'app.
        assert_eq!(decide(Some(1), None, now), Supervision::App);
        // Même compte, poll récent → le service.
        assert_eq!(decide(Some(1), Some(&status(Some(1), fresh)), now), Supervision::Service);
        // Autre compte → l'app garde la main.
        assert_eq!(decide(Some(1), Some(&status(Some(2), fresh)), now), Supervision::App);
        // Compte du service inconnu → l'app (on ne peut pas confirmer).
        assert_eq!(decide(Some(1), Some(&status(None, fresh)), now), Supervision::App);
        // Pas de session dans l'app → le service.
        assert_eq!(decide(None, Some(&status(Some(2), fresh)), now), Supervision::Service);
        // Poll trop ancien (service bloqué) ou jamais fait → l'app.
        let stale = Some(now - STALE_AFTER_S - 1);
        assert_eq!(decide(Some(1), Some(&status(Some(1), stale)), now), Supervision::App);
        assert_eq!(decide(Some(1), Some(&status(Some(1), None)), now), Supervision::App);
        // Sans clé, superviseur désactivé, protocole différent → l'app.
        let mut s = status(Some(1), fresh);
        s.has_key = false;
        assert_eq!(decide(Some(1), Some(&s), now), Supervision::App);
        let mut s = status(Some(1), fresh);
        s.supervisor_enabled = false;
        assert_eq!(decide(Some(1), Some(&s), now), Supervision::App);
        let mut s = status(Some(1), fresh);
        s.protocol = PROTOCOL_VERSION + 1;
        assert_eq!(decide(Some(1), Some(&s), now), Supervision::App);
    }

    #[test]
    fn fusion_historique() {
        let sample = |ts| MetricSample {
            ts,
            cpu: 1.0,
            mem: 1,
            mem_limit: 2,
            disk: 3,
            state: "running".into(),
        };
        let merged = merge_samples(vec![sample(1), sample(5)], vec![sample(3), sample(5), sample(9)]);
        assert_eq!(merged.iter().map(|s| s.ts).collect::<Vec<_>>(), vec![1, 3, 5, 9]);
    }

    #[test]
    fn identifiants_de_proprietaire() {
        assert!(is_valid_uid("1000"));
        assert!(is_valid_uid("0"));
        assert!(!is_valid_uid(""));
        assert!(!is_valid_uid("1000 --evil"));
        assert!(!is_valid_uid("-1"));
        assert!(is_valid_sid("S-1-5-21-3623811015-3361044348-30300820-1013"));
        assert!(is_valid_sid("S-1-5-18"));
        assert!(!is_valid_sid("S-1-5-21-1)(A;;GA;;;WD"));
        assert!(!is_valid_sid("S-1-"));
        assert!(!is_valid_sid("S-1-5--1"));
        assert!(!is_valid_sid("WD"));
    }

    #[test]
    fn citations_pour_elevation() {
        assert_eq!(sh_quote("a b"), "'a b'");
        assert_eq!(sh_quote("l'app"), r"'l'\''app'");
        let script = osascript_script(
            Path::new("/Applications/Minestrator Terminal.app/Contents/MacOS/minestrator-daemon"),
            &["enable", "--owner", "501"],
        );
        assert_eq!(
            script,
            "do shell script \"'/Applications/Minestrator Terminal.app/Contents/MacOS/minestrator-daemon' \
             'enable' '--owner' '501'\" with administrator privileges"
        );
        assert_eq!(applescript_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(windows_quote_arg("enable"), "enable");
        assert_eq!(windows_quote_arg(r"C:\Program Files\x.exe"), r#""C:\Program Files\x.exe""#);
        assert_eq!(windows_quote_arg(r#"a"b"#), r#""a\"b""#);
        assert_eq!(windows_quote_arg(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(windows_quote_arg(""), r#""""#);
    }

    #[test]
    fn chemin_sous_une_racine() {
        let roots = vec![
            r"C:\Users\Alice\AppData\Local".to_string(),
            r"C:\Users\Alice".to_string(),
        ];
        assert!(is_under_any(
            r"C:\Users\Alice\AppData\Local\Minestrator Terminal\minestrator-daemon.exe",
            &roots
        ));
        assert!(is_under_any(r"c:/users/alice/bin/x.exe", &roots));
        assert!(!is_under_any(
            r"C:\Program Files\Minestrator Terminal\minestrator-daemon.exe",
            &roots
        ));
        // Préfixe de nom ≠ sous-dossier.
        assert!(!is_under_any(r"C:\Users\Alice2\x.exe", &roots));
        assert!(!is_under_any(r"C:\x.exe", &[String::new()]));
    }

    #[tokio::test]
    async fn trames_bornees() {
        let mut ok = BufReader::new(&b"{\"op\":\"ping\"}\n{\"op\":\"status\"}\n"[..]);
        let a = read_frame(&mut ok, 1024).await.unwrap().unwrap();
        assert_eq!(a, br#"{"op":"ping"}"#);
        let b = read_frame(&mut ok, 1024).await.unwrap().unwrap();
        assert_eq!(b, br#"{"op":"status"}"#);
        assert!(read_frame(&mut ok, 1024).await.unwrap().is_none());

        let mut long = BufReader::new(&b"0123456789\n"[..]);
        assert!(read_frame(&mut long, 4).await.is_err());
        let mut cut = BufReader::new(&b"{\"op\""[..]);
        assert!(read_frame(&mut cut, 1024).await.is_err());

        let mut out = Vec::new();
        write_frame(&mut out, &Request::Ping).await.unwrap();
        assert_eq!(out, b"{\"op\":\"ping\"}\n");
    }

    #[tokio::test]
    async fn service_injoignable() {
        let endpoint = if cfg!(windows) {
            format!(r"\\.\pipe\minestrator-test-absent-{}", std::process::id())
        } else {
            std::env::temp_dir()
                .join(format!("minestrator-test-absent-{}.sock", std::process::id()))
                .to_string_lossy()
                .into_owned()
        };
        let err = request(&endpoint, &Request::Ping, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Service { ref code, .. } if code == "unreachable"), "{err:?}");
    }
}
