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
            // Suspendu / expiré : contrairement à l'hibernation, ça ne se lève pas tout seul —
            // il faut une action de facturation. Reconnecter en boucle ne ferait que consommer
            // une requête toutes les 30 s sans aucune chance d'aboutir : on s'arrête ici et le
            // front rend l'état. L'utilisateur rouvre l'onglet une fois la situation régularisée.
            Outcome::Forbidden => {
                tracing::info!(conn = %conn_id, "console non connectable : serveur suspendu ou expiré");
                phase(&events, &conn_id, "suspended");
                return;
            }
            Outcome::Disconnected(reason) => {
                tracing::warn!(conn = %conn_id, "console ws déconnectée: {reason}");
                phase(&events, &conn_id, "reconnecting");
                // Mesuré AVANT l'attente : une session qui a tenu longtemps était saine → on repart
                // d'un backoff court au lieu de traîner le délai accumulé par d'anciennes coupures.
                let stable = started.elapsed().as_secs() >= STABLE_S;
                let (wait, next) = reconnect_schedule(backoff, stable);
                if wait_or_shutdown(&mut shutdown, wait).await {
                    phase(&events, &conn_id, "closed");
                    return;
                }
                backoff = next;
            }
        }
    }
}

/// Planning de reconnexion après une coupure : `(attente avant la nouvelle tentative, backoff
/// retenu pour la coupure suivante)`. Doublement plafonné à 30 s ; une session jugée stable
/// ramène le backoff à 1 s **dès cette reconnexion-ci** (sinon elle traînerait le délai accumulé
/// par d'anciennes coupures).
fn reconnect_schedule(backoff: u64, stable: bool) -> (u64, u64) {
    if stable {
        (1, 1)
    } else {
        (backoff, (backoff * 2).min(30))
    }
}

enum Outcome {
    Shutdown,
    Hibernated,
    /// Serveur suspendu ou expiré (403) : état terminal, pas de reconnexion.
    Forbidden,
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
    // Volontairement le détail COMPLET du serveur ici, pas `get_ws_credentials` : seul lui
    // distingue « hiberné » (url/token vides, on n'ouvre pas de WS) de « suspendu/expiré »
    // (l'endpoint léger répond 403 dans les deux cas). Le gain de l'endpoint léger est pris au
    // rafraîchissement de token, bien plus fréquent que l'ouverture.
    let details = match api.get_server(token, server_id).await {
        Ok(d) => d,
        // 403 = suspendu / expiré : distinct d'une panne réseau, et non réessayable.
        Err(crate::Error::Forbidden) => return Outcome::Forbidden,
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
                    Message::Text(txt) => match frame_action(txt.as_str(), conn_id, server_id, monitor) {
                        FrameAction::Emit(event) => {
                            let _ = events.send(event);
                        }
                        FrameAction::Reply(frames) => {
                            for frame in frames {
                                let _ = write.send(Message::text(frame)).await;
                            }
                        }
                        FrameAction::RefreshToken => {
                            // Endpoint dédié : ~1 Kio, contre plusieurs Kio pour le détail
                            // complet du serveur, et ça arrive à chaque expiration de token
                            // sur CHAQUE connexion (le superviseur en tient une par serveur).
                            if let Ok(creds) = api.get_ws_credentials(token, server_id).await {
                                if let Some(fresh) = creds.token {
                                    let _ = write.send(Message::text(json!({ "event": "auth", "args": [fresh] }).to_string())).await;
                                }
                            }
                        }
                        FrameAction::Ignore => {}
                    },
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

/// Ce que la boucle de lecture doit faire d'une trame texte reçue de Wings.
#[derive(Debug)]
enum FrameAction {
    /// Publier cet event sur le bus du cœur.
    Emit(CoreEvent),
    /// Répondre à Wings avec ces trames, dans l'ordre.
    Reply(Vec<String>),
    /// Token WS bientôt (ou déjà) expiré : en redemander un par REST, puis se ré-authentifier.
    RefreshToken,
    /// Trame illisible, sans argument exploitable, ou d'un type non géré.
    Ignore,
}

/// Traduit une trame Wings en action, sans aucune E/S. `monitor` = connexion légère du
/// superviseur : ne demande pas les logs et ne relaie que `stats`/`status`, plus les lignes
/// WARN/ERROR (en `ConsoleLog`, pour le Copilote).
fn frame_action(text: &str, conn_id: &str, server_id: i64, monitor: bool) -> FrameAction {
    let Some(evt) = WingsEvent::parse(text) else {
        return FrameAction::Ignore;
    };
    let output = |line: &str| {
        FrameAction::Emit(CoreEvent::ConsoleOutput(ConsoleOutput {
            conn_id: conn_id.to_string(),
            line: line.to_string(),
        }))
    };
    match evt.event.as_str() {
        "auth success" => {
            let mut frames = Vec::with_capacity(2);
            if !monitor {
                frames.push(json!({ "event": "send logs", "args": [Value::Null] }).to_string());
            }
            frames.push(json!({ "event": "send stats", "args": [Value::Null] }).to_string());
            FrameAction::Reply(frames)
        }
        "console output" => match evt.first_str() {
            // Connexion monitor : on ne relaie pas le flux, mais on repère les lignes WARN/ERROR
            // pour le Copilote.
            Some(line) if monitor => match log_level(line) {
                Some(level) => FrameAction::Emit(CoreEvent::ConsoleLog(ConsoleLog {
                    server_id,
                    level: level.to_string(),
                    line: line.to_string(),
                })),
                None => FrameAction::Ignore,
            },
            Some(line) => output(line),
            None => FrameAction::Ignore,
        },
        "status" => match evt.first_str() {
            Some(state) => FrameAction::Emit(CoreEvent::ConsoleStatus(ConsoleStatus {
                conn_id: conn_id.to_string(),
                state: state.to_string(),
            })),
            None => FrameAction::Ignore,
        },
        "stats" => match evt.first_str().and_then(|raw| parse_stats(conn_id, raw)) {
            Some(stats) => FrameAction::Emit(CoreEvent::ConsoleStats(stats)),
            None => FrameAction::Ignore,
        },
        // Sortie de l'installateur Wings (déploiement de l'egg) : sans ça, un serveur en cours
        // d'installation affiche une console vide.
        "install output" => match evt.first_str() {
            Some(line) if !monitor => output(line),
            _ => FrameAction::Ignore,
        },
        "token expiring" | "token expired" => FrameAction::RefreshToken,
        _ => FrameAction::Ignore,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(event: &str, args: Value) -> String {
        json!({ "event": event, "args": args }).to_string()
    }

    /// Action pour une connexion d'onglet (`monitor = false`) ou du superviseur (`true`).
    fn action(text: &str, monitor: bool) -> FrameAction {
        frame_action(text, "tab-1", 42, monitor)
    }

    fn emitted(a: FrameAction) -> CoreEvent {
        match a {
            FrameAction::Emit(ev) => ev,
            other => panic!("event attendu, obtenu {other:?}"),
        }
    }

    fn replies(a: FrameAction) -> Vec<Value> {
        match a {
            FrameAction::Reply(frames) => frames
                .iter()
                .map(|f| serde_json::from_str(f).expect("trame JSON"))
                .collect(),
            other => panic!("réponse attendue, obtenu {other:?}"),
        }
    }

    fn is_ignored(a: &FrameAction) -> bool {
        matches!(a, FrameAction::Ignore)
    }

    // --- Trames Wings -------------------------------------------------------------

    #[test]
    fn parses_wings_frames() {
        let e = WingsEvent::parse(r#"{"event":"console output","args":["[12:00 INFO]: Done"]}"#)
            .unwrap();
        assert_eq!(e.event, "console output");
        assert_eq!(e.first_str(), Some("[12:00 INFO]: Done"));
        // `args` absent (ex. `auth success`) → liste vide.
        let e = WingsEvent::parse(r#"{"event":"auth success"}"#).unwrap();
        assert!(e.args.is_empty());
        assert_eq!(e.first_str(), None);
        // Premier argument non textuel.
        let e = WingsEvent::parse(r#"{"event":"x","args":[42,"y"]}"#).unwrap();
        assert_eq!(e.first_str(), None);
        // Trames invalides.
        assert!(WingsEvent::parse("pas du json").is_none());
        assert!(WingsEvent::parse(r#"{"args":["x"]}"#).is_none());
        assert!(WingsEvent::parse(r#"{"event":null}"#).is_none());
    }

    #[test]
    fn auth_success_requests_logs_then_stats() {
        assert_eq!(
            replies(action(&frame("auth success", json!([])), false)),
            [
                json!({ "event": "send logs", "args": [null] }),
                json!({ "event": "send stats", "args": [null] }),
            ]
        );
        // Connexion monitor : pas d'historique de logs, seulement les stats.
        assert_eq!(
            replies(action(r#"{"event":"auth success"}"#, true)),
            [json!({ "event": "send stats", "args": [null] })]
        );
    }

    #[test]
    fn console_output_is_relayed_to_tabs() {
        match emitted(action(&frame("console output", json!(["[12:00 INFO]: Done (3.2s)!"])), false)) {
            CoreEvent::ConsoleOutput(o) => {
                assert_eq!(o.conn_id, "tab-1");
                assert_eq!(o.line, "[12:00 INFO]: Done (3.2s)!");
            }
            other => panic!("{other:?}"),
        }
        assert!(is_ignored(&action(&frame("console output", json!([])), false)));
    }

    #[test]
    fn monitor_only_reports_warn_and_error_lines() {
        assert!(is_ignored(&action(
            &frame("console output", json!(["[12:00 INFO]: Steve joined the game"])),
            true
        )));
        match emitted(action(
            &frame("console output", json!(["[12:00 WARN]: Can't keep up! Is the server overloaded?"])),
            true,
        )) {
            CoreEvent::ConsoleLog(l) => {
                assert_eq!(l.server_id, 42);
                assert_eq!(l.level, "warn");
                assert_eq!(l.line, "[12:00 WARN]: Can't keep up! Is the server overloaded?");
            }
            other => panic!("{other:?}"),
        }
        match emitted(action(
            &frame("console output", json!(["[Server thread/ERROR]: Encountered an unexpected exception"])),
            true,
        )) {
            CoreEvent::ConsoleLog(l) => assert_eq!(l.level, "error"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn status_is_relayed_in_both_modes() {
        for monitor in [false, true] {
            match emitted(action(&frame("status", json!(["running"])), monitor)) {
                CoreEvent::ConsoleStatus(s) => {
                    assert_eq!(s.conn_id, "tab-1");
                    assert_eq!(s.state, "running");
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(is_ignored(&action(&frame("status", json!([])), false)));
    }

    #[test]
    fn stats_frames_are_decoded() {
        // Wings encode les stats en CHAÎNE JSON dans `args[0]`.
        let raw = json!({
            "memory_bytes": 1_610_612_736u64,
            "memory_limit_bytes": 4_294_967_296u64,
            "cpu_absolute": 87.512,
            "network": { "rx_bytes": 1, "tx_bytes": 2 },
            "uptime": 3_600_000,
            "state": "running",
            "disk_bytes": 2_147_483_648u64
        })
        .to_string();
        for monitor in [false, true] {
            match emitted(action(&frame("stats", json!([raw])), monitor)) {
                CoreEvent::ConsoleStats(s) => {
                    assert_eq!(s.conn_id, "tab-1");
                    assert_eq!(s.cpu_absolute, 87.512);
                    assert_eq!(s.memory_bytes, 1_610_612_736);
                    assert_eq!(s.memory_limit_bytes, 4_294_967_296);
                    assert_eq!(s.disk_bytes, 2_147_483_648);
                    assert_eq!(s.uptime, 3_600_000);
                    assert_eq!(s.state, "running");
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(is_ignored(&action(&frame("stats", json!(["{pas du json"])), false)));
        assert!(is_ignored(&action(&frame("stats", json!([])), false)));
    }

    #[test]
    fn parse_stats_defaults_missing_fields() {
        let s = parse_stats("mon:7", r#"{"state":"starting"}"#).unwrap();
        assert_eq!(s.conn_id, "mon:7");
        assert_eq!(s.state, "starting");
        assert_eq!((s.cpu_absolute, s.memory_bytes, s.memory_limit_bytes), (0.0, 0, 0));
        assert_eq!((s.disk_bytes, s.uptime), (0, 0));
        // Valeurs de mauvais type → 0 plutôt qu'un rejet de toute la trame.
        let s = parse_stats("c", r#"{"memory_bytes":-1,"cpu_absolute":"12","uptime":1.5}"#).unwrap();
        assert_eq!((s.memory_bytes, s.cpu_absolute, s.uptime), (0, 0.0, 0));
        assert!(parse_stats("c", "").is_none());
    }

    #[test]
    fn install_output_only_reaches_tabs() {
        match emitted(action(&frame("install output", json!(["Installing server jar..."])), false)) {
            CoreEvent::ConsoleOutput(o) => assert_eq!(o.line, "Installing server jar..."),
            other => panic!("{other:?}"),
        }
        assert!(is_ignored(&action(&frame("install output", json!(["x"])), true)));
    }

    #[test]
    fn token_expiry_triggers_a_refresh() {
        for event in ["token expiring", "token expired"] {
            for monitor in [false, true] {
                assert!(matches!(
                    action(&frame(event, json!([])), monitor),
                    FrameAction::RefreshToken
                ));
            }
        }
    }

    #[test]
    fn unknown_or_invalid_frames_are_ignored() {
        for text in [
            frame("daemon message", json!(["Server marked as running..."])),
            frame("backup completed", json!([])),
            frame("jwt error", json!(["expired"])),
            "pas du json".to_string(),
            "{}".to_string(),
        ] {
            assert!(is_ignored(&action(&text, false)), "{text}");
        }
    }

    // --- Niveaux de log ----------------------------------------------------------------

    #[test]
    fn log_level_recognizes_java_and_minecraft_formats() {
        assert_eq!(log_level("[12:00:00 ERROR]: x"), Some("error"));
        assert_eq!(log_level("[Server thread/ERROR]: x"), Some("error"));
        assert_eq!(log_level("SEVERE: Could not load plugin"), Some("error"));
        assert_eq!(log_level("FATAL: out of memory"), Some("error"));
        assert_eq!(log_level("[12:00:00 WARN]: Can't keep up!"), Some("warn"));
        assert_eq!(log_level("[Server thread/WARN]: x"), Some("warn"));
        assert_eq!(log_level("WARNING: An illegal reflective access"), Some("warn"));
        // L'erreur l'emporte sur l'avertissement dans une même ligne.
        assert_eq!(log_level("WARN: ERROR inside"), Some("error"));
        // Lignes colorées ANSI (telles que Wings les relaie).
        assert_eq!(log_level("\u{1b}[33m[12:00:00 WARN]: lent\u{1b}[m"), Some("warn"));
        assert_eq!(log_level("[12:00:00 INFO]: Steve joined the game"), None);
        assert_eq!(log_level(""), None);
    }

    #[test]
    fn log_level_requires_whole_uppercase_words() {
        // Pas de faux positif dans un identifiant ou un mot plus long.
        assert_eq!(log_level("[INFO]: ERRORS_COUNT=0"), None);
        assert_eq!(log_level("[INFO]: NO_ERROR"), None);
        assert_eq!(log_level("[INFO]: WARNED players"), None);
        assert_eq!(log_level("[INFO]: ÉERROR"), None);
        // Sensible à la casse : un message de joueur « error » n'est pas un niveau de log.
        assert_eq!(log_level("<Steve> error partout"), None);
        // Une occurrence collée puis une vraie occurrence plus loin.
        assert_eq!(log_level("ERRORERROR ERROR"), Some("error"));
        assert_eq!(log_level("é ERROR"), Some("error"));
    }

    #[test]
    fn contains_word_bounds() {
        assert!(contains_word("ERROR", "ERROR"));
        assert!(contains_word("x ERROR", "ERROR"));
        assert!(contains_word("ERROR]", "ERROR"));
        assert!(!contains_word("", "ERROR"));
        assert!(!contains_word("ERR", "ERROR"));
        assert!(!contains_word("_ERROR", "ERROR"));
        assert!(!contains_word("ERROR1", "ERROR"));
    }

    // --- Backoff de reconnexion ---------------------------------------------------------

    #[test]
    fn backoff_doubles_up_to_30_seconds() {
        let mut backoff = 1;
        let mut waits = Vec::new();
        for _ in 0..8 {
            let (wait, next) = reconnect_schedule(backoff, false);
            waits.push(wait);
            backoff = next;
        }
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
    }

    #[test]
    fn stable_session_resets_the_backoff() {
        assert_eq!(reconnect_schedule(1, true), (1, 1));
        assert_eq!(reconnect_schedule(16, true).1, 1);
        assert_eq!(reconnect_schedule(30, true).1, 1);
    }

    #[test]
    fn stable_session_reconnects_quickly() {
        let (wait, _) = reconnect_schedule(30, true);
        assert_eq!(wait, 1);
    }
}
