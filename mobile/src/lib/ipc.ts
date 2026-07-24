/**
 * Couche IPC typée : unique point de contact frontend ↔ commandes Rust.
 * Aucun composant n'appelle `invoke` directement — tout passe par `api`.
 */

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { t } from "./i18n";
import type {
  AppError,
  Backup,
  ConsoleStats,
  LiveLight,
  MetricSample,
  PlayerAction,
  PowerAction,
  ServerDetails,
  ServersOverview,
  SftpEntry,
  Snapshot,
  UpdateInfo,
  UserProfile,
} from "./types";

export const api = {
  // --- Auth ---
  validateAndStoreKey: (key: string) => invoke<UserProfile>("validate_and_store_key", { key }),
  hasStoredKey: () => invoke<boolean>("has_stored_key"),
  getUser: () => invoke<UserProfile>("get_user"),
  logout: () => invoke<void>("logout"),

  // --- Serveurs ---
  listServers: () => invoke<ServersOverview>("list_servers"),
  serverDetails: (id: number) => invoke<ServerDetails>("server_details", { id }),
  liveLight: (id: number) => invoke<LiveLight>("live_light", { id }),
  metricsHistory: (serverId: number, sinceSecs: number) =>
    invoke<MetricSample[]>("metrics_history", { serverId, sinceSecs }),
  /** Échantillon live de stats (CPU/RAM/disque) via une connexion monitor éphémère. */
  sampleStats: (serverId: number) => invoke<ConsoleStats | null>("sample_stats", { serverId }),

  // --- Console / power / joueurs ---
  consoleLogs: (id: number) => invoke<string[]>("console_logs", { id }),
  powerAction: (id: number, action: PowerAction) => invoke<void>("power_action", { id, action }),
  sendCommand: (id: number, command: string) => invoke<void>("send_command", { id, command }),
  playerAction: (id: number, action: PlayerAction, player: string) =>
    invoke<void>("player_action", { id, action, player }),
  consoleConnect: (connId: string, serverId: number) =>
    invoke<void>("console_connect", { connId, serverId }),
  consoleDisconnect: (connId: string) => invoke<void>("console_disconnect", { connId }),

  // --- SFTP ---
  sftpList: (serverId: number, path: string) =>
    invoke<SftpEntry[]>("sftp_list", { serverId, path }),
  sftpReadText: (serverId: number, path: string) =>
    invoke<string>("sftp_read_text", { serverId, path }),
  sftpWriteText: (serverId: number, path: string, content: string) =>
    invoke<void>("sftp_write_text", { serverId, path, content }),
  sftpMkdir: (serverId: number, path: string) => invoke<void>("sftp_mkdir", { serverId, path }),
  sftpDelete: (serverId: number, path: string, isDir: boolean) =>
    invoke<void>("sftp_delete", { serverId, path, isDir }),
  sftpRename: (serverId: number, from: string, to: string) =>
    invoke<void>("sftp_rename", { serverId, from, to }),
  sftpGzText: (serverId: number, path: string) => invoke<string>("sftp_gz_text", { serverId, path }),

  // --- Sauvegardes ---
  listBackups: (id: number) => invoke<Backup[]>("list_backups", { id }),
  restoreBackup: (serverId: number, backupId: number) =>
    invoke<void>("restore_backup", { serverId, backupId }),
  listSnapshots: () => invoke<Snapshot[]>("list_snapshots"),
  createSnapshot: (serverId: number, name: string) =>
    invoke<number>("create_snapshot", { serverId, name }),
  restoreSnapshot: (snapshotId: number, serverId: number) =>
    invoke<number>("restore_snapshot", { snapshotId, serverId }),
  deleteSnapshot: (snapshotId: number) => invoke<number>("delete_snapshot", { snapshotId }),

  // --- Auto-update (mobile) ---
  checkUpdate: () => invoke<UpdateInfo | null>("check_update"),
  downloadUpdate: (url: string) => invoke<string>("download_update", { url }),
  /** Lance l'installeur système Android sur l'APK téléchargé (commande JNI native). */
  installApk: (path: string) => invoke<void>("install_apk", { path }),
};

/** Ouvre une URL dans le navigateur externe (plugin opener). */
export function openExternal(url: string): Promise<void> {
  return openUrl(url);
}

/** Transforme une erreur du core (`{ kind, message }`) en message lisible/i18n. */
export function humanizeError(e: unknown): string {
  const err = e as Partial<AppError> | undefined;
  if (err && typeof err === "object" && "kind" in err) {
    const byKind = t(`error.${err.kind}`);
    if (byKind !== `error.${err.kind}`) return byKind;
    if (err.message) return err.message;
  }
  if (typeof e === "string") return e;
  return t("error.generic");
}
