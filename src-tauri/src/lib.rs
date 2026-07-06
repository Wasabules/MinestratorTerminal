//! Frontend desktop (Tauri) : couche d'adaptation au-dessus de `minestrator-core`.
//!
//! - expose le `Core` au webview via des commandes ;
//! - relaie les `CoreEvent` (broadcast) vers le webview et, pour les alertes, en
//!   **notifications natives** ;
//! - démarre le superviseur ;
//! - vit en **tray** (fermer la fenêtre la masque, le superviseur continue).

mod commands;

use minestrator_core::{Core, CoreEvent};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::broadcast::error::RecvError;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_tracing();

    let core = Arc::new(Core::new());
    let events = core.subscribe();
    let supervisor = core.supervisor();
    let copilot_core = core.clone();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .manage(core)
        .setup(move |app| {
            // Auto-update : plugin PRÉPARÉ (desktop). Inerte tant qu'aucun `check()` n'est appelé et
            // que `plugins.updater` (endpoints + pubkey) n'est pas renseigné — voir docs/AUTO-UPDATE.md.
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            let handle = app.handle().clone();

            // Pont : CoreEvent (broadcast) → events Tauri + notifications.
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

            // Superviseur (dans le runtime tokio).
            tauri::async_runtime::spawn(async move {
                supervisor.start();
            });

            // Copilote : écoute des alertes → diagnostic LLM automatique.
            tauri::async_runtime::spawn(async move {
                copilot_core.spawn_copilot();
            });

            setup_tray(app)?;

            // Fermer la fenêtre la masque (le superviseur reste actif en fond).
            if let Some(window) = app.get_webview_window("main") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

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
            commands::get_supervisor_config,
            commands::set_supervisor_config,
            commands::get_mcp_config,
            commands::set_mcp_config,
            commands::get_privacy_config,
            commands::set_privacy_config,
            commands::app_exe_path,
            commands::detect_clis,
            commands::console_logs,
            commands::power_action,
            commands::send_command,
            commands::player_action,
            commands::console_connect,
            commands::console_disconnect,
            commands::sftp_list,
            commands::sftp_read_text,
            commands::sftp_write_text,
            commands::sftp_mkdir,
            commands::sftp_delete,
            commands::sftp_rename,
            commands::sftp_upload,
            commands::sftp_download,
            commands::sftp_download_zip,
            commands::sftp_archive_list,
            commands::sftp_archive_read_text,
            commands::sftp_gz_text,
            commands::sftp_read_data_uri,
            commands::sftp_nbt_tree,
            commands::sftp_inspect_region,
            commands::sftp_region_chunks,
            commands::sftp_region_chunk_tree,
            commands::sftp_extract_entry,
            commands::sftp_disconnect,
            commands::get_copilot_config,
            commands::set_copilot_config,
            commands::has_copilot_key,
            commands::set_copilot_key,
            commands::clear_copilot_key,
            commands::copilot_apply,
            commands::copilot_diagnose_now,
            commands::copilot_analyze,
            commands::copilot_performance,
            commands::chat_send,
            commands::chat_reset,
            commands::chat_warm,
            commands::list_backups,
            commands::list_snapshots,
            commands::create_snapshot,
            commands::restore_snapshot,
            commands::restore_backup,
            commands::delete_snapshot,
            commands::market_minecraft_versions,
            commands::market_list,
            commands::market_versions,
            commands::install_mod,
            commands::installed_mods,
            commands::installed_plugins,
        ])
        .run(tauri::generate_context!())
        .expect("erreur au lancement de l'application Tauri");
}

/// `CoreEvent` → event Tauri du webview (+ notification native pour les alertes).
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

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItem::with_id(app, "show", "Afficher", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("main")
        .tooltip("Minestrator Terminal")
        .icon(app.default_window_icon().cloned().expect("icône par défaut"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn show_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("minestrator_core=info,minestrator_terminal_lib=info,warn"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Mode serveur MCP (stdio) : la même app, lancée avec `--mcp`, sert le protocole MCP
/// au lieu d'ouvrir la GUI. Toute la logique vit dans `minestrator_core::mcp`.
/// **stdout = protocole** ; les logs vont sur stderr.
pub fn run_mcp() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let runtime = tokio::runtime::Runtime::new().expect("runtime tokio");
    runtime.block_on(async {
        let core = Core::new();
        tracing::info!("mode MCP (stdio) prêt");
        minestrator_core::mcp::serve_stdio(&core).await;
    });
}
