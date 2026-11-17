//! Couche IPC mobile : sous-ensemble de départ des commandes exposées au webview.
//! Chacune délègue au `Core` (identique au desktop). On étoffera au fil des vues.
//! (Les arguments camelCase du frontend sont mappés en snake_case par Tauri.)

use minestrator_core::{
    ActivityPage, ArchiveEntry, Backup, ConsoleStats, Core, Egg, Error, LiveLight, LivePlayers, MetricSample, ServerDetails,
    ServersOverview, SftpEntry, Snapshot, UserNotification, UserProfile,
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
pub async fn live_players_bulk(
    core: State<'_, Arc<Core>>,
    ids: Vec<i64>,
) -> Result<Vec<LivePlayers>, Error> {
    core.live_players_bulk(&ids).await
}

#[tauri::command]
pub async fn list_eggs(core: State<'_, Arc<Core>>) -> Result<Vec<Egg>, Error> {
    core.list_eggs().await
}

#[tauri::command]
pub async fn server_activity(
    core: State<'_, Arc<Core>>,
    id: i64,
    limit: i64,
    offset: i64,
) -> Result<ActivityPage, Error> {
    core.server_activity(id, limit, offset).await
}

#[tauri::command]
pub async fn user_notifications(
    core: State<'_, Arc<Core>>,
) -> Result<Vec<UserNotification>, Error> {
    core.user_notifications().await
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

/// Échantillon live de stats (CPU/RAM/disque) via une connexion monitor éphémère.
#[tauri::command]
pub async fn sample_stats(
    core: State<'_, Arc<Core>>,
    server_id: i64,
) -> Result<Option<ConsoleStats>, Error> {
    core.sample_stats(server_id).await
}

// --- SFTP (gestionnaire de fichiers tactile) ------------------------------

#[tauri::command]
pub async fn sftp_list(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<Vec<SftpEntry>, Error> {
    core.sftp_list(server_id, &path).await
}

#[tauri::command]
pub async fn sftp_read_text(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<String, Error> {
    core.sftp_read_text(server_id, &path).await
}

#[tauri::command]
pub async fn sftp_write_text(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
    content: String,
) -> Result<(), Error> {
    core.sftp_write_text(server_id, &path, &content).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<(), Error> {
    core.sftp_mkdir(server_id, &path).await
}

/// Résultat de recherche : le tuple `(entrées, tronqué)` du core se sérialiserait en tableau
/// anonyme côté JS, illisible. La couche d'adaptation lui donne des noms.
#[derive(serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(rename = "SftpSearchResult"))]
pub struct SftpSearchOut {
    entries: Vec<SftpEntry>,
    /// La recherche s'est arrêtée avant d'avoir tout parcouru (garde-fou du core).
    truncated: bool,
}

#[tauri::command]
pub async fn sftp_search(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    root: String,
    query: String,
) -> Result<SftpSearchOut, Error> {
    let (entries, truncated) = core.sftp_search(server_id, &root, &query).await?;
    Ok(SftpSearchOut { entries, truncated })
}

/// Image lue via SFTP et rendue en data URI, directement affichable dans un `<img>`.
#[tauri::command]
pub async fn sftp_read_data_uri(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<String, Error> {
    core.sftp_read_data_uri(server_id, &path).await
}

/// Contenu d'une archive (`.zip`, `.tar`, `.tar.gz`) — lecture seule, sans extraction.
#[tauri::command]
pub async fn sftp_archive_list(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<Vec<ArchiveEntry>, Error> {
    core.sftp_archive_list(server_id, &path).await
}

#[tauri::command]
pub async fn sftp_archive_read_text(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
    entry: String,
) -> Result<String, Error> {
    core.sftp_archive_read_text(server_id, &path, &entry).await
}

/// Demande l'arrêt d'un transfert en cours (envoi natif). Sans effet s'il est déjà fini.
#[tauri::command]
pub fn cancel_transfer(core: State<'_, Arc<Core>>, transfer_id: String) {
    core.cancel_transfer(&transfer_id);
}

/// Taille d'un fichier distant : donne le total avant de télécharger par blocs.
#[tauri::command]
pub async fn sftp_file_size(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<u64, Error> {
    core.sftp_file_size(server_id, &path).await
}

/// Lit une tranche d'un fichier distant, en base64.
#[tauri::command]
pub async fn sftp_download_chunk(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
    offset: u64,
    len: usize,
) -> Result<String, Error> {
    use base64::Engine as _;
    let data = core.sftp_read_range(server_id, &path, offset, len).await?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&data))
}

#[tauri::command]
pub async fn sftp_delete(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
    is_dir: bool,
) -> Result<(), Error> {
    core.sftp_delete(server_id, &path, is_dir).await
}

#[tauri::command]
pub async fn sftp_rename(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    from: String,
    to: String,
) -> Result<(), Error> {
    core.sftp_rename(server_id, &from, &to).await
}

/// Lecture d'un fichier `.gz` (log) décompressé en texte — lecture seule côté UI.
#[tauri::command]
pub async fn sftp_gz_text(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<String, Error> {
    core.sftp_gz_text(server_id, &path).await
}

// --- Sauvegardes : backups quotidiens & snapshots à la demande -------------

#[tauri::command]
pub async fn list_backups(core: State<'_, Arc<Core>>, id: i64) -> Result<Vec<Backup>, Error> {
    core.list_backups(id).await
}

#[tauri::command]
pub async fn restore_backup(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    backup_id: i64,
) -> Result<(), Error> {
    core.restore_backup(server_id, backup_id).await
}

#[tauri::command]
pub async fn list_snapshots(core: State<'_, Arc<Core>>) -> Result<Vec<Snapshot>, Error> {
    core.list_snapshots().await
}

#[tauri::command]
pub async fn create_snapshot(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    name: String,
) -> Result<i64, Error> {
    core.create_snapshot(server_id, &name).await
}

#[tauri::command]
pub async fn restore_snapshot(
    core: State<'_, Arc<Core>>,
    snapshot_id: i64,
    server_id: i64,
) -> Result<i64, Error> {
    core.restore_snapshot(snapshot_id, server_id).await
}

/// Journaux récents **anonymisés**, à joindre à un rapport de bug (le front les copie).
#[tauri::command]
pub fn export_logs() -> String {
    minestrator_core::logs::export_recent(minestrator_core::logs::EXPORT_MAX_BYTES)
}

#[tauri::command]
pub async fn delete_snapshot(core: State<'_, Arc<Core>>, snapshot_id: i64) -> Result<i64, Error> {
    core.delete_snapshot(snapshot_id).await
}
