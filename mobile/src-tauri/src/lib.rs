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

mod alerts;
mod apk_installer;
mod background;
#[cfg(all(test, feature = "ts"))]
mod bindings;
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
    // Journaux : initialisés dans `setup`, une fois le dossier privé de l'app connu (le fichier
    // y est écrit). Ce qui précède `setup` n'est que l'enregistrement des plugins.
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
            let data_dir = app.path().app_data_dir().ok();
            if let Some(dir) = &data_dir {
                let _ = std::fs::create_dir_all(dir);
                std::env::set_var("MINESTRATOR_DATA_DIR", dir);
            }
            init_tracing();
            // Journal des alertes (centre d'alertes) : alimenté par le pont d'events ci-dessous,
            // il survit à la destruction du webview quand seul le service garde le process.
            app.manage(alerts::AlertJournal::open(
                data_dir.map(|d| d.join("alerts.json")),
            ));

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

            // Noms des serveurs pour la notification : une lecture au démarrage suffit, ils ne
            // changent pas en cours de session — sauf changement de compte, qui la refait (et
            // l'échec est sans gravité : repli sur l'id).
            load_server_names(app.handle().clone());

            // Superviseur : actif tant que l'app est au premier plan. Les alertes « app fermée »
            // viendront d'un daemon + push (cf. docs/PUSH.md), pas d'ici.
            tauri::async_runtime::spawn(async move {
                supervisor.start();
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::export_logs,
            commands::validate_and_store_key,
            commands::has_stored_key,
            commands::get_user,
            commands::logout,
            commands::accounts_list,
            commands::account_add,
            commands::account_switch,
            commands::account_remove,
            commands::list_servers,
            commands::server_details,
            commands::live_light,
            commands::live_players_bulk,
            commands::list_eggs,
            commands::server_activity,
            commands::user_notifications,
            commands::metrics_history,
            commands::server_properties,
            commands::set_server_property,
            commands::market_minecraft_versions,
            commands::market_list,
            commands::market_versions,
            commands::install_mod,
            commands::installed_mods,
            commands::installed_plugins,
            alerts::alerts_list,
            alerts::alerts_mark_read,
            alerts::alerts_clear,
            commands::console_logs,
            commands::power_action,
            commands::send_command,
            commands::player_action,
            commands::console_connect,
            commands::console_disconnect,
            commands::sample_stats,
            commands::sftp_list,
            commands::sftp_set_password,
            commands::sftp_forget_password,
            commands::sftp_has_password,
            commands::sftp_host_key,
            commands::panel_schedules,
            commands::panel_schedule_update,
            commands::sftp_trust_host_key,
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

/// Charge (en tâche de fond) les noms des serveurs du compte actif pour la notification persistante.
fn load_server_names(handle: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        let core = handle.state::<Arc<Core>>();
        if let Ok(overview) = core.list_servers().await {
            if let Some(sum) = handle.try_state::<std::sync::Mutex<MonitorSummary>>() {
                if let Ok(mut sum) = sum.lock() {
                    for srv in overview.servers {
                        sum.names.insert(srv.id, srv.name);
                    }
                }
            }
        }
    });
}

/// Changement de compte : les serveurs suivis par la notification persistante sont ceux de
/// l'ancien compte. On repart de zéro ; les connexions monitor du nouveau compte la rempliront.
fn reset_monitor_summary(handle: &tauri::AppHandle) {
    if let Some(sum) = handle.try_state::<std::sync::Mutex<MonitorSummary>>() {
        if let Ok(mut sum) = sum.lock() {
            sum.states.clear();
            sum.names.clear();
        }
    }
    load_server_names(handle.clone());
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
        CoreEvent::AccountSwitched(s) => {
            reset_monitor_summary(handle);
            let _ = handle.emit("account://switched", s);
        }
        CoreEvent::Alert(a) => {
            // Consignée AVANT l'event : le webview relit le journal en le recevant.
            if let Some(journal) = handle.try_state::<alerts::AlertJournal>() {
                journal.push(a.clone());
            }
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

/// Journaux : fichier tournant dans le dossier privé de l'app (cf. `core::logs`) — la sortie
/// standard native n'est pas reliée à logcat, sans fichier tout serait perdu.
fn init_tracing() {
    minestrator_core::logs::init("minestrator_core=info,minestrator_terminal_mobile_lib=info,warn");
}
