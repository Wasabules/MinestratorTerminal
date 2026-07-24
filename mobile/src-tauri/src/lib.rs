//! Frontend mobile (Tauri) : couche d'adaptation au-dessus de `minestrator-core`.
//!
//! Version allégée du crate desktop : **pas de tray, d'updater, ni de fenêtres détachées**
//! (l'OS mobile ne les fournit pas). On garde l'essentiel : exposer le `Core` au webview
//! via des commandes, et relayer les `CoreEvent` (broadcast) → events Tauri + notifications.
//!
//! Spécificités mobile :
//! - **secrets** : sur Android, le `Core` stocke ses secrets dans un fichier du dossier privé
//!   de l'app (le `keyring` desktop n'a pas de backend Android). On pose donc
//!   `MINESTRATOR_DATA_DIR` = dossier de données de l'app **avant** de créer le `Core`.
//! - **alertes app fermée** : le superviseur de fond n'existe pas sur mobile → daemon + push
//!   FCM (cf. `crates/minestrator-daemon` et `docs/PUSH.md`).

mod apk_installer;
mod commands;
mod update;

use minestrator_core::{Core, CoreEvent};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::broadcast::error::RecvError;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(apk_installer::init())
        .setup(|app| {
            // 1. Dossier de données privé de l'app → env, AVANT de créer le Core.
            //    C'est là que le Core écrit ses secrets (Android) et son SQLite de métriques.
            if let Ok(dir) = app.path().app_data_dir() {
                let _ = std::fs::create_dir_all(&dir);
                std::env::set_var("MINESTRATOR_DATA_DIR", &dir);
            }

            // 2. Core + abonnement aux events + superviseur.
            let core = Arc::new(Core::new());
            let mut events = core.subscribe();
            let supervisor = core.supervisor();
            app.manage(core);

            // Pont : CoreEvent (broadcast) → events Tauri + notifications locales.
            let handle = app.handle().clone();
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
            // viendront d'un daemon + push (cf. docs/PUSH.md), pas d'ici.
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
            commands::sample_stats,
            commands::sftp_list,
            commands::sftp_read_text,
            commands::sftp_write_text,
            commands::sftp_mkdir,
            commands::sftp_delete,
            commands::sftp_rename,
            commands::sftp_gz_text,
            commands::list_backups,
            commands::restore_backup,
            commands::list_snapshots,
            commands::create_snapshot,
            commands::restore_snapshot,
            commands::delete_snapshot,
            update::check_update,
            update::download_update,
            apk_installer::install_apk,
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
