//! Frontend desktop (Tauri) : couche d'adaptation au-dessus de `minestrator-core`.
//!
//! - expose le `Core` au webview via des commandes ;
//! - relaie les `CoreEvent` (broadcast) vers le webview et, pour les alertes, en
//!   **notifications natives** ;
//! - démarre le superviseur et les automatisations (redémarrage automatique, tâches planifiées,
//!   webhooks) ;
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

    // Runtime de Tauri à pile élargie : les assistants de la vue Réseau et « Appliquer » du
    // Copilote passent par les outils composites (préréglages → installations…), dont les futurs
    // imbriqués débordaient la pile par défaut de tokio en debug (voir
    // `minestrator_core::mcp::THREAD_STACK_BYTES`). Il vit jusqu'à la fin de `run`, donc de l'app.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(minestrator_core::mcp::THREAD_STACK_BYTES)
        .build()
        .expect("runtime tokio");
    tauri::async_runtime::set(runtime.handle().clone());

    let core = Arc::new(Core::new());
    let events = core.subscribe();
    let supervisor = core.supervisor();
    let copilot_core = core.clone();
    let link_core = core.clone();
    // Journaux et historique : journal local des incidents (outil MCP `list_incidents`).
    let journal_core = core.clone();

    let mut builder = tauri::Builder::default();
    // Instance unique : DOIT être le 1er plugin enregistré. Si une 2ᵉ instance est lancée alors que
    // l'app tourne déjà (potentiellement masquée dans le tray), on ré-affiche et focus la fenêtre
    // existante au lieu d'ouvrir un second processus.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }));
    }
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .manage(core)
        .setup(move |app| {
            // Auto-update (desktop) : vérifie les releases GitHub signées. Le check est déclenché
            // côté front au démarrage (voir src/lib/updater.ts) ; la vérification de signature se fait
            // avec la `pubkey` de tauri.conf.json.
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

            // Superviseur (dans le runtime tokio), après une première décision du lien avec le
            // service d'arrière-plan : si le service supervise déjà ce compte, le superviseur
            // local reste en veille (pas de double collecte ni d'alertes en double) et les
            // alertes du service arrivent par le même bus d'events (docs/SERVICE.md).
            // Automatisations (redémarrage automatique, tâches planifiées, webhooks) : exécutées
            // ici seulement tant que l'app est le superviseur actif (sinon par le service).
            tauri::async_runtime::spawn(async move {
                link_core.clone().start_service_link().await;
                supervisor.start();
                link_core.start_automations();
            });

            // Copilote : écoute des alertes → diagnostic LLM automatique.
            tauri::async_runtime::spawn(async move {
                copilot_core.spawn_copilot();
            });

            // Journaux et historique : alertes et diagnostics consignés dans la base locale, relus
            // par l'outil MCP `list_incidents` (y compris par le MCP headless, même dossier).
            tauri::async_runtime::spawn(async move {
                journal_core.start_incident_journal();
            });

            setup_tray(app)?;

            // Croix de la fenêtre : on empêche la fermeture et on délègue le choix au front (réduire
            // dans le tray / quitter, avec « se souvenir ») — voir src/lib/components/CloseDialog.svelte.
            if let Some(window) = app.get_webview_window("main") {
                let w = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.emit("close-requested", ());
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
            commands::accounts_list,
            commands::account_add,
            commands::account_switch,
            commands::account_remove,
            commands::list_servers,
            commands::server_details,
            commands::live_light,
            commands::server_live,
            commands::server_properties,
            commands::set_server_property,
            commands::mybox_details,
            commands::mybox_capacities,
            commands::mybox_capacity,
            commands::server_catalog,
            commands::mybox_create_server,
            commands::mybox_resize_server,
            commands::mybox_set_server_paused,
            commands::mybox_reorder_servers,
            commands::rename_server,
            commands::rename_mybox,
            commands::mybox_snapshot_before_delete,
            commands::backup_job,
            commands::mybox_delete_server,
            commands::mybox_plan_placement,
            commands::live_players_bulk,
            commands::list_eggs,
            commands::server_activity,
            commands::user_notifications,
            commands::metrics_history,
            commands::get_supervisor_config,
            commands::set_supervisor_config,
            commands::get_mcp_config,
            commands::set_mcp_config,
            commands::agent_actions,
            commands::agent_changesets,
            commands::rollback_changeset,
            commands::agent_secrets,
            commands::add_agent_secret,
            commands::generate_agent_secret,
            commands::delete_agent_secret,
            commands::network_topology,
            commands::network_ping,
            commands::network_databases,
            commands::network_link,
            commands::network_preset,
            commands::get_privacy_config,
            commands::set_privacy_config,
            commands::app_exe_path,
            commands::detect_clis,
            commands::console_logs,
            commands::power_action,
            commands::send_command,
            commands::player_action,
            commands::known_players,
            commands::player_profile,
            commands::player_inventory,
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
            commands::cancel_transfer,
            commands::server_compress,
            commands::server_extract,
            commands::disk_usage,
            commands::disk_space,
            commands::sftp_archive_list,
            commands::sftp_archive_read_text,
            commands::sftp_gz_text,
            commands::sftp_read_data_uri,
            commands::sftp_nbt_tree,
            commands::sftp_inspect_region,
            commands::sftp_region_chunks,
            commands::sftp_region_chunk_tree,
            commands::sftp_nbt_snbt,
            commands::sftp_region_chunk_snbt,
            commands::paste_upload,
            commands::sftp_extract_entry,
            commands::sftp_disconnect,
            commands::sftp_set_password,
            commands::sftp_forget_password,
            commands::sftp_has_password,
            commands::sftp_host_key,
            commands::panel_schedules,
            commands::panel_schedule_create,
            commands::panel_schedule_update,
            commands::panel_schedule_delete,
            commands::panel_task_add,
            commands::panel_task_update,
            commands::panel_task_delete,
            commands::panel_tasks_reorder,
            commands::sftp_trust_host_key,
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
            commands::installed_datapacks,
            commands::content_updates,
            commands::apply_content_updates,
            commands::ficsit_search,
            commands::ficsit_mod_versions,
            commands::ficsit_sml_versions,
            commands::ficsit_installed,
            commands::ficsit_set_enabled,
            commands::ficsit_remove,
            commands::ficsit_install,
            commands::mods_search,
            commands::mods_versions,
            commands::mods_install,
            commands::mods_installed,
            commands::mods_set_enabled,
            commands::mods_remove,
            commands::get_game_settings,
            commands::set_game_settings,
            commands::set_factorio_token,
            commands::has_factorio_token,
            commands::clear_factorio_token,
            commands::hide_to_tray,
            commands::quit_app,
            commands::export_logs,
            commands::open_logs_dir,
            commands::service_overview,
            commands::service_enable,
            commands::service_disable,
            commands::service_uninstall,
            commands::service_update,
            commands::service_take_pending_setup,
            commands::service_restart,
            commands::service_alerts,
            commands::server_automations,
            commands::set_auto_restart,
            commands::save_scheduled_task,
            commands::delete_scheduled_task,
            commands::automation_runs,
            commands::uptime_report,
            commands::list_webhooks,
            commands::save_webhook,
            commands::delete_webhook,
            commands::test_webhook,
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
        CoreEvent::ModInstallProgress(p) => {
            let _ = handle.emit("mods://install-progress", p);
        }
        // Toutes les fenêtres (détachées comprises) se rechargent sur le nouveau compte.
        CoreEvent::AccountSwitched(s) => {
            let _ = handle.emit("account://switched", s);
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

/// Journaux : sortie standard + fichier tournant dans le dossier de données (cf. `core::logs`).
fn init_tracing() {
    minestrator_core::logs::init("minestrator_core=info,minestrator_terminal_lib=info,warn");
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

    // Pile élargie : les outils composites imbriquent des futurs volumineux (voir
    // `minestrator_core::mcp::THREAD_STACK_BYTES`).
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(minestrator_core::mcp::THREAD_STACK_BYTES)
        .build()
        .expect("runtime tokio");
    runtime.block_on(async {
        let core = Core::new();
        tracing::info!("mode MCP (stdio) prêt");
        minestrator_core::mcp::serve_stdio(std::sync::Arc::new(core)).await;
    });
}
