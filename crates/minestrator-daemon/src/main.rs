//! Daemon de surveillance + push FCM.
//!
//! Réutilise `minestrator-core` exactement comme le desktop : il crée un `Core`, démarre le
//! **superviseur** (qui émet des `CoreEvent::Alert` sur crash / seuils CPU-RAM-disque / expiration)
//! et pousse chaque alerte en **notification FCM** aux appareils enregistrés — de quoi être prévenu
//! **app fermée**, ce que le mobile ne peut pas faire seul (pas de tâche de fond persistante).

mod config;
mod fcm;
mod tokens;

use config::{DaemonConfig, FcmConfig};
use minestrator_core::{Core, CoreEvent};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

#[tokio::main]
async fn main() {
    init_tracing();

    let cfg = match DaemonConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration invalide : {e}\nVoir crates/minestrator-daemon/README.md");
            std::process::exit(1);
        }
    };

    // Headless : le core stocke ses secrets en fichier (pas de Secret Service/D-Bus requis).
    std::env::set_var("MINESTRATOR_SECRETS_FILE", "1");
    std::env::set_var("MINESTRATOR_DATA_DIR", &cfg.data_dir);
    let _ = std::fs::create_dir_all(&cfg.data_dir);

    let core = Arc::new(Core::new());

    // Valide la clé (réseau) + la met en cache/stockage pour le superviseur.
    match core.authenticate_and_store(&cfg.api_key).await {
        Ok(u) => tracing::info!(user = %u.pseudo, "clé API validée"),
        Err(e) => {
            tracing::error!("authentification impossible : {e}");
            std::process::exit(1);
        }
    }

    if cfg.fcm.is_none() {
        tracing::warn!(
            "FCM non configuré (FCM_PROJECT_ID manquant) : les alertes seront journalisées mais PAS poussées."
        );
    }

    let mut events = core.subscribe();

    // Superviseur : surveille les serveurs et émet des CoreEvent::Alert (tâches tokio internes).
    let supervisor = core.supervisor();
    supervisor.start();
    tracing::info!("daemon démarré — surveillance active");

    let client = reqwest::Client::new();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("arrêt demandé — au revoir");
                break;
            }
            ev = events.recv() => match ev {
                Ok(CoreEvent::Alert(a)) => {
                    tracing::info!(
                        server = %a.server_name, kind = %a.kind, severity = %a.severity,
                        "alerte : {}", a.message
                    );
                    if let Some(fcm_cfg) = &cfg.fcm {
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
                        push_to_devices(&client, &cfg, fcm_cfg, &title, &a.message, data).await;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => tracing::warn!("{n} events perdus"),
                Err(RecvError::Closed) => break,
            }
        }
    }
}

async fn push_to_devices(
    client: &reqwest::Client,
    cfg: &DaemonConfig,
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
    let devices = tokens::load(cfg.device_tokens_file.as_deref());
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

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("minestrator_daemon=info,minestrator_core=info,warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
