//! Transport **concurrent** du serveur MCP local : messages JSON-RPC délimités par des sauts de
//! ligne, **une tâche par requête** (un outil long ne bloque plus les autres), **un seul
//! rédacteur** pour la sortie (canal), requêtes serveur → client (élicitation) avec table des
//! requêtes en attente, annulation (`notifications/cancelled`). Générique sur `AsyncRead` /
//! `AsyncWrite` : stdio en production, `tokio::io::duplex` dans les tests. **stdout = protocole** ;
//! les journaux vont sur stderr (`tracing`).
//!
//! Deux ères du protocole sont servies par le même processus (serveur « dual-era » de la
//! spécification `2026-07-28`) :
//! - **avec poignée de main** (`2024-11-05` à `2025-11-25`) : `initialize` négocie la version et
//!   déclare les capacités et l'identité du client pour toute la connexion ; une confirmation est
//!   une requête `elicitation/create` envoyée au client ;
//! - **sans état** (`2026-07-28`) : chaque requête porte sa version, ses capacités et l'identité du
//!   client dans `_meta` ; `server/discover` décrit le serveur ; chaque résultat porte `resultType`
//!   (et `ttlMs` / `cacheScope` pour les listes) ; une confirmation interrompt l'appel par un
//!   résultat `input_required` et le client rejoue la requête avec `inputResponses`.
//!
//! La logique MCP elle-même (outils, prompts, ressources) est derrière [`Handler`] :
//! `mcp::handle_message` en production, un faux gestionnaire dans les tests.

use crate::mcp_ctx::{CallCtx, CallInit};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

/// Versions sans état (`_meta` par requête), de la plus récente à la plus ancienne.
pub(crate) const STATELESS_VERSIONS: &[&str] = &["2026-07-28"];
/// Versions à poignée de main (`initialize`), de la plus récente à la plus ancienne.
/// `2024-11-05` reste gérée : c'est celle des agents CLI du Copilote les plus anciens.
pub(crate) const HANDSHAKE_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Clés `_meta` des requêtes et résultats sans état.
pub(crate) const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
pub(crate) const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
pub(crate) const META_CLIENT_CAPS: &str = "io.modelcontextprotocol/clientCapabilities";
pub(crate) const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// Variable d'environnement : `0` / `false` / `off` coupe l'ère sans état (`server/discover`
/// répond « méthode inconnue », les clients « dual-era » se rabattent sur `initialize`). Par
/// défaut active, sauf dans un serveur lancé par le Copilote pour un agent CLI (liste blanche
/// d'env posée) : ce chemin garde la poignée de main, comme avant.
pub(crate) const MCP_STATELESS_ENV: &str = "MINESTRATOR_MCP_STATELESS";

/// Délai maximal d'attente d'une réponse de l'humain (élicitation) : au-delà, refus.
const ELICITATION_TIMEOUT: Duration = Duration::from_secs(300);
/// Signe de vie d'un appel long sans progression (requête avec `progressToken`).
const HEARTBEAT: Duration = Duration::from_secs(15);

/// Toutes les versions gérées, sans état d'abord.
pub(crate) fn supported_versions() -> Vec<&'static str> {
    STATELESS_VERSIONS
        .iter()
        .chain(HANDSHAKE_VERSIONS)
        .copied()
        .collect()
}

/// Version renvoyée à `initialize` : celle demandée si nous la gérons, sinon notre plus récente
/// à poignée de main (une version sans état ne se négocie pas par `initialize`).
pub(crate) fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| HANDSHAKE_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(HANDSHAKE_VERSIONS[0])
}

/// Le client déclare-t-il l'élicitation en **formulaire** ? (`{}` vaut `{ "form": {} }`.)
pub(crate) fn form_elicitation(capabilities: Option<&Value>) -> bool {
    match capabilities.and_then(|c| c.get("elicitation")) {
        Some(Value::Object(modes)) => modes.is_empty() || modes.contains_key("form"),
        _ => false,
    }
}

/// Nom affichable d'un client (`clientInfo.name`), borné et sans caractères de contrôle.
fn client_label(info: Option<&Value>) -> Option<String> {
    let name = info?.get("name")?.as_str()?;
    let clean: String = name.chars().filter(|c| !c.is_control()).take(100).collect();
    (!clean.trim().is_empty()).then(|| clean.trim().to_string())
}

/// Logique MCP servie par le transport.
pub(crate) trait Handler: Send + Sync + 'static {
    /// Traite une requête (contexte d'appel posé par le transport). `None` : pas de réponse.
    fn handle(&self, msg: Value) -> BoxFuture<'static, Option<Value>>;
    /// Sur annulation, la requête peut-elle être abandonnée net ? Sinon (action modifiante),
    /// `mcp_ctx::cancelled()` passe à vrai, l'outil termine ce qu'il peut, et aucune réponse
    /// n'est envoyée.
    fn abortable(&self, msg: &Value) -> bool;
    /// `{ name, version, … }` du serveur (résultats sans état).
    fn server_info(&self) -> Value;
}

/// Réglages du transport (délais raccourcis dans les tests).
#[derive(Debug, Clone)]
pub(crate) struct ServeOptions {
    pub elicitation_timeout: Duration,
    pub heartbeat: Duration,
    /// Ère sans état servie (`server/discover`, `_meta` par requête).
    pub stateless: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            elicitation_timeout: ELICITATION_TIMEOUT,
            heartbeat: HEARTBEAT,
            stateless: true,
        }
    }
}

impl ServeOptions {
    /// Réglages du processus : variable [`MCP_STATELESS_ENV`], et poignée de main seule pour un
    /// serveur lancé par le Copilote (sauf si la variable l'active explicitement).
    pub(crate) fn from_env() -> Self {
        let forced = std::env::var(MCP_STATELESS_ENV).ok().and_then(|v| {
            match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" | "oui" => Some(true),
                "0" | "false" | "off" | "no" | "non" => Some(false),
                _ => None,
            }
        });
        let copilot = std::env::var(crate::config::MCP_ALLOWED_TOOLS_ENV).is_ok()
            || std::env::var(crate::config::MCP_COPILOT_SERVER_ENV).is_ok();
        Self {
            stateless: forced.unwrap_or(!copilot),
            ..Self::default()
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Clé canonique d'un identifiant JSON-RPC (nombre ou texte).
fn id_key(id: &Value) -> String {
    id.to_string()
}

// --- Pair : sortie unique et requêtes serveur → client ------------------------------------------

/// Échec d'une requête envoyée au client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PeerError {
    /// Connexion fermée (entrée terminée) : le client ne peut plus répondre.
    Closed,
    Timeout,
    /// L'appel qui attendait la réponse a été annulé.
    Cancelled,
    /// Le client a répondu par une erreur JSON-RPC.
    Rejected(String),
}

impl std::fmt::Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerError::Closed => write!(f, "connexion fermée"),
            PeerError::Timeout => write!(f, "pas de réponse dans le délai"),
            PeerError::Cancelled => write!(f, "appel annulé"),
            PeerError::Rejected(m) => write!(f, "refusée par le client : {m}"),
        }
    }
}

/// Le client vu du serveur : tout ce qui part vers lui passe par le canal du rédacteur unique.
pub(crate) struct Peer {
    out: Mutex<Option<mpsc::UnboundedSender<Value>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Value>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    timeout: Duration,
}

impl Peer {
    pub(crate) fn new(out: mpsc::UnboundedSender<Value>, timeout: Duration) -> Self {
        Self {
            out: Mutex::new(Some(out)),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            timeout,
        }
    }

    pub(crate) fn send(&self, msg: Value) {
        if let Some(tx) = lock(&self.out).as_ref() {
            let _ = tx.send(msg);
        }
    }

    pub(crate) fn notify(&self, method: &str, params: Value) {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Requête au client ; attend sa réponse, au plus le délai de la session. Une attente
    /// abandonnée (délai, annulation, appel interrompu) est signalée au client par
    /// `notifications/cancelled` : il retire la question affichée.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        cancel: &mut watch::Receiver<bool>,
    ) -> Result<Value, PeerError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(PeerError::Closed);
        }
        let id = format!(
            "minestrator-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        );
        let (tx, rx) = oneshot::channel();
        let mut waiting = Waiting {
            peer: self,
            key: id_key(&json!(id)),
            id: id.clone(),
            reason: "appel annulé",
            armed: true,
        };
        lock(&self.pending).insert(waiting.key.clone(), tx);
        if self.closed.load(Ordering::SeqCst) {
            waiting.armed = false;
            lock(&self.pending).remove(&waiting.key);
            return Err(PeerError::Closed);
        }
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        tokio::select! {
            answer = rx => {
                waiting.armed = false;
                match answer {
                    Ok(msg) => match msg.get("error") {
                        Some(e) => Err(PeerError::Rejected(
                            e.get("message").and_then(Value::as_str).unwrap_or("erreur").to_string(),
                        )),
                        None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                    },
                    Err(_) => Err(PeerError::Closed),
                }
            },
            _ = tokio::time::sleep(self.timeout) => {
                waiting.reason = "délai dépassé";
                Err(PeerError::Timeout)
            },
            _ = wait_cancelled(cancel) => Err(PeerError::Cancelled),
        }
    }

    /// Réponse du client à une de nos requêtes. Faux si personne ne l'attendait.
    fn resolve(&self, msg: Value) -> bool {
        let Some(id) = msg.get("id") else {
            return false;
        };
        match lock(&self.pending).remove(&id_key(id)) {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }

    /// Entrée terminée : plus aucune réponse ne viendra (les attentes échouent : refus).
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.pending).clear();
    }

    /// Plus rien à écrire : le rédacteur se termine une fois le canal vidé.
    fn shutdown(&self) {
        lock(&self.out).take();
    }
}

/// Question posée au client et pas encore répondue. Si l'attente s'arrête sans réponse — délai,
/// annulation, ou appel abandonné (future détruite) —, la question est retirée de la table et
/// annulée côté client.
struct Waiting<'a> {
    peer: &'a Peer,
    key: String,
    id: String,
    reason: &'static str,
    armed: bool,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        lock(&self.peer.pending).remove(&self.key);
        self.peer.notify(
            "notifications/cancelled",
            json!({ "requestId": self.id, "reason": self.reason }),
        );
    }
}

/// Attend l'annulation ; ne se termine jamais si l'émetteur a disparu.
async fn wait_cancelled(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|c| *c).await.is_err() {
        std::future::pending::<()>().await;
    }
}

// --- Session (une connexion) --------------------------------------------------------------------

/// Ce que l'initialisation a établi pour la connexion (ère à poignée de main).
#[derive(Debug, Clone)]
struct Handshake {
    version: String,
    client: Option<String>,
    elicitation: bool,
}

struct Session {
    id: String,
    peer: Arc<Peer>,
    opts: ServeOptions,
    handshake: Mutex<Handshake>,
    /// Requêtes en cours : identifiant → signal d'annulation.
    inflight: Mutex<HashMap<String, watch::Sender<bool>>>,
}

fn new_session_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        "mcp-{}-{nanos:x}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

/// Sert le protocole MCP sur `reader` / `writer` jusqu'à la fin de l'entrée ; les requêtes en
/// cours sont alors menées à leur terme (leurs réponses partent) avant de rendre la main.
pub(crate) async fn serve<H, R, W>(handler: Arc<H>, reader: R, writer: W, opts: ServeOptions)
where
    H: Handler,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel();
    let writer_task = tokio::spawn(write_loop(writer, rx));
    let session = Arc::new(Session {
        id: new_session_id(),
        peer: Arc::new(Peer::new(tx, opts.elicitation_timeout)),
        handshake: Mutex::new(Handshake {
            version: HANDSHAKE_VERSIONS[HANDSHAKE_VERSIONS.len() - 1].into(),
            client: None,
            elicitation: false,
        }),
        opts,
        inflight: Mutex::new(HashMap::new()),
    });
    let mut tasks = JoinSet::new();
    let mut lines = BufReader::new(reader).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => on_line(&handler, &session, &mut tasks, &line),
                Ok(None) => break,
                Err(e) => {
                    tracing::warn!("MCP : lecture de l'entrée impossible ({e}), arrêt");
                    break;
                }
            },
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    session.peer.close();
    while tasks.join_next().await.is_some() {}
    session.peer.shutdown();
    drop(session);
    let _ = writer_task.await;
}

async fn write_loop<W: AsyncWrite + Unpin>(mut writer: W, mut rx: mpsc::UnboundedReceiver<Value>) {
    while let Some(msg) = rx.recv().await {
        // serde_json échappe les sauts de ligne : un message tient toujours sur une ligne.
        let Ok(mut line) = serde_json::to_string(&msg) else {
            continue;
        };
        line.push('\n');
        if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
            tracing::warn!("MCP : sortie fermée, messages suivants abandonnés");
            // On vide le canal sans écrire : les émetteurs ne bloquent jamais.
            while rx.recv().await.is_some() {}
            return;
        }
    }
}

fn on_line<H: Handler>(
    handler: &Arc<H>,
    session: &Arc<Session>,
    tasks: &mut JoinSet<()>,
    line: &str,
) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    match serde_json::from_str::<Value>(line) {
        // Lot JSON-RPC (2025-03-26) : chaque message est traité, et répondu, séparément.
        Ok(Value::Array(items)) => {
            for msg in items {
                on_message(handler, session, tasks, msg);
            }
        }
        Ok(msg) => on_message(handler, session, tasks, msg),
        Err(e) => tracing::warn!("MCP : message JSON illisible ignoré ({e})"),
    }
}

fn on_message<H: Handler>(
    handler: &Arc<H>,
    session: &Arc<Session>,
    tasks: &mut JoinSet<()>,
    msg: Value,
) {
    let method = msg
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let id = msg.get("id").filter(|v| !v.is_null()).cloned();
    match (method, id) {
        (Some(method), Some(id)) => start_request(handler, session, tasks, method, id, msg),
        (Some(method), None) => {
            if method == "notifications/cancelled" {
                let target = msg.pointer("/params/requestId").map(id_key);
                if let Some(tx) = target.and_then(|k| lock(&session.inflight).get(&k).cloned()) {
                    let _ = tx.send(true);
                }
            }
            // Autres notifications (`notifications/initialized`…) : rien à faire.
        }
        (None, Some(_)) => {
            if !session.peer.resolve(msg) {
                tracing::debug!("MCP : réponse du client sans requête en attente, ignorée");
            }
        }
        (None, None) => {}
    }
}

fn error_response(id: &Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(d) = data {
        error["data"] = d;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

/// Ère et identité d'une requête, ou la réponse d'erreur à renvoyer tout de suite.
fn request_init(session: &Session, method: &str, msg: &Value) -> Result<CallInit, Value> {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let params = msg.get("params");
    let meta = params.and_then(|p| p.get("_meta"));
    let progress_token = meta
        .and_then(|m| m.get("progressToken"))
        .filter(|t| t.is_string() || t.is_number())
        .cloned();
    let per_request = meta
        .and_then(|m| m.get(META_VERSION))
        .filter(|_| session.opts.stateless);
    if let Some(version) = per_request {
        let supported = supported_versions();
        let Some(version) = version.as_str().filter(|v| supported.contains(v)) else {
            return Err(error_response(
                &id,
                -32022,
                "Unsupported protocol version",
                Some(json!({ "supported": supported, "requested": version })),
            ));
        };
        let Some(caps) = meta
            .and_then(|m| m.get(META_CLIENT_CAPS))
            .filter(|c| c.is_object())
        else {
            return Err(error_response(
                &id,
                -32602,
                "_meta.io.modelcontextprotocol/clientCapabilities requis",
                None,
            ));
        };
        return Ok(CallInit {
            session: session.id.clone(),
            client: client_label(meta.and_then(|m| m.get(META_CLIENT_INFO))),
            version: version.to_string(),
            stateless: true,
            elicitation: form_elicitation(Some(caps)),
            progress_token,
            input_responses: params
                .and_then(|p| p.get("inputResponses"))
                .filter(|r| r.is_object())
                .cloned(),
        });
    }
    let mut hs = lock(&session.handshake);
    if method == "initialize" {
        // Établi AVANT de traiter les requêtes suivantes (elles lisent la session).
        let version = negotiate(
            params
                .and_then(|p| p.get("protocolVersion"))
                .and_then(Value::as_str),
        );
        *hs = Handshake {
            version: version.to_string(),
            client: client_label(params.and_then(|p| p.get("clientInfo"))),
            // L'élicitation n'existe que depuis 2025-06-18.
            elicitation: version >= "2025-06-18"
                && form_elicitation(params.and_then(|p| p.get("capabilities"))),
        };
    }
    Ok(CallInit {
        session: session.id.clone(),
        client: hs.client.clone(),
        version: hs.version.clone(),
        stateless: false,
        elicitation: hs.elicitation,
        progress_token,
        input_responses: None,
    })
}

fn start_request<H: Handler>(
    handler: &Arc<H>,
    session: &Arc<Session>,
    tasks: &mut JoinSet<()>,
    method: String,
    id: Value,
    msg: Value,
) {
    let init = match request_init(session, &method, &msg) {
        Ok(init) => init,
        Err(response) => {
            session.peer.send(response);
            return;
        }
    };
    if init.stateless && method == "initialize" {
        session.peer.send(error_response(
            &id,
            -32601,
            "initialize n'existe pas dans une requête sans état",
            None,
        ));
        return;
    }
    if !session.opts.stateless && method == "server/discover" {
        session
            .peer
            .send(error_response(&id, -32601, "méthode inconnue", None));
        return;
    }
    let key = id_key(&id);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    lock(&session.inflight).insert(key.clone(), cancel_tx);
    let stateless = init.stateless;
    let ctx = Arc::new(CallCtx::new(init, cancel_rx, session.peer.clone()));
    let abortable = handler.abortable(&msg);
    let server_info = handler.server_info();
    let fut = handler.handle(msg);
    let session = session.clone();
    tasks.spawn(async move {
        let outcome = drive(ctx.clone(), fut, abortable, session.opts.heartbeat).await;
        lock(&session.inflight).remove(&key);
        // Requête annulée : aucune réponse (spécification), même si l'outil a fini son travail.
        if ctx.is_cancelled() {
            return;
        }
        let response = match outcome {
            Outcome::Done(Some(response)) if stateless => complete(response, &method, &server_info),
            Outcome::Done(Some(response)) => response,
            Outcome::Done(None) | Outcome::Aborted => return,
            Outcome::Panicked => {
                tracing::error!(method = %method, "MCP : le traitement de la requête a paniqué");
                error_response(&id, -32603, "erreur interne du serveur MCP", None)
            }
            Outcome::InputRequired => match ctx.take_wanted() {
                Some((name, request)) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "resultType": "input_required",
                        "inputRequests": { name: request },
                        "_meta": { META_SERVER_INFO: server_info },
                    }
                }),
                None => error_response(&id, -32603, "saisie demandée introuvable", None),
            },
        };
        session.peer.send(response);
    });
}

enum Outcome {
    Done(Option<Value>),
    /// Ère sans état : l'outil attend une réponse de l'humain (rejeu de la requête).
    InputRequired,
    Aborted,
    /// Panique pendant le traitement : la requête reçoit une erreur, le serveur continue.
    Panicked,
}

async fn drive(
    ctx: Arc<CallCtx>,
    fut: BoxFuture<'static, Option<Value>>,
    abortable: bool,
    heartbeat: Duration,
) -> Outcome {
    use futures_util::FutureExt;
    let call = std::panic::AssertUnwindSafe(crate::mcp_ctx::scope(ctx.clone(), fut)).catch_unwind();
    tokio::pin!(call);
    let mut cancel = ctx.cancel_receiver();
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + heartbeat, heartbeat);
    loop {
        tokio::select! {
            out = &mut call => return out.map_or(Outcome::Panicked, Outcome::Done),
            _ = ctx.input_wanted() => return Outcome::InputRequired,
            _ = wait_cancelled(&mut cancel), if abortable => return Outcome::Aborted,
            _ = beat.tick() => ctx.heartbeat(heartbeat),
        }
    }
}

/// Durée de fraîcheur annoncée (ère sans état) des résultats qui se mettent en cache.
fn cache_ttl_ms(method: &str) -> Option<u64> {
    match method {
        "server/discover" => Some(300_000),
        "tools/list" | "prompts/list" | "resources/list" | "resources/templates/list" => {
            Some(60_000)
        }
        // Le contenu d'une ressource (console, état) change sans cesse.
        "resources/read" => Some(0),
        _ => None,
    }
}

/// Résultat d'une requête sans état : `resultType`, identité du serveur, indications de cache
/// (`private` : données du compte de l'utilisateur).
fn complete(mut response: Value, method: &str, server_info: &Value) -> Value {
    let Some(result) = response.get_mut("result").and_then(Value::as_object_mut) else {
        return response; // erreur JSON-RPC : inchangée
    };
    result
        .entry("resultType")
        .or_insert_with(|| json!("complete"));
    let meta = result.entry("_meta").or_insert_with(|| json!({}));
    if let Some(meta) = meta.as_object_mut() {
        meta.insert(META_SERVER_INFO.into(), server_info.clone());
    }
    if let Some(ttl) = cache_ttl_ms(method) {
        result.entry("ttlMs").or_insert_with(|| json!(ttl));
        result
            .entry("cacheScope")
            .or_insert_with(|| json!("private"));
    }
    response
}

// --- Production : stdio + logique MCP du core ---------------------------------------------------

struct CoreHandler {
    core: Arc<crate::Core>,
}

impl Handler for CoreHandler {
    fn handle(&self, msg: Value) -> BoxFuture<'static, Option<Value>> {
        let core = self.core.clone();
        Box::pin(async move { crate::mcp::handle_message(&core, msg).await })
    }

    fn abortable(&self, msg: &Value) -> bool {
        // Une lecture s'abandonne sans risque ; une action modifiante (default-deny : tout ce qui
        // n'est pas une lecture connue) n'est jamais coupée en plein milieu.
        msg.get("method").and_then(Value::as_str) != Some("tools/call")
            || msg
                .pointer("/params/name")
                .and_then(Value::as_str)
                .is_some_and(|name| !crate::mcp::is_write_tool(name))
    }

    fn server_info(&self) -> Value {
        crate::mcp_catalog::server_info()
    }
}

/// Sert le protocole MCP sur stdio (binaire `minestrator-mcp` et `<app> --mcp`). Lit l'argument
/// `--toolsets` de la ligne de commande du processus (voir `mcp_catalog.rs`).
pub(crate) async fn serve_stdio(core: Arc<crate::Core>) {
    crate::mcp_catalog::set_cli_toolsets(crate::mcp_catalog::toolsets_from_args(std::env::args()));
    let opts = ServeOptions::from_env();
    tracing::info!(
        stateless = opts.stateless,
        "serveur MCP : protocole sur stdio"
    );
    serve(
        Arc::new(CoreHandler { core }),
        tokio::io::stdin(),
        tokio::io::stdout(),
        opts,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_ctx::{self, Confirmation};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, DuplexStream, Lines};

    /// Faux gestionnaire : chaque méthode exerce une partie du contexte d'appel.
    struct Fake {
        /// Passe à vrai avec la méthode `release`.
        released: AtomicBool,
    }

    impl Handler for Arc<Fake> {
        fn handle(&self, msg: Value) -> BoxFuture<'static, Option<Value>> {
            let me = self.clone();
            Box::pin(async move {
                let id = msg.get("id").cloned()?;
                let method = msg["method"].as_str().unwrap_or_default().to_string();
                let result = match method.as_str() {
                    "initialize" => {
                        json!({ "protocolVersion": negotiate(msg.pointer("/params/protocolVersion").and_then(Value::as_str)) })
                    }
                    "server/discover" => json!({ "supportedVersions": supported_versions() }),
                    "whoami" => {
                        json!({ "client": mcp_ctx::client_name(), "session": mcp_ctx::session_id().is_some() })
                    }
                    "slow" => {
                        // Attend d'être libéré ; s'arrête s'il est annulé (outil modifiant).
                        while !me.released.load(Ordering::SeqCst) {
                            if mcp_ctx::cancelled() {
                                return Some(
                                    json!({ "jsonrpc": "2.0", "id": id, "result": { "stopped": true } }),
                                );
                            }
                            tokio::time::sleep(Duration::from_millis(5)).await;
                        }
                        json!({ "released": true })
                    }
                    "release" => {
                        me.released.store(true, Ordering::SeqCst);
                        json!({})
                    }
                    "panic" => panic!("outil en panique"),
                    "confirm" => {
                        let c =
                            mcp_ctx::confirm("Supprimer le serveur « Survie » ?", Some("Survie"))
                                .await;
                        json!({ "confirmation": format!("{c:?}") })
                    }
                    "progress" => {
                        mcp_ctx::progress(1.0, Some(2.0), "moitié").await;
                        mcp_ctx::progress(2.0, Some(2.0), "fini").await;
                        json!({})
                    }
                    _ => return Some(error_response(&id, -32601, "méthode inconnue", None)),
                };
                Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            })
        }

        fn abortable(&self, msg: &Value) -> bool {
            msg["method"] != "slow"
        }

        fn server_info(&self) -> Value {
            json!({ "name": "test", "version": "0" })
        }
    }

    struct Client {
        input: DuplexStream,
        output: Lines<BufReader<DuplexStream>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Client {
        fn start(opts: ServeOptions) -> Self {
            let (client_in, server_in) = tokio::io::duplex(1 << 16);
            let (server_out, client_out) = tokio::io::duplex(1 << 16);
            let handler = Arc::new(Arc::new(Fake {
                released: AtomicBool::new(false),
            }));
            let server = tokio::spawn(serve(handler, server_in, server_out, opts));
            Client {
                input: client_in,
                output: BufReader::new(client_out).lines(),
                server,
            }
        }

        async fn send(&mut self, msg: Value) {
            let mut line = msg.to_string();
            line.push('\n');
            self.input.write_all(line.as_bytes()).await.unwrap();
        }

        async fn recv(&mut self) -> Value {
            let line = tokio::time::timeout(Duration::from_secs(5), self.output.next_line())
                .await
                .expect("réponse attendue")
                .unwrap()
                .expect("sortie fermée");
            serde_json::from_str(&line).unwrap()
        }

        async fn silent(&mut self, ms: u64) -> bool {
            tokio::time::timeout(Duration::from_millis(ms), self.output.next_line())
                .await
                .is_err()
        }
    }

    fn req(id: i64, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn modern_meta(caps: Value) -> Value {
        json!({
            META_VERSION: "2026-07-28",
            META_CLIENT_INFO: { "name": "client-moderne", "version": "1" },
            META_CLIENT_CAPS: caps,
        })
    }

    #[test]
    fn version_negotiation_echoes_known_versions_else_latest_handshake() {
        for v in HANDSHAKE_VERSIONS {
            assert_eq!(negotiate(Some(v)), *v);
        }
        assert_eq!(
            negotiate(Some("2024-11-05")),
            "2024-11-05",
            "agents CLI actuels"
        );
        assert_eq!(negotiate(Some("1999-01-01")), "2025-11-25");
        assert_eq!(
            negotiate(Some("2026-07-28")),
            "2025-11-25",
            "une version sans état ne se négocie pas"
        );
        assert_eq!(negotiate(None), "2025-11-25");
        assert_eq!(supported_versions()[0], "2026-07-28");
    }

    #[test]
    fn form_elicitation_follows_the_capability_shape() {
        assert!(
            form_elicitation(Some(&json!({ "elicitation": {} }))),
            "{{}} = formulaire"
        );
        assert!(form_elicitation(Some(
            &json!({ "elicitation": { "form": {}, "url": {} } })
        )));
        assert!(
            !form_elicitation(Some(&json!({ "elicitation": { "url": {} } }))),
            "URL seule"
        );
        assert!(!form_elicitation(Some(&json!({ "sampling": {} }))));
        assert!(!form_elicitation(None));
    }

    #[tokio::test]
    async fn a_slow_request_does_not_block_the_next_ones() {
        let mut c = Client::start(ServeOptions::default());
        c.send(req(1, "slow", json!({}))).await;
        c.send(req(2, "whoami", json!({}))).await;
        let first = c.recv().await;
        assert_eq!(
            first["id"], 2,
            "la requête rapide répond pendant que la lente attend"
        );
        c.send(req(3, "release", json!({}))).await;
        let mut ids = vec![c.recv().await["id"].clone(), c.recv().await["id"].clone()];
        ids.sort_by_key(|v| v.as_i64());
        assert_eq!(ids, vec![json!(1), json!(3)]);
    }

    #[tokio::test]
    async fn handshake_sets_the_client_for_the_connection() {
        let mut c = Client::start(ServeOptions::default());
        c.send(req(1, "initialize", json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "claude-code\n" } }))).await;
        assert_eq!(c.recv().await["result"]["protocolVersion"], "2024-11-05");
        c.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
            .await;
        c.send(req(2, "whoami", json!({}))).await;
        let r = c.recv().await;
        assert_eq!(
            r["result"],
            json!({ "client": "claude-code", "session": true })
        );
        assert!(
            r["result"].get("resultType").is_none(),
            "ère à poignée de main : résultat inchangé"
        );
    }

    fn cancel(id: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": id, "reason": "test" } })
    }

    #[tokio::test]
    async fn cancelled_requests_get_no_response() {
        let mut c = Client::start(ServeOptions::default());
        // Action modifiante (non abandonnable) : `cancelled()` passe à vrai, l'outil s'arrête de
        // lui-même, et rien n'est répondu.
        c.send(req(2, "slow", json!({}))).await;
        c.send(cancel(json!(2))).await;
        assert!(
            c.silent(200).await,
            "aucune réponse pour une requête annulée"
        );
        c.send(req(3, "whoami", json!({}))).await;
        assert_eq!(c.recv().await["id"], 3);
    }

    #[tokio::test]
    async fn an_abandoned_call_withdraws_its_question() {
        let mut c = Client::start(ServeOptions::default());
        let ask = elicit_handshake(&mut c).await;
        // Appel abandonnable annulé pendant que l'humain réfléchit : la question est retirée côté
        // client, l'appel ne répond pas.
        c.send(cancel(json!(2))).await;
        let withdrawn = c.recv().await;
        assert_eq!(withdrawn["method"], "notifications/cancelled");
        assert_eq!(withdrawn["params"]["requestId"], ask["id"]);
        assert!(c.silent(200).await);
        // Réponse tardive de l'humain : ignorée sans dommage.
        c.send(json!({ "jsonrpc": "2.0", "id": ask["id"], "result": { "action": "accept", "content": { "confirm_name": "Survie" } } })).await;
        c.send(req(3, "whoami", json!({}))).await;
        assert_eq!(c.recv().await["id"], 3);
    }

    #[tokio::test]
    async fn a_panicking_call_gets_an_error_and_the_server_goes_on() {
        let mut c = Client::start(ServeOptions::default());
        c.send(req(1, "panic", json!({}))).await;
        assert_eq!(c.recv().await["error"]["code"], -32603);
        c.send(req(2, "whoami", json!({}))).await;
        assert_eq!(c.recv().await["id"], 2);
    }

    #[tokio::test]
    async fn progress_notifications_carry_the_token() {
        let mut c = Client::start(ServeOptions::default());
        c.send(req(
            1,
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": {} }),
        ))
        .await;
        let _ = c.recv().await;
        c.send(req(
            2,
            "progress",
            json!({ "_meta": { "progressToken": "p-1" } }),
        ))
        .await;
        let a = c.recv().await;
        assert_eq!(a["method"], "notifications/progress");
        assert_eq!(a["params"]["progressToken"], "p-1");
        assert_eq!(a["params"]["message"], "moitié");
        let b = c.recv().await;
        assert_eq!(b["params"]["progress"], 2.0);
        assert_eq!(c.recv().await["id"], 2);
    }

    async fn elicit_handshake(c: &mut Client) -> Value {
        c.send(req(
            1,
            "initialize",
            json!({ "protocolVersion": "2025-11-25", "capabilities": { "elicitation": {} } }),
        ))
        .await;
        let _ = c.recv().await;
        c.send(req(2, "confirm", json!({}))).await;
        let ask = c.recv().await;
        assert_eq!(ask["method"], "elicitation/create");
        assert_eq!(ask["params"]["mode"], "form");
        assert!(ask["params"]["message"]
            .as_str()
            .unwrap()
            .contains("« Survie »"));
        ask
    }

    #[tokio::test]
    async fn handshake_elicitation_round_trip() {
        let mut c = Client::start(ServeOptions::default());
        let ask = elicit_handshake(&mut c).await;
        c.send(json!({ "jsonrpc": "2.0", "id": ask["id"], "result": { "action": "accept", "content": { "confirm_name": "Survie" } } })).await;
        assert_eq!(
            c.recv().await["result"]["confirmation"],
            format!("{:?}", Confirmation::Accepted)
        );

        // Mauvais nom retapé : refus.
        c.send(req(3, "confirm", json!({}))).await;
        let ask = c.recv().await;
        c.send(json!({ "jsonrpc": "2.0", "id": ask["id"], "result": { "action": "accept", "content": { "confirm_name": "survie" } } })).await;
        assert_eq!(
            c.recv().await["result"]["confirmation"],
            format!("{:?}", Confirmation::Declined)
        );

        // Erreur du client : refus.
        c.send(req(4, "confirm", json!({}))).await;
        let ask = c.recv().await;
        c.send(json!({ "jsonrpc": "2.0", "id": ask["id"], "error": { "code": -32601, "message": "non" } })).await;
        assert_eq!(
            c.recv().await["result"]["confirmation"],
            format!("{:?}", Confirmation::Declined)
        );
    }

    #[tokio::test]
    async fn elicitation_timeout_declines_and_cancels_the_question() {
        let opts = ServeOptions {
            elicitation_timeout: Duration::from_millis(50),
            ..ServeOptions::default()
        };
        let mut c = Client::start(opts);
        let ask = elicit_handshake(&mut c).await;
        let cancel = c.recv().await;
        assert_eq!(cancel["method"], "notifications/cancelled");
        assert_eq!(cancel["params"]["requestId"], ask["id"]);
        assert_eq!(
            c.recv().await["result"]["confirmation"],
            format!("{:?}", Confirmation::Declined)
        );
    }

    #[tokio::test]
    async fn end_of_input_answers_pending_requests_and_declines_questions() {
        let mut c = Client::start(ServeOptions::default());
        let _ask = elicit_handshake(&mut c).await;
        c.send(req(3, "whoami", json!({}))).await;
        c.input.shutdown().await.unwrap();
        let mut seen = HashMap::new();
        for _ in 0..2 {
            let r = c.recv().await;
            seen.insert(r["id"].as_i64().unwrap(), r);
        }
        assert_eq!(
            seen[&2]["result"]["confirmation"],
            format!("{:?}", Confirmation::Declined)
        );
        assert!(seen.contains_key(&3));
        tokio::time::timeout(Duration::from_secs(5), c.server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn stateless_requests_are_served_from_their_meta() {
        let mut c = Client::start(ServeOptions::default());
        c.send(req(
            1,
            "server/discover",
            json!({ "_meta": modern_meta(json!({})) }),
        ))
        .await;
        let r = c.recv().await;
        assert_eq!(r["result"]["resultType"], "complete");
        assert_eq!(r["result"]["cacheScope"], "private");
        assert!(r["result"]["ttlMs"].is_u64());
        assert_eq!(r["result"]["_meta"][META_SERVER_INFO]["name"], "test");

        c.send(req(2, "whoami", json!({ "_meta": modern_meta(json!({})) })))
            .await;
        let r = c.recv().await;
        assert_eq!(r["result"]["client"], "client-moderne");
        assert!(
            r["result"].get("ttlMs").is_none(),
            "pas une liste : pas d'indication de cache"
        );

        // Version inconnue : erreur dédiée, versions gérées listées.
        let mut meta = modern_meta(json!({}));
        meta[META_VERSION] = json!("1900-01-01");
        c.send(req(3, "whoami", json!({ "_meta": meta }))).await;
        let r = c.recv().await;
        assert_eq!(r["error"]["code"], -32022);
        assert_eq!(r["error"]["data"]["requested"], "1900-01-01");
        assert!(r["error"]["data"]["supported"]
            .as_array()
            .unwrap()
            .contains(&json!("2024-11-05")));

        // Capacités absentes : requête malformée.
        c.send(req(
            4,
            "whoami",
            json!({ "_meta": { META_VERSION: "2026-07-28" } }),
        ))
        .await;
        assert_eq!(c.recv().await["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn stateless_confirmation_is_a_multi_round_trip() {
        let mut c = Client::start(ServeOptions::default());
        let meta = modern_meta(json!({ "elicitation": { "form": {} } }));
        c.send(req(1, "confirm", json!({ "_meta": meta }))).await;
        let r = c.recv().await;
        assert_eq!(r["result"]["resultType"], "input_required");
        let ask = &r["result"]["inputRequests"]["confirmation-1"];
        assert_eq!(ask["method"], "elicitation/create");
        assert_eq!(
            ask["params"]["requestedSchema"]["required"],
            json!(["confirm_name"])
        );

        // Rejeu (nouvel identifiant) avec la réponse de l'humain.
        let answer = json!({ "confirmation-1": { "action": "accept", "content": { "confirm_name": "Survie" } } });
        c.send(req(
            2,
            "confirm",
            json!({ "_meta": meta, "inputResponses": answer }),
        ))
        .await;
        let r = c.recv().await;
        assert_eq!(
            r["result"]["confirmation"],
            format!("{:?}", Confirmation::Accepted)
        );
        assert_eq!(r["result"]["resultType"], "complete");
    }

    #[tokio::test]
    async fn stateless_era_can_be_switched_off() {
        let mut c = Client::start(ServeOptions {
            stateless: false,
            ..ServeOptions::default()
        });
        c.send(req(
            1,
            "server/discover",
            json!({ "_meta": modern_meta(json!({})) }),
        ))
        .await;
        assert_eq!(
            c.recv().await["error"]["code"],
            -32601,
            "client dual-era : repli sur initialize"
        );
    }

    #[tokio::test]
    async fn malformed_lines_are_skipped() {
        let mut c = Client::start(ServeOptions::default());
        c.input.write_all(b"pas du json\n\n").await.unwrap();
        c.send(json!([
            req(1, "whoami", json!({})),
            req(2, "whoami", json!({}))
        ]))
        .await;
        let mut ids = vec![c.recv().await["id"].as_i64(), c.recv().await["id"].as_i64()];
        ids.sort();
        assert_eq!(ids, vec![Some(1), Some(2)]);
    }
}
