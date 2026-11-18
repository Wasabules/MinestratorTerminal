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
  fakeProperty,
} from "./fake.svelte";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { t } from "./i18n";
import type {
  Account,
  ActivityPage,
  AlertEntry,
  AppError,
  ArchiveEntry,
  Backup,
  ConsoleStats,
  ContentUpdates,
  Egg,
  InstalledItem,
  LiveLight,
  LivePlayers,
  MarketKind,
  MarketPage,
  MarketSource,
  MarketVersion,
  MetricSample,
  PlayerAction,
  PowerAction,
  ServerDetails,
  ServerProperties,
  ServersOverview,
  SftpEntry,
  SftpHostKey,
  PanelSchedule,
  PanelScheduleInput,
  SftpSearchResult,
  Snapshot,
  UpdateInfo,
  UpdateReport,
  UpdateSelection,
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

/** Mode démo : profil du compte (pseudo et e-mail). */
function fakeProfile(u: UserProfile): UserProfile {
  return { ...u, pseudo: fakePlayer(u.pseudo), mail: fakeMail(u.mail) };
}

/** Mode démo : compte enregistré (pseudo). */
function fakeAccount(a: Account): Account {
  return { ...a, pseudo: fakePlayer(a.pseudo) };
}

export const api = {
  // --- Auth ---
  /** Connexion : la clé devient celle du compte actif (compte ajouté, ou sa clé remplacée). */
  validateAndStoreKey: (key: string) =>
    demo(invoke<UserProfile>("validate_and_store_key", { key }), fakeProfile),
  hasStoredKey: () => invoke<boolean>("has_stored_key"),
  getUser: () => demo(invoke<UserProfile>("get_user"), fakeProfile),
  /** Retire le compte actif ; renvoie celui qui prend la relève (`null` : écran de connexion). */
  logout: () => demo(invoke<Account | null>("logout"), (a) => (a ? fakeAccount(a) : a)),

  // --- Comptes (plusieurs comptes MineStrator) ---
  accountsList: () => demo(invoke<Account[]>("accounts_list"), (list) => list.map(fakeAccount)),
  /** Ajoute un compte (clé validée, doublon refusé), rangé inactif. */
  accountAdd: (key: string) => demo(invoke<Account>("account_add", { key }), fakeAccount),
  /** Bascule vers un compte enregistré (le core émet `account://switched`). */
  accountSwitch: (id: number) => demo(invoke<Account>("account_switch", { id }), fakeAccount),
  /** Retire un compte ; renvoie le compte actif après coup (`null` : aucun). */
  accountRemove: (id: number) =>
    demo(invoke<Account | null>("account_remove", { id }), (a) => (a ? fakeAccount(a) : a)),

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
  /** Historique local (SQLite du core) des échantillons du superviseur, depuis `sinceSecs`. */
  metricsHistory: (serverId: number, sinceSecs: number) =>
    invoke<MetricSample[]>("metrics_history", { serverId, sinceSecs }),
  /** Échantillon live de stats (CPU/RAM/disque) via une connexion monitor éphémère. */
  sampleStats: (serverId: number) => invoke<ConsoleStats | null>("sample_stats", { serverId }),

  // --- Paramètres du serveur (`server.properties`, via l'API) ---
  /** Réglages du serveur ; ceux qui l'identifient sont substitués en mode démo. */
  serverProperties: (id: number) =>
    demo(invoke<ServerProperties>("server_properties", { id }), (p) => ({
      ...p,
      properties: Object.fromEntries(
        Object.entries(p.properties).map(([k, v]) => [k, fakeProperty(k, v)]),
      ),
    })),
  /** Modifie UN réglage ; effet au prochain démarrage du serveur. */
  setServerProperty: (id: number, property: string, value: string) =>
    invoke<void>("set_server_property", { id, property, value }),

  // --- Centre d'alertes (journal local des alertes du superviseur) ---
  /**
   * Alertes consignées, les plus récentes d'abord. Le message cite le serveur ou la MyBox
   * concernée : en mode démo, le nom est substitué partout où il apparaît.
   */
  alertsList: () =>
    demo(invoke<AlertEntry[]>("alerts_list"), (list) =>
      list.map((a) => {
        const name = a.kind === "expiry" ? fakeBox(a.server_name) : fakeServer(a.server_name);
        const message = a.server_name
          ? fakeText(a.message).replaceAll(a.server_name, name)
          : fakeText(a.message);
        return { ...a, server_name: name, message };
      }),
    ),
  alertsMarkRead: () => invoke<void>("alerts_mark_read"),
  alertsClear: () => invoke<void>("alerts_clear"),

  // --- Marketplace Minecraft (mods & plugins, via l'API : pas de SFTP) ---
  marketMinecraftVersions: () => invoke<string[]>("market_minecraft_versions"),
  /** Catalogue paginé (18 par page). `kind` = mods|plugins, `source` = modrinth|curseforge|spigot. */
  marketList: (
    kind: MarketKind,
    source: MarketSource,
    page: number,
    query: string,
    loader: string,
    gameVersion: string,
  ) => invoke<MarketPage>("market_list", { kind, source, page, query, loader, gameVersion }),
  /** Versions d'un projet (slug Modrinth ou identifiant numérique CurseForge/SpigotMC). */
  marketVersions: (source: MarketSource, slugOrId: string, loader: string, gameVersion: string) =>
    invoke<MarketVersion[]>("market_versions", { source, slugOrId, loader, gameVersion }),
  /** Installe un projet sur le serveur (sources `modrinth` et `spigot` uniquement). */
  installMod: (
    serverId: number,
    source: MarketSource,
    kind: "mod" | "plugin",
    slug: string,
    versionId: string,
    loader: string,
  ) => invoke<void>("install_mod", { serverId, source, kind, slug, versionId, loader }),
  installedMods: (serverId: number) => invoke<InstalledItem[]>("installed_mods", { serverId }),
  installedPlugins: (serverId: number) =>
    invoke<InstalledItem[]>("installed_plugins", { serverId }),
  /** Datapacks du monde actif (`filename` = nom exact, clé des mises à jour). */
  installedDatapacks: (serverId: number) =>
    invoke<InstalledItem[]>("installed_datapacks", { serverId }),
  /**
   * Mises à jour disponibles des mods, plugins et datapacks (vérification de l'hébergeur passée
   * au garde-fou du core). Gardées 15 min côté Rust ; `force` refait la vérification.
   */
  contentUpdates: (serverId: number, force = false) =>
    invoke<ContentUpdates>("content_updates", { serverId, force }),
  /** Applique des mises à jour : effet au prochain démarrage (`restart_needed`). */
  applyContentUpdates: (serverId: number, items: UpdateSelection[]) =>
    invoke<UpdateReport>("apply_content_updates", { serverId, items }),

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
  /** Vérifie le mot de passe SFTP (session ouverte) puis l'enregistre dans le Keystore. */
  sftpSetPassword: (serverId: number, password: string) =>
    invoke<void>("sftp_set_password", { serverId, password }),
  sftpForgetPassword: (serverId: number) => invoke<void>("sftp_forget_password", { serverId }),
  sftpHasPassword: (serverId: number) => invoke<boolean>("sftp_has_password", { serverId }),
  /** Identité du serveur SFTP : empreinte mémorisée et, après un refus, celle présentée. */
  sftpHostKey: (serverId: number) => invoke<SftpHostKey>("sftp_host_key", { serverId }),

  // --- Tâches planifiées du panel MineStrator (lecture + activation) ---
  panelSchedules: (serverId: number) => invoke<PanelSchedule[]>("panel_schedules", { serverId }),
  panelScheduleUpdate: (serverId: number, scheduleId: number, input: PanelScheduleInput) =>
    invoke<void>("panel_schedule_update", { serverId, scheduleId, input }),
  /** Accepte l'empreinte présentée par le serveur (exactement celle affichée à l'utilisateur). */
  sftpTrustHostKey: (serverId: number, fingerprint: string) =>
    invoke<void>("sftp_trust_host_key", { serverId, fingerprint }),
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
  /** Journaux récents anonymisés (rapport de bug). Chaîne vide s'il n'y en a aucun. */
  exportLogs: () => invoke<string>("export_logs"),
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
