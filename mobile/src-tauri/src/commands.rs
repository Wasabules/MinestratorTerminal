//! Couche IPC mobile : sous-ensemble de départ des commandes exposées au webview.
//! Chacune délègue au `Core` (identique au desktop). On étoffera au fil des vues.
//! (Les arguments camelCase du frontend sont mappés en snake_case par Tauri.)

use minestrator_core::{
    PanelSchedule, PanelScheduleInput,
    PlayerInventory, PlayerProfile, PlayersPage, PlayersQuery,
    Account, ActivityPage, ArchiveEntry, Backup, ConsoleStats, Core, Egg, Error, InstalledItem,
    LiveLight, LivePlayers, MarketPage, MarketVersion, MetricsHistory, ServerDetails, ServerProperties,
    ServersOverview, SftpEntry, SftpHostKey, Snapshot, UserNotification, UserProfile,
    ContentUpdates, UpdateReport, UpdateSelection,
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
    core.sign_in(&key).await
}

#[tauri::command]
pub fn has_stored_key(core: State<'_, Arc<Core>>) -> Result<bool, Error> {
    core.has_key()
}

/// Profil du compte actif (et mise en cohérence de la liste des comptes).
#[tauri::command]
pub async fn get_user(core: State<'_, Arc<Core>>) -> Result<UserProfile, Error> {
    core.session_user().await
}

/// Retire le compte actif ; renvoie celui qui prend la relève (`null` : écran de connexion).
#[tauri::command]
pub async fn logout(core: State<'_, Arc<Core>>) -> Result<Option<Account>, Error> {
    core.sign_out().await
}

// --- Comptes (plusieurs comptes MineStrator) ---------------------------------

#[tauri::command]
pub fn accounts_list(core: State<'_, Arc<Core>>) -> Vec<Account> {
    core.accounts()
}

/// Ajoute un compte (clé validée par l'API, doublon refusé), rangé inactif.
#[tauri::command]
pub async fn account_add(core: State<'_, Arc<Core>>, key: String) -> Result<Account, Error> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(Error::Unexpected("clé vide".into()));
    }
    core.add_account(&key).await
}

#[tauri::command]
pub async fn account_switch(core: State<'_, Arc<Core>>, id: i64) -> Result<Account, Error> {
    core.switch_account(id).await
}

/// Retire un compte ; renvoie le compte actif après l'opération (`null` : plus aucun).
#[tauri::command]
pub async fn account_remove(
    core: State<'_, Arc<Core>>,
    id: i64,
) -> Result<Option<Account>, Error> {
    core.remove_account(id).await
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

/// Historique croisé : échantillons locaux (surveillance de l'app) complétés par les statistiques
/// de l'hébergeur là où l'app ne surveillait pas.
#[tauri::command]
pub async fn metrics_history(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    since_secs: i64,
    max_points: Option<u32>,
) -> Result<MetricsHistory, Error> {
    core.server_history(server_id, since_secs, max_points).await
}

// --- Paramètres du serveur (`server.properties`, via l'API : ni SFTP ni mot de passe) -------

/// Réglages du serveur. Réservé aux jeux dont `GameCapabilities.properties` est vrai (Minecraft).
#[tauri::command]
pub async fn server_properties(core: State<'_, Arc<Core>>, id: i64) -> Result<ServerProperties, Error> {
    core.server_properties(id).await
}

/// Modifie UN réglage existant ; pris en compte au prochain démarrage du serveur.
#[tauri::command]
pub async fn set_server_property(
    core: State<'_, Arc<Core>>,
    id: i64,
    property: String,
    value: String,
) -> Result<(), Error> {
    core.set_server_property(id, &property, &value).await
}

// --- Marketplace Minecraft (Modrinth / CurseForge / SpigotMC, via l'API MineStrator) --------
// Catalogue et installation passent par l'API de l'hébergeur : pas de SFTP, donc pas de mot de
// passe SFTP requis (contrairement aux marketplaces des autres jeux, non exposées sur mobile).

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

#[tauri::command]
pub async fn installed_datapacks(
    core: State<'_, Arc<Core>>,
    server_id: i64,
) -> Result<Vec<InstalledItem>, Error> {
    core.installed_datapacks(server_id).await
}

/// Mises à jour disponibles des mods, plugins et datapacks (gardées 15 min, `force` revérifie).
#[tauri::command]
pub async fn content_updates(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    force: bool,
) -> Result<ContentUpdates, Error> {
    core.content_updates(server_id, force).await
}

/// Applique des mises à jour (nouvelle version installée, ancien fichier supprimé).
#[tauri::command]
pub async fn apply_content_updates(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    items: Vec<UpdateSelection>,
) -> Result<UpdateReport, Error> {
    core.apply_content_updates(server_id, items).await
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

// --- Joueurs déjà venus (Minecraft Java) -----------------------------------

/// Joueurs déjà venus : recherche, filtre, tri, pagination et statistiques (liste complète gardée
/// une minute par le cœur ; `refresh` la relit).
#[tauri::command]
pub async fn known_players(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    query: PlayersQuery,
    refresh: bool,
) -> Result<PlayersPage, Error> {
    core.known_players(server_id, query, refresh).await
}

/// Fiche d'un joueur (statistiques, état, dates de venue).
#[tauri::command]
pub async fn player_profile(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    uuid: String,
) -> Result<PlayerProfile, Error> {
    core.player_profile(server_id, &uuid).await
}

/// Inventaire d'un joueur, en lecture seule.
#[tauri::command]
pub async fn player_inventory(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    uuid: String,
) -> Result<PlayerInventory, Error> {
    core.player_inventory(server_id, &uuid).await
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

// --- Mot de passe SFTP de secours (saisi par serveur quand celui de l'API manque) ---------

/// Vérifie le mot de passe en ouvrant une session, puis l'enregistre dans le stockage sécurisé.
/// Refusé par le serveur → erreur `sftp_auth`, rien n'est enregistré.
#[tauri::command]
pub async fn sftp_set_password(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    password: String,
) -> Result<(), Error> {
    core.set_sftp_password(server_id, &password).await
}

#[tauri::command]
pub fn sftp_forget_password(core: State<'_, Arc<Core>>, server_id: i64) -> Result<(), Error> {
    core.forget_sftp_password(server_id)
}

#[tauri::command]
pub fn sftp_has_password(core: State<'_, Arc<Core>>, server_id: i64) -> bool {
    core.has_sftp_password(server_id)
}

/// Identité du serveur SFTP (empreinte mémorisée, et celle présentée lors d'un refus).
#[tauri::command]
pub async fn sftp_host_key(core: State<'_, Arc<Core>>, server_id: i64) -> Result<SftpHostKey, Error> {
    core.sftp_host_key(server_id).await
}

/// Accepte la nouvelle empreinte présentée par le serveur SFTP (après confirmation explicite).
#[tauri::command]
pub async fn sftp_trust_host_key(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    fingerprint: String,
) -> Result<(), Error> {
    core.trust_sftp_host_key(server_id, &fingerprint).await
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

// --- Tâches planifiées du panel MineStrator (lecture + activation) ---------------------------

#[tauri::command]
pub async fn panel_schedules(core: State<'_, Arc<Core>>, server_id: i64) -> Result<Vec<PanelSchedule>, Error> {
    core.panel_schedules(server_id).await
}

#[tauri::command]
pub async fn panel_schedule_update(
    core: State<'_, Arc<Core>>,
    server_id: i64,
    schedule_id: i64,
    input: PanelScheduleInput,
) -> Result<(), Error> {
    core.update_panel_schedule(server_id, schedule_id, input).await
}
