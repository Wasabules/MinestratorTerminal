/**
 * Couche IPC typée : unique point de contact frontend ↔ commandes Rust.
 * Aucun composant n'appelle `invoke` directement — tout passe par `api`.
 */

import { invoke } from "@tauri-apps/api/core";
import {
  isFake,
  fakePlayer,
  fakeServer,
  fakeBox,
  fakeIp,
  fakeAddress,
  fakeMail,
  fakeRef,
  fakeText,
} from "./fake.svelte";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { t } from "./i18n";
import type {
  ActivityPage,
  AppError,
  ArchiveEntry,
  Backup,
  ConsoleStats,
  Egg,
  LiveLight,
  LivePlayers,
  MetricSample,
  PlayerAction,
  PowerAction,
  ServerDetails,
  ServersOverview,
  SftpEntry,
  SftpSearchResult,
  Snapshot,
  UpdateInfo,
  UserNotification,
  UserProfile,
} from "./types";

/**
 * Substitution des données identifiantes en mode démo. Appliquée ici parce que `ipc.ts` est le
 * point de passage unique vers le Rust : aucune vue ne peut afficher une valeur réelle en
 * contournant le filtre. Ciblé champ par champ — un parcours aveugle falsifierait aussi des noms
 * de fichiers SFTP, qui n'ont rien de sensible.
 */
async function demo<T>(value: Promise<T>, transform: (v: T) => T): Promise<T> {
  const v = await value;
  return isFake() ? transform(v) : v;
}

export const api = {
  // --- Auth ---
  validateAndStoreKey: (key: string) => invoke<UserProfile>("validate_and_store_key", { key }),
  hasStoredKey: () => invoke<boolean>("has_stored_key"),
  getUser: () =>
    demo(invoke<UserProfile>("get_user"), (u) => ({
      ...u,
      pseudo: fakePlayer(u.pseudo),
      mail: fakeMail(u.mail),
    })),
  logout: () => invoke<void>("logout"),

  // --- Serveurs ---
  listServers: () =>
    demo(invoke<ServersOverview>("list_servers"), (o) => ({
      myboxes: o.myboxes.map((m) => ({ ...m, name: fakeBox(m.name) })),
      servers: o.servers.map((s) => ({
        ...s,
        name: fakeServer(s.name),
        address: fakeAddress(s.address),
        support_ref: s.support_ref ? fakeRef(s.support_ref) : s.support_ref,
      })),
    })),
  serverDetails: (id: number) => invoke<ServerDetails>("server_details", { id }),
  liveLight: (id: number) =>
    demo(invoke<LiveLight>("live_light", { id }), (l) => ({
      ...l,
      motd: l.motd ? fakeText(l.motd) : l.motd,
      hostname: l.hostname ? fakeText(l.hostname) : l.hostname,
      players: l.players
        ? { ...l.players, list: l.players.list.map((p) => fakePlayer(p)) }
        : l.players,
    })),
  /** Joueurs en ligne de plusieurs serveurs en UN appel (liste). Joueurs uniquement. */
  livePlayersBulk: (ids: number[]) => invoke<LivePlayers[]>("live_players_bulk", { ids }),
  /** Catalogue des eggs de l'hébergeur. */
  listEggs: () => invoke<Egg[]>("list_eggs"),
  /** Journal d'audit d'un serveur (paginé). Nomme l'auteur de chaque action. */
  serverActivity: (id: number, limit = 20, offset = 0) =>
    demo(invoke<ActivityPage>("server_activity", { id, limit, offset }), (page) => ({
      ...page,
      items: page.items.map((e) => ({
        ...e,
        actor: e.actor ? fakePlayer(e.actor) : e.actor,
        ip: e.ip ? fakeIp(e.ip) : e.ip,
        context: e.context ? fakeText(e.context) : e.context,
      })),
    })),
  /** Notifications de l'hébergeur (lecture seule ; livrées avec le profil). */
  userNotifications: () =>
    demo(invoke<UserNotification[]>("user_notifications"), (list) =>
      list.map((n) => ({ ...n, message: fakeText(n.message) })),
    ),
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
  /** Recherche récursive par nom sous `root` (le core borne le parcours). */
  sftpSearch: (serverId: number, root: string, query: string) =>
    invoke<SftpSearchResult>("sftp_search", { serverId, root, query }),
  /** Image rendue en data URI, affichable directement dans un `<img>`. */
  sftpReadDataUri: (serverId: number, path: string) =>
    invoke<string>("sftp_read_data_uri", { serverId, path }),
  /** Contenu d'une archive, en lecture seule. */
  sftpArchiveList: (serverId: number, path: string) =>
    invoke<ArchiveEntry[]>("sftp_archive_list", { serverId, path }),
  sftpArchiveReadText: (serverId: number, path: string, entry: string) =>
    invoke<string>("sftp_archive_read_text", { serverId, path, entry }),
  /**
   * Nom réel d'un fichier choisi dans le sélecteur. Indispensable sur Android : le SAF rend un
   * `content://` dont le dernier segment n'est qu'un identifiant (`…/document/image%3A3604`).
   */
  fileDisplayName: (uri: string) => invoke<string>("file_display_name", { uri }),
  /**
   * Envoie un fichier choisi dans le sélecteur, **sans faire transiter les octets par le
   * webview** : Kotlin cède le descripteur au Rust, qui lit et écrit en SFTP. La progression
   * arrive par l'event `sftp://progress`.
   */
  sftpUploadUri: (
    serverId: number,
    uri: string,
    name: string,
    remoteDir: string,
    transferId: string,
  ) => invoke<void>("sftp_upload_uri", { serverId, uri, name, remoteDir, transferId }),
  /** Demande l'arrêt d'un transfert en cours (arrêt coopératif côté Rust). */
  cancelTransfer: (transferId: string) => invoke<void>("cancel_transfer", { transferId }),
  /** Taille d'un fichier distant (total d'un téléchargement, avant de lire par blocs). */
  sftpFileSize: (serverId: number, path: string) =>
    invoke<number>("sftp_file_size", { serverId, path }),
  /** Lit une tranche d'un fichier distant, en base64. */
  sftpDownloadChunk: (serverId: number, path: string, offset: number, len: number) =>
    invoke<string>("sftp_download_chunk", { serverId, path, offset, len }),
  /** Notification système de progression d'un transfert (Android). */
  notifyTransfer: (
    id: number,
    title: string,
    text: string,
    done: number,
    total: number,
    finished: boolean,
  ) => invoke<void>("notify_transfer", { id, title, text, done, total, finished }),
  clearTransfer: (id: number) => invoke<void>("clear_transfer", { id }),
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

  // --- Surveillance en arrière-plan (alertes app fermée) ---
  /** Démarre/arrête le service au premier plan qui garde la surveillance active app fermée. */
  setBackgroundMonitoring: (enabled: boolean) =>
    invoke<void>("set_background_monitoring", { enabled }),
  /** `true` si l'app est exemptée d'optimisation batterie (peut tourner en arrière-plan). */
  isBatteryUnrestricted: () => invoke<boolean>("is_battery_unrestricted"),
  /** Ouvre la boîte de dialogue système pour exempter l'app de l'optimisation batterie. */
  requestBatteryUnrestricted: () => invoke<void>("request_battery_unrestricted"),
  /** Ouvre la fiche « infos de l'app » (batterie, autostart selon l'OEM, notifications). */
  openAppSettings: () => invoke<void>("open_app_settings"),
};

/** Ouvre une URL dans le navigateur externe (plugin opener). */
export function openExternal(url: string): Promise<void> {
  return openUrl(url);
}

/**
 * S'assure que la permission de notifications est accordée (obligatoire sur Android 13+ :
 * `POST_NOTIFICATIONS` doit être demandée à l'exécution, sinon les alertes sont bloquées
 * silencieusement). Ne redemande pas si déjà accordée. Renvoie `true` si autorisé.
 */
export async function ensureNotificationPermission(): Promise<boolean> {
  try {
    let granted = await isPermissionGranted();
    if (!granted) {
      granted = (await requestPermission()) === "granted";
    }
    return granted;
  } catch {
    return false;
  }
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
