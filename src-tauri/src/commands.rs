//! Couche IPC : commandes exposées au webview. Chacune délègue au `Core`.
//! (Les arguments camelCase du frontend sont mappés en snake_case par Tauri.)

use minestrator_core::{
    ArchiveEntry, Backup, ChatReply, CliStatus, CopilotConfig, Core, Error, InstalledItem,
    LiveLight, MarketPage, MarketVersion, McpConfig, MetricSample, PrivacyConfig, ServerDetails,
    ServersOverview, SftpEntry, Snapshot, SupervisorConfig, UserProfile,
};
use serde_json::Value;
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

#[tauri::command]
pub fn get_supervisor_config(core: State<'_, Arc<Core>>) -> SupervisorConfig {
    core.get_supervisor_config()
}

#[tauri::command]
pub fn set_supervisor_config(core: State<'_, Arc<Core>>, config: SupervisorConfig) {
    core.set_supervisor_config(config);
}

#[tauri::command]
pub fn get_mcp_config(core: State<'_, Arc<Core>>) -> McpConfig {
    core.get_mcp_config()
}

#[tauri::command]
pub fn set_mcp_config(core: State<'_, Arc<Core>>, config: McpConfig) {
    core.set_mcp_config(config);
}

#[tauri::command]
pub fn get_privacy_config(core: State<'_, Arc<Core>>) -> PrivacyConfig {
    core.get_privacy_config()
}

#[tauri::command]
pub fn set_privacy_config(core: State<'_, Arc<Core>>, config: PrivacyConfig) {
    core.set_privacy_config(config);
}

/// Chemin absolu de l'exécutable de l'app — pour composer la config MCP de Claude
/// (`<exe> --mcp`).
#[tauri::command]
pub fn app_exe_path() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Détecte les agents CLI installés (Claude Code / OpenCode / Gemini) pour l'UI Copilote.
#[tauri::command]
pub async fn detect_clis() -> Vec<CliStatus> {
    minestrator_core::detect_clis().await
}

#[tauri::command]
pub async fn console_logs(core: State<'_, Arc<Core>>, id: i64) -> Result<Vec<String>, Error> {
    core.console_logs(id).await
}

#[tauri::command]
pub async fn power_action(core: State<'_, Arc<Core>>, id: i64, action: String) -> Result<(), Error> {
    core.power_action(id, &action).await
}

#[tauri::command]
pub async fn send_command(core: State<'_, Arc<Core>>, id: i64, command: String) -> Result<(), Error> {
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

// --- SFTP -----------------------------------------------------------------

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
pub async fn sftp_mkdir(core: State<'_, Arc<Core>>, server_id: i64, path: String) -> Result<(), Error> {
    core.sftp_mkdir(server_id, &path).await
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

// Les transferts sont lancés en TÂCHE DE FOND : la commande rend la main aussitôt et la
// progression/complétion arrive via l'event `sftp://progress` (corrélé par `transfer_id`).
#[tauri::command]
pub fn sftp_upload(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    local_path: String,
    remote_dir: String,
    transfer_id: String,
) {
    let core = core.inner().clone();
    tauri::async_runtime::spawn(async move {
        core.run_sftp_upload(server_id, &local_path, &remote_dir, &transfer_id).await;
    });
}

#[tauri::command]
pub fn sftp_download(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    remote_path: String,
    local_path: String,
    transfer_id: String,
) {
    let core = core.inner().clone();
    tauri::async_runtime::spawn(async move {
        core.run_sftp_download(server_id, &remote_path, &local_path, &transfer_id).await;
    });
}

/// Télécharge une sélection (fichiers/dossiers) en UN `.zip` local, en tâche de fond.
#[tauri::command]
pub fn sftp_download_zip(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    paths: Vec<String>,
    local_zip: String,
    transfer_id: String,
) {
    let core = core.inner().clone();
    tauri::async_runtime::spawn(async move {
        core.run_sftp_download_zip(server_id, paths, &local_zip, &transfer_id).await;
    });
}

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

#[tauri::command]
pub async fn sftp_gz_text(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
) -> Result<String, Error> {
    core.sftp_gz_text(server_id, &path).await
}

#[tauri::command]
pub async fn sftp_extract_entry(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    path: String,
    entry: String,
    local_path: String,
) -> Result<(), Error> {
    core.sftp_extract_entry(server_id, &path, &entry, &local_path).await
}

#[tauri::command]
pub fn sftp_disconnect(core: State<'_, Arc<Core>>, server_id: i64) {
    core.sftp_disconnect(server_id);
}

// --- Copilote (agent LLM multi-fournisseur) -------------------------------

#[tauri::command]
pub fn get_copilot_config(core: State<'_, Arc<Core>>) -> CopilotConfig {
    core.get_copilot_config()
}

#[tauri::command]
pub fn set_copilot_config(core: State<'_, Arc<Core>>, config: CopilotConfig) {
    core.set_copilot_config(config);
}

/// Une clé LLM est-elle enregistrée pour le fournisseur sélectionné ?
#[tauri::command]
pub fn has_copilot_key(core: State<'_, Arc<Core>>) -> Result<bool, Error> {
    core.has_copilot_key()
}

/// Enregistre la clé LLM du fournisseur sélectionné (trousseau OS).
#[tauri::command]
pub fn set_copilot_key(core: State<'_, Arc<Core>>, key: String) -> Result<(), Error> {
    core.set_copilot_key(&key)
}

#[tauri::command]
pub fn clear_copilot_key(core: State<'_, Arc<Core>>) -> Result<(), Error> {
    core.clear_copilot_key()
}

/// Applique une action proposée par le Copilote (bouton « Appliquer »).
#[tauri::command]
pub async fn copilot_apply(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    tool: String,
    args: Value,
) -> Result<String, Error> {
    core.copilot_apply(server_id, &tool, args).await
}

/// Déclenche un diagnostic manuel (async : garantit le contexte runtime tokio).
#[tauri::command]
pub async fn copilot_diagnose_now(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    server_name: String,
) -> Result<(), Error> {
    core.inner().clone().diagnose_now(server_id, server_name);
    Ok(())
}

/// Analyse un extrait sélectionné (clic droit → Copilote).
#[tauri::command]
pub async fn copilot_analyze(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    server_name: String,
    text: String,
) -> Result<(), Error> {
    core.inner()
        .clone()
        .copilot_analyze(server_id, server_name, text);
    Ok(())
}

/// Analyse de performance Spark (bouton « Analyser les performances »).
#[tauri::command]
pub async fn copilot_performance(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    server_name: String,
) -> Result<(), Error> {
    core.inner().clone().analyze_performance(server_id, server_name);
    Ok(())
}

/// Assistant conversationnel : envoie un message et renvoie la réponse (+ actions proposées).
#[tauri::command]
pub async fn chat_send(
    core: State<'_, Arc<Core>>,
    session_id: String,
    server_id: i64,
    server_name: String,
    message: String,
    autonomous: bool,
) -> Result<ChatReply, Error> {
    Ok(core
        .chat_send(session_id, server_id, server_name, message, autonomous)
        .await)
}

/// Réinitialise une conversation assistant.
#[tauri::command]
pub fn chat_reset(core: State<'_, Arc<Core>>, session_id: String) {
    core.chat_reset(&session_id);
}

/// Pré-chauffe (F) : démarre en arrière-plan le process agent persistant d'une session, avant le
/// 1er message, pour masquer le bootstrap. Best-effort — n'échoue jamais côté UI.
#[tauri::command]
pub async fn chat_warm(
    core: State<'_, Arc<Core>>,
    session_id: String,
    autonomous: bool,
) -> Result<(), Error> {
    core.chat_warm(session_id, autonomous).await;
    Ok(())
}

// --- Filet de sécurité : backups & snapshots ------------------------------

#[tauri::command]
pub async fn list_backups(core: State<'_, Arc<Core>>, server_id: i64) -> Result<Vec<Backup>, Error> {
    core.list_backups(server_id).await
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

/// DESTRUCTIF : écrase le serveur avec l'état du snapshot.
#[tauri::command]
pub async fn restore_snapshot(
    core: State<'_, Arc<Core>>,
    snapshot_id: i64,
    server_id: i64,
) -> Result<i64, Error> {
    core.restore_snapshot(snapshot_id, server_id).await
}

/// DESTRUCTIF : écrase le serveur avec l'état du backup quotidien.
#[tauri::command]
pub async fn restore_backup(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    backup_id: i64,
) -> Result<(), Error> {
    core.restore_backup(server_id, backup_id).await
}

#[tauri::command]
pub async fn delete_snapshot(core: State<'_, Arc<Core>>, snapshot_id: i64) -> Result<i64, Error> {
    core.delete_snapshot(snapshot_id).await
}

// --- Marketplace (mods & plugins) -----------------------------------------

#[tauri::command]
pub async fn market_minecraft_versions(core: State<'_, Arc<Core>>) -> Result<Vec<String>, Error> {
    core.market_minecraft_versions().await
}

#[tauri::command]
pub async fn market_list(
    core: State<'_, Arc<Core>>,
    kind: String,
    source: String,
    page: i64,
    query: String,
    loader: String,
    game_version: String,
) -> Result<MarketPage, Error> {
    core.market_list(&kind, &source, page, &query, &loader, &game_version).await
}

#[tauri::command]
pub async fn market_versions(
    core: State<'_, Arc<Core>>,
    source: String,
    slug_or_id: String,
    loader: String,
    game_version: String,
) -> Result<Vec<MarketVersion>, Error> {
    core.market_versions(&source, &slug_or_id, &loader, &game_version).await
}

#[tauri::command]
pub async fn install_mod(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    source: String,
    kind: String,
    slug: String,
    version_id: String,
    loader: String,
) -> Result<(), Error> {
    core.install_mod(server_id, &source, &kind, &slug, &version_id, &loader).await
}

#[tauri::command]
pub async fn installed_mods(
    core: State<'_, Arc<Core>>,
    server_id: i64,
) -> Result<Vec<InstalledItem>, Error> {
    core.installed_mods(server_id).await
}

#[tauri::command]
pub async fn installed_plugins(
    core: State<'_, Arc<Core>>,
    server_id: i64,
) -> Result<Vec<InstalledItem>, Error> {
    core.installed_plugins(server_id).await
}
