//! Couche IPC mobile : sous-ensemble de départ des commandes exposées au webview.
//! Chacune délègue au `Core` (identique au desktop). On étoffera au fil des vues.
//! (Les arguments camelCase du frontend sont mappés en snake_case par Tauri.)

use minestrator_core::{
    Core, Error, LiveLight, MetricSample, ServerDetails, ServersOverview, UserProfile,
};
use std::sync::Arc;
use tauri::State;

// --- Authentification -----------------------------------------------------

#[tauri::command]
pub async fn validate_and_store_key(
    core: State<'_, Arc<Core>>,
    key: String,
) -> Result<UserProfile, Error> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(Error::Unexpected("clé vide".into()));
    }
    core.authenticate_and_store(&key).await
}

#[tauri::command]
pub fn has_stored_key(core: State<'_, Arc<Core>>) -> Result<bool, Error> {
    core.has_key()
}

#[tauri::command]
pub async fn get_user(core: State<'_, Arc<Core>>) -> Result<UserProfile, Error> {
    core.get_user().await
}

#[tauri::command]
pub fn logout(core: State<'_, Arc<Core>>) -> Result<(), Error> {
    core.logout()
}

// --- Serveurs -------------------------------------------------------------

#[tauri::command]
pub async fn list_servers(core: State<'_, Arc<Core>>) -> Result<ServersOverview, Error> {
    core.list_servers().await
}

#[tauri::command]
pub async fn server_details(core: State<'_, Arc<Core>>, id: i64) -> Result<ServerDetails, Error> {
    core.server_details(id).await
}

#[tauri::command]
pub async fn live_light(core: State<'_, Arc<Core>>, id: i64) -> Result<LiveLight, Error> {
    core.live_light(id).await
}

#[tauri::command]
pub fn metrics_history(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    since_secs: i64,
) -> Result<Vec<MetricSample>, Error> {
    core.metrics(server_id, since_secs)
}

// --- Console / power / joueurs -------------------------------------------

#[tauri::command]
pub async fn console_logs(core: State<'_, Arc<Core>>, id: i64) -> Result<Vec<String>, Error> {
    core.console_logs(id).await
}

#[tauri::command]
pub async fn power_action(core: State<'_, Arc<Core>>, id: i64, action: String) -> Result<(), Error> {
    core.power_action(id, &action).await
}

#[tauri::command]
pub async fn send_command(
    core: State<'_, Arc<Core>>,
    id: i64,
    command: String,
) -> Result<(), Error> {
    core.send_command(id, &command).await
}

#[tauri::command]
pub async fn player_action(
    core: State<'_, Arc<Core>>,
    id: i64,
    action: String,
    player: String,
) -> Result<(), Error> {
    core.player_action(id, &action, &player).await
}

// --- Console WebSocket ----------------------------------------------------

// async : garantit l'exécution dans le runtime tokio (le core fait `tokio::spawn`).
#[tauri::command]
pub async fn console_connect(
    core: State<'_, Arc<Core>>,
    conn_id: String,
    server_id: i64,
) -> Result<(), Error> {
    core.console_connect(conn_id, server_id)
}

#[tauri::command]
pub fn console_disconnect(core: State<'_, Arc<Core>>, conn_id: String) {
    core.console_disconnect(&conn_id);
}
