/**
 * Couche IPC typée : unique point de contact entre le frontend et les commandes Rust.
 * Aucun composant n'appelle `invoke` directement — tout passe par `api`.
 */

import { invoke } from '@tauri-apps/api/core';
import { t } from './i18n';
import type {
  AppError,
  Backup,
  ChatReply,
  CopilotConfig,
  InstalledItem,
  LiveLight,
  MarketKind,
  MarketPage,
  MarketSource,
  MarketVersion,
  McpConfig,
  MetricSample,
  PlayerAction,
  PowerAction,
  PrivacyConfig,
  ServerDetails,
  ServersOverview,
  SftpEntry,
  Snapshot,
  SupervisorConfig,
  UserProfile,
} from './types';

export const api = {
  /** Valide une clé API et l'enregistre dans le trousseau. Renvoie le profil. */
  validateAndStoreKey: (key: string) => invoke<UserProfile>('validate_and_store_key', { key }),

  /** Une clé est-elle déjà enregistrée ? */
  hasStoredKey: () => invoke<boolean>('has_stored_key'),

  /** Profil de l'utilisateur courant (à partir de la clé enregistrée). */
  getUser: () => invoke<UserProfile>('get_user'),

  /** MyBox et serveurs de l'utilisateur. */
  listServers: () => invoke<ServersOverview>('list_servers'),

  /** Détails d'un serveur (inclut url/token WebSocket). */
  serverDetails: (id: number) => invoke<ServerDetails>('server_details', { id }),

  /** Données live allégées (limites, joueurs, version). */
  liveLight: (id: number) => invoke<LiveLight>('live_light', { id }),

  /** Historique de métriques (depuis N secondes) pour les graphes. */
  metricsHistory: (serverId: number, sinceSecs: number) =>
    invoke<MetricSample[]>('metrics_history', { serverId, sinceSecs }),

  /** Réglages du superviseur. */
  getSupervisorConfig: () => invoke<SupervisorConfig>('get_supervisor_config'),
  setSupervisorConfig: (config: SupervisorConfig) =>
    invoke<void>('set_supervisor_config', { config }),

  /** Réglages du serveur MCP. */
  getMcpConfig: () => invoke<McpConfig>('get_mcp_config'),
  setMcpConfig: (config: McpConfig) => invoke<void>('set_mcp_config', { config }),
  /** Réglages de confidentialité (anonymisation). */
  getPrivacyConfig: () => invoke<PrivacyConfig>('get_privacy_config'),
  setPrivacyConfig: (config: PrivacyConfig) => invoke<void>('set_privacy_config', { config }),
  /** Chemin de l'exécutable (pour composer la config Claude). */
  appExePath: () => invoke<string>('app_exe_path'),

  // --- Copilote (agent LLM multi-fournisseur) ---
  getCopilotConfig: () => invoke<CopilotConfig>('get_copilot_config'),
  setCopilotConfig: (config: CopilotConfig) => invoke<void>('set_copilot_config', { config }),
  /** Une clé LLM est-elle enregistrée pour le fournisseur sélectionné ? */
  hasCopilotKey: () => invoke<boolean>('has_copilot_key'),
  /** Enregistre la clé LLM du fournisseur sélectionné (trousseau OS). */
  setCopilotKey: (key: string) => invoke<void>('set_copilot_key', { key }),
  clearCopilotKey: () => invoke<void>('clear_copilot_key'),
  /** Applique une action proposée par le Copilote. Le server_id vient du diagnostic. */
  copilotApply: (serverId: number, tool: string, args: Record<string, unknown>) =>
    invoke<string>('copilot_apply', { serverId, tool, args }),
  /** Déclenche un diagnostic manuel pour un serveur. */
  copilotDiagnoseNow: (serverId: number, serverName: string) =>
    invoke<void>('copilot_diagnose_now', { serverId, serverName }),
  /** Analyse un extrait sélectionné (clic droit → Copilote). */
  copilotAnalyze: (serverId: number, serverName: string, text: string) =>
    invoke<void>('copilot_analyze', { serverId, serverName, text }),
  /** Lance une analyse de performance Spark (health/tps/gc + profiler). */
  copilotPerformance: (serverId: number, serverName: string) =>
    invoke<void>('copilot_performance', { serverId, serverName }),

  /** Assistant conversationnel : envoie un message, renvoie la réponse (+ actions). */
  chatSend: (
    sessionId: string,
    serverId: number,
    serverName: string,
    message: string,
    autonomous: boolean
  ) =>
    invoke<ChatReply>('chat_send', { sessionId, serverId, serverName, message, autonomous }),
  /** Réinitialise une conversation assistant. */
  chatReset: (sessionId: string) => invoke<void>('chat_reset', { sessionId }),
  /** Pré-chauffe (best-effort) le process agent persistant d'une session avant le 1er message. */
  chatWarm: (sessionId: string, autonomous: boolean) =>
    invoke<void>('chat_warm', { sessionId, autonomous }),

  // --- Filet de sécurité : backups & snapshots ---
  /** Backups quotidiens automatiques d'un serveur (récents d'abord). */
  listBackups: (serverId: number) => invoke<Backup[]>('list_backups', { serverId }),
  /** Snapshots de l'utilisateur (points de sauvegarde à la demande). */
  listSnapshots: () => invoke<Snapshot[]>('list_snapshots'),
  /** Crée un snapshot du serveur. Renvoie l'id du job asynchrone. */
  createSnapshot: (serverId: number, name: string) =>
    invoke<number>('create_snapshot', { serverId, name }),
  /** DESTRUCTIF : restaure un snapshot sur un serveur. Renvoie l'id du job. */
  restoreSnapshot: (snapshotId: number, serverId: number) =>
    invoke<number>('restore_snapshot', { snapshotId, serverId }),
  /** DESTRUCTIF : restaure un backup quotidien sur un serveur. */
  restoreBackup: (serverId: number, backupId: number) =>
    invoke<void>('restore_backup', { serverId, backupId }),
  /** Supprime définitivement un snapshot. Renvoie l'id du job. */
  deleteSnapshot: (snapshotId: number) => invoke<number>('delete_snapshot', { snapshotId }),

  // --- Marketplace (mods & plugins) ---
  /** Versions Minecraft connues du catalogue. */
  marketMinecraftVersions: () => invoke<string[]>('market_minecraft_versions'),
  /** Catalogue paginé. kind=mods|plugins, source=modrinth|curseforge|spigot. */
  marketList: (
    kind: MarketKind,
    source: MarketSource,
    page: number,
    query: string,
    loader: string,
    gameVersion: string
  ) =>
    invoke<MarketPage>('market_list', { kind, source, page, query, loader, gameVersion }),
  /** Versions d'un projet (slug Modrinth ou id numérique CurseForge/SpigotMC). */
  marketVersions: (source: MarketSource, slugOrId: string, loader: string, gameVersion: string) =>
    invoke<MarketVersion[]>('market_versions', { source, slugOrId, loader, gameVersion }),
  /** Installe un projet sur un serveur (source `modrinth` vérifiée). */
  installMod: (
    serverId: number,
    source: MarketSource,
    kind: 'mod' | 'plugin',
    slug: string,
    versionId: string,
    loader: string
  ) => invoke<void>('install_mod', { serverId, source, kind, slug, versionId, loader }),
  /** Mods installés sur un serveur. */
  installedMods: (serverId: number) => invoke<InstalledItem[]>('installed_mods', { serverId }),
  /** Plugins installés sur un serveur. */
  installedPlugins: (serverId: number) =>
    invoke<InstalledItem[]>('installed_plugins', { serverId }),

  /** 100 dernières lignes de console (ANSI). */
  consoleLogs: (id: number) => invoke<string[]>('console_logs', { id }),

  /** Action d'alimentation (start/stop/restart/kill…). */
  powerAction: (id: number, action: PowerAction) =>
    invoke<void>('power_action', { id, action }),

  /** Envoie une commande console. */
  sendCommand: (id: number, command: string) => invoke<void>('send_command', { id, command }),

  /** Action joueur Minecraft (kick/ban/unban/op/whitelist). */
  playerAction: (serverId: number, action: PlayerAction, player: string) =>
    invoke<void>('player_action', { id: serverId, action, player }),

  /** Ouvre une connexion console WebSocket (identifiée par connId). */
  consoleConnect: (connId: string, serverId: number) =>
    invoke<void>('console_connect', { connId, serverId }),

  /** Ferme la connexion console connId. */
  consoleDisconnect: (connId: string) => invoke<void>('console_disconnect', { connId }),

  // --- SFTP --- (Tauri mappe les clés camelCase JS → snake_case Rust)
  sftpList: (serverId: number, path: string) =>
    invoke<SftpEntry[]>('sftp_list', { serverId, path }),
  sftpReadText: (serverId: number, path: string) =>
    invoke<string>('sftp_read_text', { serverId, path }),
  sftpWriteText: (serverId: number, path: string, content: string) =>
    invoke<void>('sftp_write_text', { serverId, path, content }),
  sftpMkdir: (serverId: number, path: string) =>
    invoke<void>('sftp_mkdir', { serverId, path }),
  sftpDelete: (serverId: number, path: string, isDir: boolean) =>
    invoke<void>('sftp_delete', { serverId, path, isDir }),
  sftpRename: (serverId: number, from: string, to: string) =>
    invoke<void>('sftp_rename', { serverId, from, to }),
  sftpUpload: (serverId: number, localPath: string, remoteDir: string) =>
    invoke<string>('sftp_upload', { serverId, localPath, remoteDir }),
  sftpDownload: (serverId: number, remotePath: string, localPath: string) =>
    invoke<void>('sftp_download', { serverId, remotePath, localPath }),
  sftpDisconnect: (serverId: number) => invoke<void>('sftp_disconnect', { serverId }),

  /** Efface la clé enregistrée. */
  logout: () => invoke<void>('logout'),
};

/** Garde de type : un rejet d'invoke porte-t-il la forme d'un `AppError` ? */
export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'kind' in value &&
    'message' in value
  );
}

/** Message d'erreur prêt à afficher, adapté au cas (i18n). */
export function humanizeError(value: unknown): string {
  if (!isAppError(value)) return String(value);
  const known: Record<string, string> = {
    unauthorized: 'errors.unauthorized',
    forbidden: 'errors.forbidden',
    rate_limited: 'errors.rate_limited',
    network: 'errors.network',
    no_key: 'errors.no_key',
  };
  const key = known[value.kind];
  return key ? t(key) : value.message;
}
