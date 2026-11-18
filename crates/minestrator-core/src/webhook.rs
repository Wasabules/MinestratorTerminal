//! Alertes vers l'extérieur : **webhook Discord** (embeds) et **webhook JSON générique**.
//!
//! - **Destinations** : configuration non secrète (id, nom, type, événements choisis) persistée
//!   dans `webhooks.json` ; l'**URL est un secret** (une URL Discord suffit à poster dans le salon)
//!   rangée par [`crate::secrets`] (`webhook-url-<id>`), jamais dans un fichier de réglages ni dans
//!   un journal — les erreurs réseau sont journalisées **sans** URL (`reqwest::Error::without_url`).
//! - **Événements** : crash, redémarrage automatique, seuils CPU/RAM/disque, échéance MyBox,
//!   réussite et échec des tâches planifiées. Chaque destination choisit les siens.
//! - **Anonymisation** : tout ce qui part passe par [`crate::redact`].
//! - **Anti-spam** : par destination, les événements sont regroupés sur [`GROUP_WINDOW_S`]
//!   secondes, un envoi au plus toutes les [`MIN_INTERVAL_S`] secondes, file bornée (les plus
//!   anciens sont résumés en « N autres événements »). Les alertes du superviseur ont déjà leurs
//!   propres délais anti-répétition.
//! - **Une seule exécution** : comme les automatisations, seul le superviseur actif envoie. L'app
//!   en veille ignore les alertes relayées depuis le service — c'est le service qui les a envoyées
//!   (il reçoit destinations et URL par IPC : `set_webhooks`, voir [`crate::service`]).

use crate::automation::new_id;
use crate::error::{Error, Result};
use crate::events::{Alert, CoreEvent};
use crate::persist::PersistedConfig;
use crate::store::now_secs;
use crate::{redact, secrets, Core};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

/// Fenêtre de regroupement des événements d'une destination.
pub const GROUP_WINDOW_S: i64 = 5;
/// Écart minimal entre deux envois vers une même destination.
pub const MIN_INTERVAL_S: i64 = 10;
/// Événements en attente par destination au plus (au-delà, les plus anciens sont résumés).
const QUEUE_CAP: usize = 50;
/// Événements par message : 10 embeds (limite Discord), 25 en JSON.
const DISCORD_MAX_EMBEDS: usize = 10;
const JSON_MAX_EVENTS: usize = 25;
/// Budget de caractères des embeds d'un message (Discord : 6 000 au plus).
const DISCORD_BUDGET: usize = 5500;
const MAX_URL_LEN: usize = 2048;
const MAX_DESTINATIONS: usize = 10;
const MAX_NAME_LEN: usize = 60;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// Hôtes acceptés pour un webhook Discord.
const DISCORD_HOSTS: &[&str] = &["discord.com", "discordapp.com", "ptb.discord.com", "canary.discord.com"];

// --- Configuration ------------------------------------------------------------------------

/// Événement transmis aux destinations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum WebhookEvent {
    Crash,
    AutoRestart,
    /// Seuils CPU / RAM / disque.
    Threshold,
    /// Échéance d'une MyBox.
    Mybox,
    TaskSuccess,
    TaskFailure,
}

impl WebhookEvent {
    pub const ALL: [WebhookEvent; 6] = [
        WebhookEvent::Crash,
        WebhookEvent::AutoRestart,
        WebhookEvent::Threshold,
        WebhookEvent::Mybox,
        WebhookEvent::TaskSuccess,
        WebhookEvent::TaskFailure,
    ];

    fn label(self) -> &'static str {
        match self {
            WebhookEvent::Crash => "Crash",
            WebhookEvent::AutoRestart => "Redémarrage automatique",
            WebhookEvent::Threshold => "Seuil dépassé",
            WebhookEvent::Mybox => "Échéance MyBox",
            WebhookEvent::TaskSuccess => "Tâche réussie",
            WebhookEvent::TaskFailure => "Échec de tâche",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            WebhookEvent::Crash => "💥",
            WebhookEvent::AutoRestart => "🔁",
            WebhookEvent::Threshold => "📈",
            WebhookEvent::Mybox => "📦",
            WebhookEvent::TaskSuccess => "✅",
            WebhookEvent::TaskFailure => "❌",
        }
    }
}

/// Format d'envoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum WebhookKind {
    /// Message Discord à embeds.
    Discord,
    /// `POST` JSON générique (`{ source, version, events: […], omitted }`).
    Json,
}

/// Destination d'alertes (partie **non secrète** : l'URL est dans le stockage sécurisé).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WebhookDestination {
    /// Attribué par le cœur (vide à la création).
    pub id: String,
    pub name: String,
    pub kind: WebhookKind,
    pub enabled: bool,
    pub events: Vec<WebhookEvent>,
}

/// Destinations configurées (persistées dans `webhooks.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WebhookConfig {
    pub destinations: Vec<WebhookDestination>,
}

/// Destination vue par l'UI : jamais l'URL, seulement sa présence.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WebhookInfo {
    pub destination: WebhookDestination,
    pub has_url: bool,
}

// --- Événements et formats (purs) -----------------------------------------------------------

/// Un événement à transmettre.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Notice {
    pub event: WebhookEvent,
    /// `info` | `warning` | `critical`
    pub severity: String,
    pub server_id: i64,
    pub server_name: String,
    pub message: String,
    pub details: Option<String>,
    pub ts: i64,
}

impl Notice {
    /// Alerte du superviseur ou des automatisations → événement (`None` : non transmise).
    pub(crate) fn from_alert(a: &Alert) -> Option<Self> {
        let event = match a.kind.as_str() {
            "crash" => WebhookEvent::Crash,
            "auto_restart" => WebhookEvent::AutoRestart,
            "cpu" | "ram" | "disk" => WebhookEvent::Threshold,
            "expiry" => WebhookEvent::Mybox,
            "task" => WebhookEvent::TaskFailure,
            _ => return None,
        };
        Some(Notice {
            event,
            severity: a.severity.clone(),
            server_id: a.server_id,
            server_name: a.server_name.clone(),
            message: a.message.clone(),
            details: a.details.clone(),
            ts: a.ts,
        })
    }

    /// Copie anonymisée : rien ne sort sans passer par `redact`.
    fn redacted(&self) -> Self {
        Notice {
            server_name: redact::redact(&self.server_name),
            message: redact::redact(&self.message),
            details: self.details.as_deref().map(redact::redact),
            ..self.clone()
        }
    }
}

/// Destinations abonnées à un événement.
pub(crate) fn subscribers(cfg: &WebhookConfig, event: WebhookEvent) -> Vec<&WebhookDestination> {
    cfg.destinations
        .iter()
        .filter(|d| d.enabled && d.events.contains(&event))
        .collect()
}

fn color(severity: &str) -> u32 {
    match severity {
        "critical" => 0xff5f57,
        "warning" => 0xf59e0b,
        _ => 0x009b72,
    }
}

/// Tronque au plus `max` octets (frontière de caractère), avec `…`.
fn clip(s: &str, max: usize) -> String {
    crate::util::truncate_on_boundary(s, max.saturating_sub(3), "…")
}

/// Garde la **fin** d'un texte (logs : la cause est à la fin), au plus `max` octets.
fn clip_tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut cut = s.len() - max.saturating_sub(4);
    while !s.is_char_boundary(cut) {
        cut += 1;
    }
    format!("…\n{}", &s[cut..])
}

fn rfc3339(ts: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.to_rfc3339())
}

/// Message Discord : un embed par événement (couleur selon la gravité), détail en bloc de code,
/// **aucune mention** interprétée (`allowed_mentions`) — un pseudo ou une ligne de log contenant
/// `@everyone` ne doit rien déclencher. `title` : remplace le titre (message de test).
pub(crate) fn discord_payload(batch: &[Notice], omitted: usize, title: Option<&str>) -> Value {
    let mut embeds = Vec::new();
    let mut used = 0;
    let mut omitted = omitted;
    for n in batch.iter().take(DISCORD_MAX_EMBEDS).map(Notice::redacted) {
        let title = match title {
            Some(t) => clip(t, 256),
            None => clip(&format!("{} {} — {}", n.event.icon(), n.server_name, n.event.label()), 256),
        };
        let message = clip(&n.message, 1000);
        let full = match n.details.as_deref() {
            // Un ``` dans les logs fermerait le bloc de code.
            Some(d) => format!("{message}\n```\n{}\n```", clip_tail(&d.replace("```", "'''"), 900)),
            None => message.clone(),
        };
        let description = if used + title.len() + full.len() <= DISCORD_BUDGET {
            full
        } else if used + title.len() + message.len() <= DISCORD_BUDGET {
            message
        } else {
            omitted += 1;
            continue;
        };
        used += title.len() + description.len();
        let mut embed = json!({
            "title": title,
            "description": description,
            "color": color(&n.severity),
            "footer": { "text": "Minestrator Terminal" },
        });
        if let Some(ts) = rfc3339(n.ts) {
            embed["timestamp"] = json!(ts);
        }
        embeds.push(embed);
    }
    omitted += batch.len().saturating_sub(DISCORD_MAX_EMBEDS);
    let mut payload = json!({
        "username": "Minestrator Terminal",
        "embeds": embeds,
        "allowed_mentions": { "parse": [] },
    });
    if omitted > 0 {
        payload["content"] = json!(format!(
            "… et {omitted} autre(s) événement(s) non détaillé(s) (limitation du débit)."
        ));
    }
    payload
}

/// Corps JSON générique.
pub(crate) fn json_payload(batch: &[Notice], omitted: usize) -> Value {
    let events: Vec<Value> = batch
        .iter()
        .take(JSON_MAX_EVENTS)
        .map(Notice::redacted)
        .map(|n| {
            json!({
                "event": n.event,
                "severity": n.severity,
                "server_id": n.server_id,
                "server_name": n.server_name,
                "message": n.message,
                "details": n.details,
                "ts": n.ts,
            })
        })
        .collect();
    json!({
        "source": "minestrator-terminal",
        "version": env!("CARGO_PKG_VERSION"),
        "events": events,
        "omitted": omitted + batch.len().saturating_sub(JSON_MAX_EVENTS),
    })
}

fn payload_for(kind: WebhookKind, batch: &[Notice], omitted: usize, title: Option<&str>) -> Value {
    match kind {
        WebhookKind::Discord => discord_payload(batch, omitted, title),
        WebhookKind::Json => json_payload(batch, omitted),
    }
}

fn max_batch(kind: WebhookKind) -> usize {
    match kind {
        WebhookKind::Discord => DISCORD_MAX_EMBEDS,
        WebhookKind::Json => JSON_MAX_EVENTS,
    }
}

/// File d'attente d'une destination : regroupement + limitation du débit (pur, horloge injectée).
#[derive(Debug, Default)]
pub(crate) struct Outbox {
    queue: VecDeque<Notice>,
    first_at: Option<i64>,
    last_sent: Option<i64>,
    dropped: usize,
}

impl Outbox {
    pub(crate) fn push(&mut self, n: Notice, now: i64) {
        if self.queue.len() >= QUEUE_CAP {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back(n);
        self.first_at.get_or_insert(now);
    }

    /// Lot prêt à partir (au plus `max` événements) et nombre d'événements écartés à résumer.
    pub(crate) fn take(&mut self, now: i64, max: usize) -> Option<(Vec<Notice>, usize)> {
        let first = self.first_at?;
        if now - first < GROUP_WINDOW_S || self.last_sent.is_some_and(|t| now - t < MIN_INTERVAL_S) {
            return None;
        }
        let n = self.queue.len().min(max.max(1));
        let batch: Vec<Notice> = self.queue.drain(..n).collect();
        let omitted = std::mem::take(&mut self.dropped);
        self.last_sent = Some(now);
        self.first_at = (!self.queue.is_empty()).then_some(now);
        Some((batch, omitted))
    }
}

// --- Validation (pure) --------------------------------------------------------------------

fn invalid(msg: impl Into<String>) -> Error {
    Error::Unexpected(msg.into())
}

/// URL acceptable pour ce type de destination (renvoyée nettoyée). Discord : `https://`,
/// domaine Discord, chemin `/api/webhooks/…`. JSON : `http(s)://` vers un hôte quelconque (un
/// récepteur local, par exemple).
pub fn validate_url(kind: WebhookKind, raw: &str) -> Result<String> {
    let url = raw.trim();
    if url.is_empty() || url.len() > MAX_URL_LEN || url.chars().any(char::is_whitespace) {
        return Err(invalid("URL de webhook invalide."));
    }
    let parsed = reqwest::Url::parse(url).map_err(|_| invalid("URL de webhook invalide."))?;
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() {
        return Err(invalid("URL de webhook invalide."));
    }
    match kind {
        WebhookKind::Discord => {
            if parsed.scheme() != "https"
                || !DISCORD_HOSTS.contains(&host.as_str())
                || !parsed.path().starts_with("/api/webhooks/")
            {
                return Err(invalid(
                    "URL de webhook Discord attendue (https://discord.com/api/webhooks/…).",
                ));
            }
        }
        WebhookKind::Json => {
            if !matches!(parsed.scheme(), "https" | "http") {
                return Err(invalid("URL http(s) attendue."));
            }
        }
    }
    Ok(url.to_string())
}

/// Identifiant de destination sûr (il entre dans un nom de compte du stockage sécurisé).
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Nom nettoyé, événements dédoublonnés dans l'ordre canonique.
pub(crate) fn normalize_destination(mut d: WebhookDestination) -> Result<WebhookDestination> {
    d.name = d.name.trim().chars().take(MAX_NAME_LEN).collect();
    if d.name.is_empty() {
        return Err(invalid("Donne un nom à la destination."));
    }
    d.events = WebhookEvent::ALL.into_iter().filter(|e| d.events.contains(e)).collect();
    Ok(d)
}

// --- Envoi ---------------------------------------------------------------------------------

fn client() -> reqwest::Client {
    crate::http::builder()
        .timeout(SEND_TIMEOUT)
        .user_agent(crate::config::USER_AGENT)
        .build()
        .unwrap_or_else(|_| crate::http::client())
}

/// `POST` du corps ; un `429` est réessayé une fois après le délai demandé (borné). Les erreurs ne
/// contiennent **jamais** l'URL.
async fn post(client: &reqwest::Client, url: &str, payload: &Value) -> Result<()> {
    for attempt in 0..2 {
        let resp = client
            .post(url)
            .json(payload)
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        if status.as_u16() == 429 && attempt == 0 {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse::<f64>().ok())
                .map(|s| Duration::from_secs_f64(s.clamp(0.0, MAX_RETRY_AFTER.as_secs_f64())))
                .unwrap_or(Duration::from_secs(2));
            tokio::time::sleep(wait).await;
            continue;
        }
        return Err(invalid(format!("le webhook a répondu HTTP {}", status.as_u16())));
    }
    Err(invalid("le webhook limite le débit (HTTP 429)"))
}

// --- État et façade --------------------------------------------------------------------------

/// État des webhooks, porté par le [`Core`].
pub(crate) struct WebhookState {
    config: PersistedConfig<WebhookConfig>,
    /// Événements hors alertes (réussite d'une tâche) vers le répartiteur, une fois démarré.
    notices: Mutex<Option<mpsc::UnboundedSender<Notice>>>,
}

impl WebhookState {
    pub(crate) fn load(dir: &Path) -> Self {
        Self {
            config: PersistedConfig::load(dir, "webhooks.json"),
            notices: Mutex::new(None),
        }
    }
}

impl Core {
    /// Destinations configurées (sans leurs URL).
    pub fn webhooks(&self) -> Vec<WebhookInfo> {
        self.webhooks
            .config
            .get()
            .destinations
            .into_iter()
            .map(|d| WebhookInfo {
                has_url: secrets::read_webhook_url(&d.id).ok().flatten().is_some(),
                destination: d,
            })
            .collect()
    }

    /// Crée (`id` vide) ou modifie une destination. `url` : nouvelle URL (obligatoire à la
    /// création ; `None` ou vide = conserver l'actuelle), rangée dans le stockage sécurisé.
    pub async fn save_webhook(&self, dest: WebhookDestination, url: Option<String>) -> Result<WebhookInfo> {
        let mut dest = normalize_destination(dest)?;
        let mut cfg = self.webhooks.config.get();
        let is_new = dest.id.is_empty();
        if is_new {
            if cfg.destinations.len() >= MAX_DESTINATIONS {
                return Err(invalid(format!("{MAX_DESTINATIONS} destinations au plus.")));
            }
        } else if !cfg.destinations.iter().any(|d| d.id == dest.id) {
            return Err(invalid("Destination introuvable."));
        }
        let url = url.filter(|u| !u.trim().is_empty());
        let url = match url {
            Some(u) => Some(validate_url(dest.kind, &u)?),
            None if is_new => return Err(invalid("URL du webhook requise.")),
            None => {
                // Type changé sans nouvelle URL : l'actuelle doit convenir au nouveau type.
                if let Some(existing) = secrets::read_webhook_url(&dest.id)? {
                    validate_url(dest.kind, &existing)?;
                }
                None
            }
        };
        if is_new {
            dest.id = new_id("wh");
        }
        if let Some(u) = &url {
            secrets::store_webhook_url(&dest.id, u)?;
        }
        match cfg.destinations.iter_mut().find(|d| d.id == dest.id) {
            Some(existing) => *existing = dest.clone(),
            None => cfg.destinations.push(dest.clone()),
        }
        self.webhooks.config.set(cfg);
        self.sync_webhooks().await;
        let has_url = secrets::read_webhook_url(&dest.id).ok().flatten().is_some();
        Ok(WebhookInfo { destination: dest, has_url })
    }

    /// Supprime une destination et son URL.
    pub async fn delete_webhook(&self, id: &str) -> Result<()> {
        let mut cfg = self.webhooks.config.get();
        cfg.destinations.retain(|d| d.id != id);
        self.webhooks.config.set(cfg);
        secrets::delete_webhook_url(id)?;
        self.sync_webhooks().await;
        Ok(())
    }

    /// Envoie un message de test, tout de suite (sans file ni regroupement) : à l'URL saisie si
    /// fournie, sinon à celle enregistrée pour `id`.
    pub async fn test_webhook(&self, kind: WebhookKind, url: Option<String>, id: Option<String>) -> Result<()> {
        let url = match url.filter(|u| !u.trim().is_empty()) {
            Some(u) => validate_url(kind, &u)?,
            None => {
                let id = id.ok_or_else(|| invalid("URL du webhook requise."))?;
                secrets::read_webhook_url(&id)?.ok_or_else(|| invalid("Aucune URL enregistrée."))?
            }
        };
        let notice = Notice {
            event: WebhookEvent::TaskSuccess,
            severity: "info".into(),
            server_id: 0,
            server_name: "Minestrator Terminal".into(),
            message: "Message de test : les alertes de Minestrator Terminal arriveront ici.".into(),
            details: None,
            ts: now_secs(),
        };
        let payload = payload_for(kind, &[notice], 0, Some("🔔 Minestrator Terminal — test"));
        post(&client(), &url, &payload).await
    }

    /// Destinations (toutes) — transmises au service d'arrière-plan.
    pub fn webhook_config(&self) -> WebhookConfig {
        self.webhooks.config.get()
    }

    /// URL enregistrées, par destination — transmises au service par IPC, jamais ailleurs.
    pub(crate) fn webhook_urls(&self) -> BTreeMap<String, String> {
        self.webhooks
            .config
            .get()
            .destinations
            .iter()
            .filter_map(|d| Some((d.id.clone(), secrets::read_webhook_url(&d.id).ok().flatten()?)))
            .collect()
    }

    /// Remplace destinations et URL (réception par le service d'arrière-plan). Les URL des
    /// destinations retirées sont effacées.
    pub fn apply_webhooks(&self, cfg: WebhookConfig, urls: BTreeMap<String, String>) -> Result<()> {
        if cfg.destinations.len() > MAX_DESTINATIONS || cfg.destinations.iter().any(|d| !valid_id(&d.id)) {
            return Err(invalid("destinations invalides"));
        }
        for d in &cfg.destinations {
            if let Some(u) = urls.get(&d.id) {
                validate_url(d.kind, u)?;
            }
        }
        let old = self.webhooks.config.get();
        for d in &cfg.destinations {
            if let Some(u) = urls.get(&d.id) {
                secrets::store_webhook_url(&d.id, u)?;
            }
        }
        for d in old.destinations.iter().filter(|o| !cfg.destinations.iter().any(|n| n.id == o.id)) {
            secrets::delete_webhook_url(&d.id)?;
        }
        self.webhooks.config.set(cfg);
        Ok(())
    }

    /// Transmet un événement hors alerte (réussite d'une tâche) aux destinations abonnées.
    pub(crate) fn notify_webhooks(&self, n: Notice) {
        if let Some(tx) = self.webhooks.notices.lock().unwrap().as_ref() {
            let _ = tx.send(n);
        }
    }

    /// Démarre le répartiteur (dans un runtime tokio) : alertes du bus + événements des
    /// automatisations → files par destination → envois groupés.
    pub(crate) fn start_webhooks(self: Arc<Self>) {
        let (tx, mut rx) = mpsc::unbounded_channel();
        *self.webhooks.notices.lock().unwrap() = Some(tx);
        let mut bus = self.subscribe();
        tokio::spawn(async move {
            let client = client();
            let mut outboxes: HashMap<String, Outbox> = HashMap::new();
            let mut tick = tokio::time::interval(FLUSH_EVERY);
            loop {
                tokio::select! {
                    ev = bus.recv() => match ev {
                        Ok(CoreEvent::Alert(a)) => {
                            // App en veille : l'alerte est relayée depuis le service, qui l'a déjà
                            // transmise lui-même.
                            if self.sup_state.is_suspended() {
                                continue;
                            }
                            if let Some(n) = Notice::from_alert(&a) {
                                self.route(&mut outboxes, n);
                            }
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    Some(n) = rx.recv() => self.route(&mut outboxes, n),
                    _ = tick.tick() => self.flush(&client, &mut outboxes),
                }
            }
        });
    }

    fn route(&self, outboxes: &mut HashMap<String, Outbox>, n: Notice) {
        let now = now_secs();
        self.webhooks.config.with(|cfg| {
            for d in subscribers(cfg, n.event) {
                outboxes.entry(d.id.clone()).or_default().push(n.clone(), now);
            }
        });
    }

    fn flush(&self, client: &reqwest::Client, outboxes: &mut HashMap<String, Outbox>) {
        let now = now_secs();
        let cfg = self.webhooks.config.get();
        outboxes.retain(|id, _| cfg.destinations.iter().any(|d| &d.id == id && d.enabled));
        for (id, outbox) in outboxes.iter_mut() {
            let Some(dest) = cfg.destinations.iter().find(|d| &d.id == id) else { continue };
            let Some((batch, omitted)) = outbox.take(now, max_batch(dest.kind)) else { continue };
            let url = match secrets::read_webhook_url(id) {
                Ok(Some(u)) => u,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(destination = %dest.name, "URL du webhook illisible : {e}");
                    continue;
                }
            };
            let payload = payload_for(dest.kind, &batch, omitted, None);
            let (client, name) = (client.clone(), dest.name.clone());
            tokio::spawn(async move {
                if let Err(e) = post(&client, &url, &payload).await {
                    tracing::warn!(destination = %name, "envoi du webhook échoué : {e}");
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notice(event: WebhookEvent, severity: &str) -> Notice {
        Notice {
            event,
            severity: severity.into(),
            server_id: 42,
            server_name: "Survie".into(),
            message: "« Survie » s'est arrêté de façon inattendue.".into(),
            details: None,
            ts: 1_763_460_000,
        }
    }

    fn dest(id: &str, enabled: bool, events: Vec<WebhookEvent>) -> WebhookDestination {
        WebhookDestination { id: id.into(), name: format!("dest {id}"), kind: WebhookKind::Discord, enabled, events }
    }

    fn alert(kind: &str) -> Alert {
        Alert {
            server_id: 1,
            server_name: "Survie".into(),
            kind: kind.into(),
            severity: "warning".into(),
            message: "m".into(),
            ts: 5,
            details: Some("d".into()),
        }
    }

    #[test]
    fn alertes_vers_evenements() {
        let map = |k: &str| Notice::from_alert(&alert(k)).map(|n| n.event);
        assert_eq!(map("crash"), Some(WebhookEvent::Crash));
        assert_eq!(map("auto_restart"), Some(WebhookEvent::AutoRestart));
        assert_eq!(map("cpu"), Some(WebhookEvent::Threshold));
        assert_eq!(map("ram"), Some(WebhookEvent::Threshold));
        assert_eq!(map("disk"), Some(WebhookEvent::Threshold));
        assert_eq!(map("expiry"), Some(WebhookEvent::Mybox));
        assert_eq!(map("task"), Some(WebhookEvent::TaskFailure));
        // Notes locales de l'app (service activé…) : jamais transmises.
        assert_eq!(map("service"), None);
        let n = Notice::from_alert(&alert("crash")).unwrap();
        assert_eq!((n.server_id, n.ts, n.details.as_deref()), (1, 5, Some("d")));
    }

    #[test]
    fn filtrage_par_evenement_et_activation() {
        let cfg = WebhookConfig {
            destinations: vec![
                dest("a", true, vec![WebhookEvent::Crash, WebhookEvent::TaskFailure]),
                dest("b", true, vec![WebhookEvent::Threshold]),
                dest("c", false, vec![WebhookEvent::Crash]),
            ],
        };
        let ids = |e| subscribers(&cfg, e).into_iter().map(|d| d.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids(WebhookEvent::Crash), vec!["a"], "une destination désactivée ne reçoit rien");
        assert_eq!(ids(WebhookEvent::Threshold), vec!["b"]);
        assert!(ids(WebhookEvent::TaskSuccess).is_empty());
    }

    #[test]
    fn format_discord() {
        let mut n = notice(WebhookEvent::Crash, "critical");
        n.details = Some("[ERROR] OutOfMemoryError\n@everyone ```injection```".into());
        let p = discord_payload(&[n], 0, None);
        assert_eq!(p["username"], "Minestrator Terminal");
        // Aucune mention interprétée, même si un log contient @everyone.
        assert_eq!(p["allowed_mentions"], json!({ "parse": [] }));
        let e = &p["embeds"][0];
        assert_eq!(e["title"], "💥 Survie — Crash");
        assert_eq!(e["color"], 0xff5f57);
        assert_eq!(e["timestamp"], "2025-11-18T10:00:00+00:00");
        let desc = e["description"].as_str().unwrap();
        assert!(desc.starts_with("« Survie » s'est arrêté de façon inattendue.\n```\n"));
        assert!(desc.ends_with("\n```"));
        // Le ``` des logs ne ferme pas le bloc de code.
        assert_eq!(desc.matches("```").count(), 2, "{desc}");
        assert!(p.get("content").is_none());

        let info = discord_payload(&[notice(WebhookEvent::TaskSuccess, "info")], 0, Some("Test"));
        assert_eq!(info["embeds"][0]["color"], 0x009b72);
        assert_eq!(info["embeds"][0]["title"], "Test");
        assert_eq!(discord_payload(&[notice(WebhookEvent::Threshold, "warning")], 0, None)["embeds"][0]["color"], 0xf59e0b);
    }

    #[test]
    fn discord_respecte_ses_limites() {
        // 12 événements avec de gros détails : 10 embeds au plus, budget de caractères tenu, le
        // reste résumé dans `content`.
        let batch: Vec<Notice> = (0..12)
            .map(|i| Notice { details: Some("x".repeat(2000)), server_id: i, ..notice(WebhookEvent::Crash, "critical") })
            .collect();
        let p = discord_payload(&batch, 3, None);
        let embeds = p["embeds"].as_array().unwrap();
        assert!(embeds.len() <= DISCORD_MAX_EMBEDS);
        let total: usize = embeds
            .iter()
            .map(|e| e["title"].as_str().unwrap().len() + e["description"].as_str().unwrap().len())
            .sum();
        assert!(total <= DISCORD_BUDGET, "{total}");
        // Les premiers gardent leur détail (tronqué par la fin), les suivants seulement le message.
        assert!(embeds[0]["description"].as_str().unwrap().contains("```"));
        assert!(!embeds.last().unwrap()["description"].as_str().unwrap().contains("```"));
        let content = p["content"].as_str().unwrap();
        assert!(content.contains("5 autre(s)"), "3 écartés de la file + 2 au-delà de 10 : {content}");
    }

    #[test]
    fn format_json_generique() {
        let mut n = notice(WebhookEvent::TaskFailure, "warning");
        n.details = Some("erreur".into());
        let p = json_payload(&[n], 2);
        assert_eq!(p["source"], "minestrator-terminal");
        assert_eq!(p["omitted"], 2);
        let e = &p["events"][0];
        assert_eq!(e["event"], "task_failure");
        assert_eq!(e["severity"], "warning");
        assert_eq!(e["server_id"], 42);
        assert_eq!(e["details"], "erreur");
        assert_eq!(e["ts"], 1_763_460_000);
    }

    #[test]
    fn tout_ce_qui_sort_est_anonymise() {
        let mut n = notice(WebhookEvent::Crash, "critical");
        n.server_name = "Serveur 203.0.113.9".into();
        n.message = "contact admin@example.org".into();
        n.details = Some("[INFO] Alex[/198.51.100.4:5000] logged in\nrcon.password=hunter2".into());
        for p in [discord_payload(&[n.clone()], 0, None), json_payload(&[n.clone()], 0)] {
            let s = p.to_string();
            for secret in ["203.0.113.9", "admin@example.org", "198.51.100.4", "hunter2"] {
                assert!(!s.contains(secret), "{secret} a fuité : {s}");
            }
        }
    }

    #[test]
    fn regroupement_et_limitation_du_debit() {
        let mut ob = Outbox::default();
        assert!(ob.take(0, 10).is_none(), "rien en attente");
        ob.push(notice(WebhookEvent::Crash, "critical"), 100);
        ob.push(notice(WebhookEvent::Threshold, "warning"), 102);
        // Fenêtre de regroupement pas écoulée.
        assert!(ob.take(104, 10).is_none());
        let (batch, omitted) = ob.take(105, 10).unwrap();
        assert_eq!((batch.len(), omitted), (2, 0), "les deux événements partent ensemble");
        // Un nouvel événement juste après : retenu jusqu'à MIN_INTERVAL_S après l'envoi.
        ob.push(notice(WebhookEvent::Crash, "critical"), 106);
        assert!(ob.take(111, 10).is_none());
        assert_eq!(ob.take(115, 10).unwrap().0.len(), 1);
        assert!(ob.take(200, 10).is_none(), "file vide");
    }

    #[test]
    fn file_bornee_et_lots_limites() {
        let mut ob = Outbox::default();
        for i in 0..(QUEUE_CAP + 7) {
            ob.push(Notice { server_id: i as i64, ..notice(WebhookEvent::Crash, "critical") }, 0);
        }
        let (batch, omitted) = ob.take(GROUP_WINDOW_S, 10).unwrap();
        assert_eq!(omitted, 7, "les plus anciens sont résumés");
        assert_eq!(batch.len(), 10);
        assert_eq!(batch[0].server_id, 7);
        // Le reste part au lot suivant, après l'intervalle minimal.
        assert!(ob.take(GROUP_WINDOW_S + 1, 10).is_none());
        let (next, omitted) = ob.take(GROUP_WINDOW_S + MIN_INTERVAL_S, 10).unwrap();
        assert_eq!((next.len(), omitted), (10, 0));
    }

    #[test]
    fn validation_des_url() {
        let ok = "https://discord.com/api/webhooks/123/abc";
        assert_eq!(validate_url(WebhookKind::Discord, &format!("  {ok} ")).unwrap(), ok);
        assert!(validate_url(WebhookKind::Discord, "https://canary.discord.com/api/webhooks/1/x").is_ok());
        assert!(validate_url(WebhookKind::Discord, "http://discord.com/api/webhooks/1/x").is_err(), "https exigé");
        assert!(validate_url(WebhookKind::Discord, "https://evil.example/api/webhooks/1/x").is_err());
        assert!(validate_url(WebhookKind::Discord, "https://discord.com.evil.example/api/webhooks/1").is_err());
        assert!(validate_url(WebhookKind::Discord, "https://discord.com/channels/1").is_err());
        assert!(validate_url(WebhookKind::Json, "https://hooks.example.net/alertes").is_ok());
        assert!(validate_url(WebhookKind::Json, "http://192.168.1.10:8123/api/webhook/x").is_ok());
        assert!(validate_url(WebhookKind::Json, "ftp://example.net/x").is_err());
        assert!(validate_url(WebhookKind::Json, "pas une url").is_err());
        assert!(validate_url(WebhookKind::Json, "").is_err());
        assert!(validate_url(WebhookKind::Json, "https://a.example/x y").is_err());
    }

    #[test]
    fn normalisation_des_destinations() {
        let d = normalize_destination(WebhookDestination {
            id: String::new(),
            name: "  Salon staff  ".into(),
            kind: WebhookKind::Json,
            enabled: true,
            events: vec![WebhookEvent::TaskFailure, WebhookEvent::Crash, WebhookEvent::Crash],
        })
        .unwrap();
        assert_eq!(d.name, "Salon staff");
        assert_eq!(d.events, vec![WebhookEvent::Crash, WebhookEvent::TaskFailure]);
        assert!(normalize_destination(WebhookDestination { name: "  ".into(), ..d.clone() }).is_err());
        assert!(valid_id("wh-18a2b3c") && !valid_id("") && !valid_id("a b") && !valid_id(&"a".repeat(65)));
    }

    #[test]
    fn forme_json_de_la_configuration() {
        let cfg = WebhookConfig { destinations: vec![dest("wh-1", true, vec![WebhookEvent::AutoRestart])] };
        let v = serde_json::to_value(&cfg).unwrap();
        assert_eq!(
            v,
            json!({ "destinations": [{ "id": "wh-1", "name": "dest wh-1", "kind": "discord",
                "enabled": true, "events": ["auto_restart"] }] })
        );
        // Aucune URL dans la configuration persistée.
        assert!(!v.to_string().contains("http"));
        let back: WebhookConfig = serde_json::from_value(v).unwrap();
        assert_eq!(back, cfg);
    }
}
