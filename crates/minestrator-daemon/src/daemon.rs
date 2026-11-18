//! Boucle principale du daemon, commune au premier plan (`run`) et au service (`service`).
//!
//! Elle crée un `Core` sur le dossier de données machine, démarre le superviseur et les
//! automatisations (redémarrage automatique, tâches planifiées, webhooks), consigne chaque alerte
//! (relue par l'app via IPC), la pousse en FCM si configuré, et sert le canal IPC.

use crate::config::{EnvConfig, FcmConfig};
use crate::{fcm, ipc, tokens};
use minestrator_core::service::{ServiceStatus, PROTOCOL_VERSION};
use minestrator_core::{Core, CoreEvent};
use serde_json::json;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;

/// Paramètres résolus avant le démarrage du runtime.
pub struct Options {
    /// Lancé par systemd / launchd / le SCM.
    pub managed: bool,
    /// Comptes autorisés sur le canal IPC (`--owner`).
    pub owners: Vec<String>,
    pub data_dir: PathBuf,
    pub endpoint: String,
}

/// Pourquoi la boucle s'est arrêtée.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Arrêt demandé (signal, SCM).
    Stop,
    /// Redémarrage demandé par l'app : le gestionnaire de services relance le processus.
    Restart,
    /// Erreur fatale (clé refusée au démarrage, point d'écoute IPC impossible en service…).
    Fatal,
}

/// État partagé avec le serveur IPC.
pub struct Shared {
    pub core: Arc<Core>,
    pub managed: bool,
    started_at: i64,
    data_dir: String,
    fcm: bool,
    account: Mutex<Option<(i64, String)>>,
    restart: Notify,
}

impl Shared {
    pub fn new(core: Arc<Core>, managed: bool, data_dir: &std::path::Path, fcm: bool) -> Self {
        Self {
            core,
            managed,
            started_at: now(),
            data_dir: data_dir.to_string_lossy().into_owned(),
            fcm,
            account: Mutex::new(None),
            restart: Notify::new(),
        }
    }

    pub fn status(&self) -> ServiceStatus {
        let account = self.account.lock().unwrap().clone();
        ServiceStatus {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            started_at: self.started_at,
            managed: self.managed,
            has_key: self.core.has_key().unwrap_or(false),
            account_id: account.as_ref().map(|(id, _)| *id),
            account_name: account.map(|(_, name)| name),
            supervisor_enabled: self.core.get_supervisor_config().enabled,
            heartbeat: self.core.supervisor_heartbeat(),
            data_dir: self.data_dir.clone(),
            fcm: self.fcm,
        }
    }

    pub fn set_account(&self, account: Option<(i64, String)>) {
        *self.account.lock().unwrap() = account;
    }

    /// Redémarrage différé : laisse à la réponse IPC le temps de partir.
    pub fn request_restart(&self) {
        tracing::info!("redémarrage demandé par l'app");
        self.restart.notify_one();
    }
}

/// Exécute le daemon jusqu'à `shutdown` (ou un redémarrage demandé).
///
/// Prérequis (posés par `main` avant la création du runtime) : `MINESTRATOR_DATA_DIR` et
/// `MINESTRATOR_SECRETS_FILE`, pour que le core range historique et secrets dans le dossier
/// machine plutôt que dans le trousseau d'un utilisateur.
pub async fn run(opts: Options, env: EnvConfig, shutdown: impl Future<Output = ()>) -> Exit {
    let core = Arc::new(Core::new());
    let shared = Arc::new(Shared::new(core.clone(), opts.managed, &opts.data_dir, env.fcm.is_some()));

    // Clé fournie par l'environnement (usage historique : VPS, conteneur) : validée au démarrage.
    if let Some(key) = &env.api_key {
        match core.authenticate_and_store(key).await {
            Ok(u) => {
                tracing::info!(user = %u.pseudo, "clé API (environnement) validée");
                shared.set_account(Some((u.id, u.pseudo)));
            }
            Err(e) => {
                tracing::error!("clé API de l'environnement refusée : {e}");
                return Exit::Fatal;
            }
        }
    } else if !core.has_key().unwrap_or(false) {
        tracing::warn!(
            "aucune clé API : en attente d'activation depuis l'app (IPC) ou de MINESTRATOR_API_KEY"
        );
    }

    if env.fcm.is_none() {
        tracing::info!("FCM non configuré : les alertes sont consignées et journalisées, sans push");
    }

    let mut events = core.subscribe();
    core.supervisor().start();
    // Automatisations (redémarrage automatique, tâches planifiées) et webhooks : reçus de l'app par
    // IPC, exécutés ici tant que le service supervise (l'app, elle, se met alors en veille).
    core.clone().start_automations();
    tracing::info!(data_dir = %shared.data_dir, managed = opts.managed, "daemon démarré");

    // Compte surveillé (affiché par l'app, et condition pour qu'elle lui délègue la supervision).
    tokio::spawn(resolve_account(shared.clone()));

    // Canal IPC. En service, il est indispensable (c'est par lui que l'app s'efface) ; au premier
    // plan, il est facultatif (VPS sans app).
    let mut ipc_task = tokio::spawn(ipc::serve(shared.clone(), opts.endpoint.clone(), opts.owners.clone()));

    let client = minestrator_core::http::client();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                tracing::info!("arrêt demandé");
                return Exit::Stop;
            }
            _ = shared.restart.notified() => {
                // Laisse partir la réponse IPC avant de quitter.
                tokio::time::sleep(Duration::from_millis(300)).await;
                return Exit::Restart;
            }
            res = &mut ipc_task => {
                let err = match res {
                    Ok(Ok(())) => "le serveur IPC s'est arrêté".to_string(),
                    Ok(Err(e)) => e.to_string(),
                    Err(e) => e.to_string(),
                };
                if opts.managed {
                    tracing::error!(endpoint = %opts.endpoint, "IPC indisponible : {err}");
                    return Exit::Fatal;
                }
                tracing::warn!(endpoint = %opts.endpoint, "IPC indisponible (sans conséquence au premier plan) : {err}");
                // Ne plus sonder cette branche : une tâche qui ne finit jamais.
                ipc_task = tokio::spawn(std::future::pending::<std::io::Result<()>>());
            }
            ev = events.recv() => match ev {
                Ok(CoreEvent::Alert(a)) => {
                    if let Err(e) = core.record_alert(&a) {
                        tracing::warn!("alerte non consignée : {e}");
                    }
                    tracing::info!(
                        server = %a.server_name, kind = %a.kind, severity = %a.severity,
                        "alerte : {}", a.message
                    );
                    if let Some(fcm_cfg) = &env.fcm {
                        let title = if a.severity == "critical" {
                            format!("⚠ {}", a.server_name)
                        } else {
                            a.server_name.clone()
                        };
                        let data = json!({
                            "server_id": a.server_id.to_string(),
                            "kind": a.kind,
                            "severity": a.severity,
                        });
                        push_to_devices(&client, &env, fcm_cfg, &title, &a.message, data).await;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => tracing::warn!("{n} events perdus"),
                Err(RecvError::Closed) => return Exit::Fatal,
            }
        }
    }
}

/// Résout le compte de la clé enregistrée (au démarrage, puis toutes les minutes tant qu'il
/// manque : réseau absent au boot, clé arrivée par IPC sans compte…).
async fn resolve_account(shared: Arc<Shared>) {
    loop {
        let missing = shared.account.lock().unwrap().is_none();
        if missing && shared.core.has_key().unwrap_or(false) {
            match shared.core.get_user().await {
                Ok(u) => shared.set_account(Some((u.id, u.pseudo))),
                Err(e) => tracing::debug!("compte non résolu : {e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

async fn push_to_devices(
    client: &reqwest::Client,
    env: &EnvConfig,
    fcm_cfg: &FcmConfig,
    title: &str,
    body: &str,
    data: serde_json::Value,
) {
    let access = match fcm::access_token(fcm_cfg) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("access token FCM indisponible : {e}");
            return;
        }
    };
    let devices = tokens::load(env.device_tokens_file.as_deref());
    if devices.is_empty() {
        tracing::warn!("aucun appareil enregistré (DEVICE_TOKENS_FILE) — push ignoré");
        return;
    }
    for device in devices {
        match fcm::send(client, &fcm_cfg.project_id, &access, &device, title, body, data.clone()).await
        {
            Ok(()) => tracing::info!("push envoyé (…{})", tail(&device)),
            Err(e) => tracing::warn!("push échoué (…{}) : {e}", tail(&device)),
        }
    }
}

/// Derniers caractères d'un token, pour des logs non sensibles.
fn tail(s: &str) -> &str {
    let start = s.len().saturating_sub(6);
    s.get(start..).unwrap_or(s)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
