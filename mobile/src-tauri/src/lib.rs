//! Frontend mobile (Tauri) : couche d'adaptation au-dessus de `minestrator-core`.
//!
//! Version allégée du crate desktop : **pas de tray, d'updater, ni de fenêtres détachées**
//! (l'OS mobile ne les fournit pas). On garde l'essentiel : exposer le `Core` au webview
//! via des commandes, et relayer les `CoreEvent` (broadcast) → events Tauri + notifications.
//!
//! Ce qui change vs desktop (à venir) :
//! - `secrets` : Keychain/Keystore au lieu du trousseau desktop ;
//! - alertes hors-app : **daemon + push FCM/APNs** (le superviseur de fond n'existe pas ici).

mod commands;

use minestrator_core::{Core, CoreEvent};
use std::sync::Arc;
use tauri::Emitter;
use tauri_plugin_notification::NotificationExt;
use tokio::sync::broadcast::error::RecvError;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let core = Arc::new(Core::new());
    let events = core.subscribe();
    let supervisor = core.supervisor();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .manage(core)
        .setup(move |app| {
            let handle = app.handle().clone();

            // Pont : CoreEvent (broadcast) → events Tauri + notifications locales.
            let mut events = events;
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(ev) => forward(&handle, ev),
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!("pont d'events en retard : {n} events perdus")
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
            });

            // Superviseur : actif tant que l'app est au premier plan. Les alertes « app fermée »
            // viendront d'un daemon + push (cf. mobile/README.md), pas d'ici.
            tauri::async_runtime::spawn(async move {
                supervisor.start();
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::validate_and_store_key,
            commands::has_stored_key,
            commands::get_user,
            commands::logout,
            commands::list_servers,
            commands::server_details,
            commands::live_light,
            commands::metrics_history,
            commands::console_logs,
            commands::power_action,
            commands::send_command,
            commands::player_action,
            commands::console_connect,
            commands::console_disconnect,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application Tauri (mobile)");
}

/// `CoreEvent` → event Tauri du webview (+ notification locale pour les alertes).
/// Le `match` est exhaustif : tous les variants de `CoreEvent` sont traités.
fn forward(handle: &tauri::AppHandle, ev: CoreEvent) {
    match ev {
        CoreEvent::ConsoleOutput(p) => {
            let _ = handle.emit("console://output", p);
        }
        CoreEvent::ConsoleStatus(p) => {
            let _ = handle.emit("console://status", p);
        }
        CoreEvent::ConsoleConnection(p) => {
            let _ = handle.emit("console://connection", p);
        }
        CoreEvent::ConsoleStats(p) => {
            let _ = handle.emit("console://stats", p);
        }
        // Interne (déclencheur Copilote) : non relayé au webview.
        CoreEvent::ConsoleLog(_) => {}
        CoreEvent::SftpProgress(p) => {
            let _ = handle.emit("sftp://progress", p);
        }
        CoreEvent::ModInstallProgress(p) => {
            let _ = handle.emit("mods://install-progress", p);
        }
        CoreEvent::Alert(a) => {
            let title = if a.severity == "critical" {
                format!("⚠ {}", a.server_name)
            } else {
                a.server_name.clone()
            };
            let _ = handle
                .notification()
                .builder()
                .title(title)
                .body(a.message.clone())
                .show();
            let _ = handle.emit("alert://new", a);
        }
        CoreEvent::CopilotStarted(s) => {
            let _ = handle.emit("copilot://started", s);
        }
        CoreEvent::CopilotProgress(p) => {
            let _ = handle.emit("copilot://progress", p);
        }
        CoreEvent::ChatDelta(d) => {
            let _ = handle.emit("chat://delta", d);
        }
        CoreEvent::Diagnosis(d) => {
            let _ = handle
                .notification()
                .builder()
                .title(format!("🩺 {}", d.server_name))
                .body(d.summary.clone())
                .show();
            let _ = handle.emit("copilot://diagnosis", d);
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("minestrator_core=info,minestrator_terminal_mobile_lib=info,warn")
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
