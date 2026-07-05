//! Gestionnaire de connexions console (WebSocket Pterodactyl Wings).
//!
//! Indépendant de toute UI : publie des `CoreEvent` sur un canal `broadcast`.
//! Une connexion par `conn_id` ; `Origin: https://minestrator.com` obligatoire ;
//! reconnexion avec backoff ; ré-auth sur `token expiring`/`token expired`.

use crate::api::ApiClient;
use crate::config::WS_ORIGIN;
use crate::events::{
    ConsoleConnection, ConsoleLog, ConsoleOutput, ConsoleStats, ConsoleStatus, CoreEvent,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

pub struct ConsoleManager {
    conns: Mutex<HashMap<String, mpsc::Sender<()>>>,
    events: broadcast::Sender<CoreEvent>,
}

impl ConsoleManager {
    pub fn new(events: broadcast::Sender<CoreEvent>) -> Self {
        Self {
            conns: Mutex::new(HashMap::new()),
            events,
        }
    }

    /// `monitor = true` : connexion légère du superviseur (ne demande pas les logs et
    /// n'émet pas de `console output`) ; on ne relaie que `stats`/`status`.
    pub fn connect(
        &self,
        api: ApiClient,
        token: String,
        conn_id: String,
        server_id: i64,
        monitor: bool,
    ) {
        self.disconnect(&conn_id);
        let (tx, rx) = mpsc::channel::<()>(1);
        self.conns.lock().unwrap().insert(conn_id.clone(), tx);
        let events = self.events.clone();
        tokio::spawn(async move {
            supervise(events, api, token, conn_id, server_id, monitor, rx).await;
        });
    }

    pub fn disconnect(&self, conn_id: &str) {
        if let Some(tx) = self.conns.lock().unwrap().remove(conn_id) {
            let _ = tx.try_send(());
        }
    }
}

fn phase(events: &broadcast::Sender<CoreEvent>, conn_id: &str, phase: &str) {
    let _ = events.send(CoreEvent::ConsoleConnection(ConsoleConnection {
        conn_id: conn_id.to_string(),
        phase: phase.to_string(),
    }));
}

#[allow(clippy::too_many_arguments)]
async fn supervise(
    events: broadcast::Sender<CoreEvent>,
    api: ApiClient,
    token: String,
    conn_id: String,
    server_id: i64,
    monitor: bool,
    mut shutdown: mpsc::Receiver<()>,
) {
    // Au-delà de ce seuil, une session est jugée « saine » → le backoff repart à zéro.
    const STABLE_S: u64 = 30;
    let mut backoff = 1u64;
    loop {
        phase(&events, &conn_id, "connecting");
        let started = Instant::now();
        match run_once(&events, &api, &token, &conn_id, server_id, monitor, &mut shutdown).await {
            Outcome::Shutdown => {
                phase(&events, &conn_id, "closed");
                return;
            }
            Outcome::Hibernated => {
                phase(&events, &conn_id, "hibernated");
                if wait_or_shutdown(&mut shutdown, 15).await {
                    return;
                }
            }
            Outcome::Disconnected(reason) => {
                tracing::warn!(conn = %conn_id, "console ws déconnectée: {reason}");
                phase(&events, &conn_id, "reconnecting");
                // Mesuré AVANT l'attente : une session qui a tenu longtemps était saine → on repart
                // d'un backoff court au lieu de traîner le délai accumulé par d'anciennes coupures.
                let stable = started.elapsed().as_secs() >= STABLE_S;
                if wait_or_shutdown(&mut shutdown, backoff).await {
                    phase(&events, &conn_id, "closed");
                    return;
                }
                backoff = if stable { 1 } else { (backoff * 2).min(30) };
            }
        }
    }
}

enum Outcome {
    Shutdown,
    Hibernated,
    Disconnected(String),
}

#[allow(clippy::too_many_arguments)]
async fn run_once(
    events: &broadcast::Sender<CoreEvent>,
    api: &ApiClient,
    token: &str,
    conn_id: &str,
    server_id: i64,
    monitor: bool,
    shutdown: &mut mpsc::Receiver<()>,
) -> Outcome {
    let details = match api.get_server(token, server_id).await {
        Ok(d) => d,
        Err(e) => return Outcome::Disconnected(format!("détails serveur: {e:?}")),
    };
    let (url, ws_token) = match (details.ws_url, details.ws_token) {
        (Some(u), Some(t)) => (u, t),
        _ => return Outcome::Hibernated,
    };

    let mut request = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => return Outcome::Disconnected(e.to_string()),
    };
    request
        .headers_mut()
        .insert("Origin", HeaderValue::from_static(WS_ORIGIN));

    let stream = match tokio_tungstenite::connect_async(request).await {
        Ok((s, _)) => s,
        Err(e) => return Outcome::Disconnected(e.to_string()),
    };
    let (mut write, mut read) = stream.split();

    if write
        .send(Message::text(json!({ "event": "auth", "args": [ws_token] }).to_string()))
        .await
        .is_err()
    {
        return Outcome::Disconnected("échec de l'envoi d'auth".into());
    }
    phase(events, conn_id, "open");

    loop {
        tokio::select! {
            _ = shutdown.recv() => return Outcome::Shutdown,
            incoming = read.next() => {
                let msg = match incoming {
                    Some(Ok(m)) => m,
                    Some(Err(e)) => return Outcome::Disconnected(e.to_string()),
                    None => return Outcome::Disconnected("flux fermé".into()),
                };
                match msg {
                    Message::Text(txt) => {
                        let Some(evt) = WingsEvent::parse(txt.as_str()) else { continue };
                        match evt.event.as_str() {
                            "auth success" => {
                                if !monitor {
                                    let _ = write.send(Message::text(json!({ "event": "send logs", "args": [Value::Null] }).to_string())).await;
                                }
                                let _ = write.send(Message::text(json!({ "event": "send stats", "args": [Value::Null] }).to_string())).await;
                            }
                            "console output" => {
                                if let Some(line) = evt.first_str() {
                                    if monitor {
                                        // Connexion monitor : on ne relaie pas le flux, mais on
                                        // repère les lignes WARN/ERROR pour le Copilote.
                                        if let Some(level) = log_level(line) {
                                            let _ = events.send(CoreEvent::ConsoleLog(ConsoleLog { server_id, level: level.to_string(), line: line.to_string() }));
                                        }
                                    } else {
                                        let _ = events.send(CoreEvent::ConsoleOutput(ConsoleOutput { conn_id: conn_id.to_string(), line: line.to_string() }));
                                    }
                                }
                            }
                            "status" => {
                                if let Some(state) = evt.first_str() {
                                    let _ = events.send(CoreEvent::ConsoleStatus(ConsoleStatus { conn_id: conn_id.to_string(), state: state.to_string() }));
                                }
                            }
                            "stats" => {
                                if let Some(raw) = evt.first_str() {
                                    if let Some(stats) = parse_stats(conn_id, raw) {
                                        let _ = events.send(CoreEvent::ConsoleStats(stats));
                                    }
                                }
                            }
                            "token expiring" | "token expired" => {
                                if let Ok(d) = api.get_server(token, server_id).await {
                                    if let Some(fresh) = d.ws_token {
                                        let _ = write.send(Message::text(json!({ "event": "auth", "args": [fresh] }).to_string())).await;
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Message::Ping(payload) => {
                        let _ = write.send(Message::Pong(payload)).await;
                    }
                    Message::Close(_) => return Outcome::Disconnected("fermé par le serveur".into()),
                    _ => {}
                }
            }
        }
    }
}

async fn wait_or_shutdown(shutdown: &mut mpsc::Receiver<()>, secs: u64) -> bool {
    tokio::select! {
        _ = shutdown.recv() => true,
        _ = tokio::time::sleep(Duration::from_secs(secs)) => false,
    }
}

#[derive(serde::Deserialize)]
struct WingsEvent {
    event: String,
    #[serde(default)]
    args: Vec<Value>,
}

impl WingsEvent {
    fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }
    fn first_str(&self) -> Option<&str> {
        self.args.first().and_then(|v| v.as_str())
    }
}

/// Classe une ligne de console Java/Minecraft en niveau `error`/`warn`, sinon `None`.
/// Aligné sur le filtre de la console (frontière de mot `\bERROR\b`), pour repérer tous les
/// formats (`[.../ERROR]`, `[12:34 ERROR]`, `ERROR:` …), pas seulement `/ERROR]`.
fn log_level(line: &str) -> Option<&'static str> {
    if contains_word(line, "ERROR") || contains_word(line, "SEVERE") || contains_word(line, "FATAL")
    {
        Some("error")
    } else if contains_word(line, "WARNING") || contains_word(line, "WARN") {
        Some("warn")
    } else {
        None
    }
}

/// Cherche `word` en tant que mot entier (bornes non alphanumériques, `_` compris), sans regex.
fn contains_word(haystack: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(pos) = haystack[from..].find(word) {
        let start = from + pos;
        let end = start + word.len();
        let before_ok = haystack[..start].chars().next_back().is_none_or(|c| !is_word(c));
        let after_ok = haystack[end..].chars().next().is_none_or(|c| !is_word(c));
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

fn parse_stats(conn_id: &str, raw: &str) -> Option<ConsoleStats> {
    let v: Value = serde_json::from_str(raw).ok()?;
    Some(ConsoleStats {
        conn_id: conn_id.to_string(),
        cpu_absolute: v.get("cpu_absolute").and_then(|x| x.as_f64()).unwrap_or(0.0),
        memory_bytes: v.get("memory_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
        memory_limit_bytes: v.get("memory_limit_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
        disk_bytes: v.get("disk_bytes").and_then(|x| x.as_u64()).unwrap_or(0),
        uptime: v.get("uptime").and_then(|x| x.as_u64()).unwrap_or(0),
        state: v.get("state").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}
