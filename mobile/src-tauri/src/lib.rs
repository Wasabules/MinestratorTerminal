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
mod background;
mod commands;
mod files;
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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(apk_installer::init())
        .plugin(background::init())
        .plugin(files::init())
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
            // État agrégé des serveurs surveillés, lu par `push_monitor_state`.
            app.manage(std::sync::Mutex::new(MonitorSummary::default()));

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

            // Noms des serveurs pour la notification : une seule lecture au démarrage suffit,
            // ils ne changent pas en cours de session (et l'échec est sans gravité : repli sur l'id).
            let names_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let core = names_handle.state::<Arc<Core>>();
                if let Ok(overview) = core.list_servers().await {
                    if let Some(sum) = names_handle.try_state::<std::sync::Mutex<MonitorSummary>>() {
                        if let Ok(mut sum) = sum.lock() {
                            for srv in overview.servers {
                                sum.names.insert(srv.id, srv.name);
                            }
                        }
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
            commands::live_players_bulk,
            commands::list_eggs,
            commands::server_activity,
            commands::user_notifications,
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
            commands::sftp_search,
            commands::sftp_read_data_uri,
            commands::sftp_archive_list,
            commands::sftp_archive_read_text,
            commands::sftp_file_size,
            commands::cancel_transfer,
            commands::sftp_download_chunk,
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
            background::set_background_monitoring,
            background::is_battery_unrestricted,
            background::request_battery_unrestricted,
            background::open_app_settings,
            files::file_display_name,
            files::notify_transfer,
            files::clear_transfer,
            files::sftp_upload_uri,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application Tauri (mobile)");
}

/// Résumé d'état des serveurs surveillés, poussé dans la notification persistante.
///
/// Le superviseur ouvre une connexion « monitor » par serveur, dont les events portent un
/// `conn_id` de la forme `monitor-<id>` : c'est le seul moyen de rattacher un `stats`/`status`
/// à un serveur depuis cette couche. Les connexions ouvertes par l'UI sont ignorées, sans quoi
/// consulter une console ferait double compte.
#[derive(Default)]
struct MonitorSummary {
    states: std::collections::HashMap<i64, String>,
    names: std::collections::HashMap<i64, String>,
}

impl MonitorSummary {
    /// `mon:377829` → `Some(377829)`. `None` pour une connexion ouverte par l'UI.
    /// Le préfixe vient de `supervisor::monitor_conn_id` : le garder synchronisé avec le core.
    fn server_of(conn_id: &str) -> Option<i64> {
        conn_id.strip_prefix("mon:")?.parse().ok()
    }

    /// Ligne de titre + corps de la notification. `None` tant qu'aucun serveur n'est connu :
    /// mieux vaut garder le texte par défaut qu'afficher « 0 serveur ».
    fn render(&self) -> Option<(String, String)> {
        if self.states.is_empty() {
            return None;
        }
        let online = self.states.values().filter(|s| *s == "running").count();
        let title = format!("{online}/{} serveur(s) en ligne", self.states.len());
        let mut lines: Vec<String> = self
            .states
            .iter()
            .map(|(id, state)| {
                let name = self.names.get(id).cloned().unwrap_or_else(|| format!("#{id}"));
                let mark = if state == "running" { "●" } else { "○" };
                format!("{mark} {name}")
            })
            .collect();
        lines.sort();
        Some((title, lines.join("
")))
    }
}

/// Enregistre l'état d'un serveur surveillé et rafraîchit la notification persistante.
///
/// N'écrit dans la notification **que si l'état a changé** : les connexions monitor émettent des
/// `stats` environ une fois par seconde et par serveur, ce qui ferait autrement un appel JNI par
/// seconde pour un texte identique.
fn push_monitor_state(handle: &tauri::AppHandle, server_id: i64, state: &str) {
    let Some(summary) = handle.try_state::<std::sync::Mutex<MonitorSummary>>() else {
        return;
    };
    let rendered = {
        let Ok(mut sum) = summary.lock() else { return };
        let previous = sum.states.insert(server_id, state.to_string());
        if previous.as_deref() == Some(state) {
            return;
        }
        sum.render()
    };
    if let Some((title, text)) = rendered {
        let _ = handle
            .state::<background::BackgroundMonitor<tauri::Wry>>()
            .update_status(&title, &text);
    }
}

/// `CoreEvent` → event Tauri du webview (+ notification locale pour les alertes).
/// Le `match` est exhaustif : tous les variants de `CoreEvent` sont traités.
fn forward(handle: &tauri::AppHandle, ev: CoreEvent) {
    match ev {
        CoreEvent::ConsoleOutput(p) => {
            let _ = handle.emit("console://output", p);
        }
        CoreEvent::ConsoleStatus(p) => {
            if let Some(id) = MonitorSummary::server_of(&p.conn_id) {
                push_monitor_state(handle, id, &p.state);
            }
            let _ = handle.emit("console://status", p);
        }
        CoreEvent::ConsoleConnection(p) => {
            let _ = handle.emit("console://connection", p);
        }
        CoreEvent::ConsoleStats(p) => {
            if let Some(id) = MonitorSummary::server_of(&p.conn_id) {
                push_monitor_state(handle, id, &p.state);
            }
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
