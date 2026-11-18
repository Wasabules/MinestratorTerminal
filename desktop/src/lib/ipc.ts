/**
 * Couche IPC typée : unique point de contact entre le frontend et les commandes Rust.
 * Aucun composant n'appelle `invoke` directement — tout passe par `api`.
 */

import { invoke } from '@tauri-apps/api/core';
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
  fakeUuid,
  realUuid,
} from './fake.svelte';
import { t } from './i18n';
import type {
  Account,
  ActivityPage,
  Alert,
  AppError,
  AppErrorKind,
  ArchiveEntry,
  AutomationRun,
  Backup,
  ChatReply,
  CliStatus,
  ContentUpdates,
  CopilotConfig,
  Egg,
  FicsitInstallItem,
  FicsitInstalledMod,
  FicsitModPage,
  FicsitVersion,
  GameSettings,
  InstalledItem,
  JsonValue,
  KnownPlayer,
  LiveLight,
  MyBoxDetails,
  ServerLive,
  ServerProperties,
  LivePlayers,
  MarketInstalledMod,
  MarketModPage,
  MarketModVersion,
  MarketKind,
  MarketPage,
  MarketSource,
  MarketVersion,
  McpConfig,
  MetricsHistory,
  ModInstallItem,
  NbtNode,
  PlayerAction,
  PlayerInventory,
  PlayerProfile,
  PlayersPage,
  PlayersQuery,
  PowerAction,
  PrivacyConfig,
  RegionChunk,
  RestartRule,
  UpdateReport,
  UpdateSelection,
  ScheduledTask,
  ServerAutomations,
  ServerDetails,
  ServersOverview,
  ServiceOverview,
  SftpEntry,
  SftpHostKey,
  PanelSchedule,
  PanelScheduleInput,
  PanelTaskInput,
  SmlVersion,
  Snapshot,
  SupervisorConfig,
  TaskInfo,
  UptimeReport,
  UserNotification,
  UserProfile,
  WebhookDestination,
  WebhookInfo,
  WebhookKind,
} from './types';

/**
 * Substitution des données identifiantes, appliquée aux réponses IPC quand le mode démo est actif.
 *
 * C'est le bon endroit : `ipc.ts` est le point de passage unique entre le front et le Rust
 * (aucun composant n'appelle `invoke` directement), donc une vue ne peut pas afficher une valeur
 * réelle en contournant le filtre. Les transformations sont ciblées champ par champ plutôt que
 * récursives : un parcours aveugle sur les noms de champs falsifierait aussi des noms de fichiers
 * SFTP ou des libellés d'egg, qui n'ont rien de sensible.
 */
async function demo<T>(value: Promise<T>, transform: (v: T) => T): Promise<T> {
  const v = await value;
  return isFake() ? transform(v) : v;
}

/** Mode démo : le pseudo du compte surveillé par le service est substitué comme les autres. */
function fakeServiceOverview(o: ServiceOverview): ServiceOverview {
  if (!o.status?.account_name) return o;
  return { ...o, status: { ...o.status, account_name: fakePlayer(o.status.account_name) } };
}

/** Mode démo : profil du compte (pseudo et e-mail). */
function fakeProfile(u: UserProfile): UserProfile {
  return { ...u, pseudo: fakePlayer(u.pseudo), mail: fakeMail(u.mail) };
}

/** Mode démo : compte enregistré (pseudo). */
function fakeAccount(a: Account): Account {
  return { ...a, pseudo: fakePlayer(a.pseudo) };
}

/** Mode démo : joueur déjà venu (pseudo et UUID ; l'UUID affiché reste utilisable, cf. `realUuid`). */
function fakeKnownPlayer(p: KnownPlayer): KnownPlayer {
  return { ...p, uuid: fakeUuid(p.uuid), name: p.name ? fakePlayer(p.name) : p.name };
}

/** Mode démo : objet d'inventaire (un nom personnalisé peut contenir un pseudo, une adresse). */
function fakeItem<T extends { name: string; custom_name: boolean } | null>(item: T): T {
  return item && item.custom_name ? { ...item, name: fakeText(item.name) } : item;
}

export const api = {
  /**
   * Connexion (écran de connexion) : valide la clé et en fait la clé du compte actif — compte
   * ajouté s'il est nouveau, clé remplacée s'il était déjà enregistré. Renvoie le profil.
   */
  validateAndStoreKey: (key: string) =>
    demo(invoke<UserProfile>('validate_and_store_key', { key }), fakeProfile),

  /** Une clé est-elle déjà enregistrée ? */
  hasStoredKey: () => invoke<boolean>('has_stored_key'),

  /** Profil du compte actif (à partir de la clé enregistrée). */
  getUser: () => demo(invoke<UserProfile>('get_user'), fakeProfile),

  // --- Comptes (plusieurs comptes MineStrator, changement de compte) ---
  /** Comptes enregistrés sur l'appareil, dans l'ordre d'ajout (`active` marque le compte actif). */
  accountsList: () => demo(invoke<Account[]>('accounts_list'), (list) => list.map(fakeAccount)),
  /** Ajoute un compte (clé validée par l'API, doublon refusé). Il est rangé inactif. */
  accountAdd: (key: string) => demo(invoke<Account>('account_add', { key }), fakeAccount),
  /**
   * Bascule vers un compte enregistré. Le core ferme consoles et sessions SFTP, relance la
   * supervision et émet `account://switched` (toutes les fenêtres se rechargent).
   */
  accountSwitch: (id: number) => demo(invoke<Account>('account_switch', { id }), fakeAccount),
  /** Retire un compte (clé et mots de passe SFTP). Renvoie le compte actif après coup (`null` : aucun). */
  accountRemove: (id: number) =>
    demo(invoke<Account | null>('account_remove', { id }), (a) => (a ? fakeAccount(a) : a)),

  /** MyBox et serveurs de l'utilisateur. */
  listServers: () =>
    demo(invoke<ServersOverview>('list_servers'), (o) => ({
      myboxes: o.myboxes.map((m) => ({ ...m, name: fakeBox(m.name) })),
      servers: o.servers.map((s) => ({
        ...s,
        name: fakeServer(s.name),
        address: fakeAddress(s.address),
        support_ref: s.support_ref ? fakeRef(s.support_ref) : s.support_ref,
        // `unavailable_reason` n'est pas touché : c'est une clé i18n de l'hébergeur
        // (`PANEL.SERVERS_LIST...`), elle n'identifie personne.
      })),
    })),

  /** Détails d'un serveur (inclut url/token WebSocket). */
  serverDetails: (id: number) => invoke<ServerDetails>('server_details', { id }),

  /** Données live allégées (limites, joueurs, version). */
  liveLight: (id: number) =>
    demo(invoke<LiveLight>('live_light', { id }), (l) => ({
      ...l,
      motd: l.motd ? fakeText(l.motd) : l.motd,
      hostname: l.hostname ? fakeText(l.hostname) : l.hostname,
      players: l.players
        ? { ...l.players, list: l.players.list.map((p) => fakePlayer(p)) }
        : l.players,
    })),

  /**
   * État temps réel complet : consommation instantanée (CPU en centièmes de cœur, dont FlexCore
   * via `is_bursting` ; RAM/disque en Mo), réseau cumulé, uptime. Blocs absents serveur éteint.
   */
  serverLive: (id: number) =>
    demo(invoke<ServerLive>('server_live', { id }), (l) => ({
      ...l,
      stats: {
        ...l.stats,
        motd: l.stats.motd ? fakeText(l.stats.motd) : l.stats.motd,
        hostname: l.stats.hostname ? fakeText(l.stats.hostname) : l.stats.hostname,
        players: l.stats.players
          ? { ...l.stats.players, list: l.stats.players.list.map((p) => fakePlayer(p)) }
          : l.stats.players,
      },
    })),

  /** Réglages du serveur (`server.properties`), sans SFTP. */
  serverProperties: (id: number) =>
    demo(invoke<ServerProperties>('server_properties', { id }), (p) => ({
      ...p,
      properties: Object.fromEntries(
        Object.entries(p.properties).map(([k, v]) => [k, fakeProperty(k, v)])
      ),
    })),
  /** Modifie UN réglage existant ; effet au prochain démarrage. */
  setServerProperty: (id: number, property: string, value: string) =>
    invoke<void>('set_server_property', { id, property, value }),

  /** Détail d'une MyBox : renouvellement auto, ressources restantes, offre, serveurs. */
  myboxDetails: (id: number) =>
    demo(invoke<MyBoxDetails>('mybox_details', { id }), (d) => ({
      ...d,
      mybox: { ...d.mybox, name: fakeBox(d.mybox.name) },
      servers: d.servers.map((s) => ({ ...s, name: fakeServer(s.name) })),
    })),

  /**
   * Joueurs en ligne de plusieurs serveurs en UN appel — pour une liste, là où `liveLight`
   * coûterait une requête par serveur. Ne renvoie que les joueurs (ni CPU, ni disque, ni statut).
   */
  livePlayersBulk: (ids: number[]) => invoke<LivePlayers[]>('live_players_bulk', { ids }),

  /** Catalogue des eggs de l'hébergeur (jeux + bases de données). Mis en cache côté Rust. */
  listEggs: () => invoke<Egg[]>('list_eggs'),

  /** Journal d'audit d'un serveur (paginé). Nomme l'auteur de chaque action. */
  serverActivity: (id: number, limit = 20, offset = 0) =>
    demo(invoke<ActivityPage>('server_activity', { id, limit, offset }), (page) => ({
      ...page,
      items: page.items.map((e) => ({
        ...e,
        actor: e.actor ? fakePlayer(e.actor) : e.actor,
        ip: e.ip ? fakeIp(e.ip) : e.ip,
        // Le contexte porte la commande tapée : elle peut nommer un joueur ou une IP.
        context: e.context ? fakeText(e.context) : e.context,
      })),
    })),

  /** Notifications de l'hébergeur (lecture seule ; livrées avec le profil). */
  userNotifications: () =>
    demo(invoke<UserNotification[]>('user_notifications'), (list) =>
      list.map((n) => ({ ...n, message: fakeText(n.message) })),
    ),

  /**
   * Historique des `sinceSecs` dernières secondes (30 jours au plus) : surveillance locale complétée
   * par les statistiques de l'hébergeur (origine de chaque point dans `source`), joueurs connectés,
   * au plus `maxPoints` points (600 par défaut). Que des mesures : rien à masquer en mode démo.
   */
  metricsHistory: (serverId: number, sinceSecs: number, maxPoints?: number) =>
    invoke<MetricsHistory>('metrics_history', { serverId, sinceSecs, maxPoints: maxPoints ?? null }),

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

  /** Détecte les agents CLI installés (claude / opencode / gemini) + leur version. */
  detectClis: () => invoke<CliStatus[]>('detect_clis'),

  // --- Copilote (agent LLM multi-fournisseur) ---
  getCopilotConfig: () => invoke<CopilotConfig>('get_copilot_config'),
  setCopilotConfig: (config: CopilotConfig) => invoke<void>('set_copilot_config', { config }),
  /** Une clé LLM est-elle enregistrée pour le fournisseur sélectionné ? */
  hasCopilotKey: () => invoke<boolean>('has_copilot_key'),
  /** Enregistre la clé LLM du fournisseur sélectionné (trousseau OS). */
  setCopilotKey: (key: string) => invoke<void>('set_copilot_key', { key }),
  clearCopilotKey: () => invoke<void>('clear_copilot_key'),
  /** Applique une action proposée par le Copilote. Le server_id vient du diagnostic. */
  copilotApply: (serverId: number, tool: string, args: JsonValue) =>
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
  /** Datapacks du monde actif (`filename` = nom exact, clé des mises à jour). */
  installedDatapacks: (serverId: number) =>
    invoke<InstalledItem[]>('installed_datapacks', { serverId }),
  /**
   * Mises à jour disponibles des mods, plugins et datapacks (vérification de l'hébergeur passée
   * au garde-fou du core). Gardées 15 min côté Rust ; `force` refait la vérification.
   */
  contentUpdates: (serverId: number, force = false) =>
    invoke<ContentUpdates>('content_updates', { serverId, force }),
  /** Applique des mises à jour : effet au prochain démarrage (`restart_needed`). */
  applyContentUpdates: (serverId: number, items: UpdateSelection[]) =>
    invoke<UpdateReport>('apply_content_updates', { serverId, items }),

  // --- Mods Satisfactory (ficsit.app / SMR) ---
  /** Recherche / tri paginé du catalogue ficsit.app (search vide/court = parcours trié). */
  ficsitSearch: (
    search: string,
    offset: number,
    limit: number,
    orderBy: string,
    order: string
  ) => invoke<FicsitModPage>('ficsit_search', { search, offset, limit, orderBy, order }),
  /** Versions d'un mod ficsit (par id). */
  ficsitModVersions: (modId: string) =>
    invoke<FicsitVersion[]>('ficsit_mod_versions', { modId }),
  /** Versions disponibles du Satisfactory Mod Loader (SML). */
  ficsitSmlVersions: () => invoke<SmlVersion[]>('ficsit_sml_versions'),
  /** Mods Satisfactory installés sur le serveur (listing SFTP du dossier Mods/). */
  ficsitInstalled: (serverId: number) =>
    invoke<FicsitInstalledMod[]>('ficsit_installed', { serverId }),
  /** Active/désactive un mod installé. */
  ficsitSetEnabled: (serverId: number, reference: string, enabled: boolean) =>
    invoke<void>('ficsit_set_enabled', { serverId, reference, enabled }),
  /** Supprime définitivement un mod installé. */
  ficsitRemove: (serverId: number, reference: string) =>
    invoke<void>('ficsit_remove', { serverId, reference }),
  /** Installe un lot de mods (fond, un seul redémarrage) : suivi via mods://install-progress. */
  ficsitInstall: (
    serverId: number,
    items: FicsitInstallItem[],
    transferId: string
  ) => invoke<void>('ficsit_install', { serverId, items, transferId }),

  // --- Marketplaces de mods multi-sources (Thunderstore / Factorio / uMod) ---
  /** Recherche / tri paginé dans le catalogue d'une source (family = jeu, pour Thunderstore). */
  modsSearch: (source: string, family: string, query: string, order: string, page: number) =>
    invoke<MarketModPage>('mods_search', { source, family, query, order, page }),
  /** Versions d'un mod d'une source (référence source-spécifique). */
  modsVersions: (source: string, reference: string) =>
    invoke<MarketModVersion[]>('mods_versions', { source, reference }),
  /** Installe un lot de mods d'une source (fond, un seul redémarrage) : suivi via mods://install-progress. */
  modsInstall: (
    serverId: number,
    source: string,
    items: ModInstallItem[],
    transferId: string
  ) => invoke<void>('mods_install', { serverId, source, items, transferId }),
  /** Mods installés d'une source, avec leur état activé. */
  modsInstalled: (source: string, serverId: number) =>
    invoke<MarketInstalledMod[]>('mods_installed', { source, serverId }),
  /** Active/désactive un mod installé (effet au prochain redémarrage). */
  modsSetEnabled: (source: string, serverId: number, reference: string, enabled: boolean) =>
    invoke<void>('mods_set_enabled', { source, serverId, reference, enabled }),
  /** Supprime un mod installé (retire de la liste + efface le zip). */
  modsRemove: (source: string, serverId: number, reference: string) =>
    invoke<void>('mods_remove', { source, serverId, reference }),

  // --- Réglages par jeu (Paramètres → Jeux) ---
  getGameSettings: () => invoke<GameSettings>('get_game_settings'),
  setGameSettings: (settings: GameSettings) => invoke<void>('set_game_settings', { settings }),
  /** Token de download factorio.com (trousseau OS). */
  setFactorioToken: (token: string) => invoke<void>('set_factorio_token', { token }),
  hasFactorioToken: () => invoke<boolean>('has_factorio_token'),
  clearFactorioToken: () => invoke<void>('clear_factorio_token'),

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

  /**
   * Joueurs déjà venus (Minecraft Java) : page recherchée, filtrée, triée, avec les statistiques
   * de l'ensemble. La liste complète est gardée une minute côté Rust ; `refresh` la relit.
   */
  knownPlayers: (serverId: number, query: PlayersQuery, refresh = false) =>
    demo(invoke<PlayersPage>('known_players', { serverId, query, refresh }), (page) => ({
      ...page,
      players: page.players.map(fakeKnownPlayer),
      stats: { ...page.stats, top_play_time: page.stats.top_play_time.map(fakeKnownPlayer) },
    })),

  /** Fiche d'un joueur (statistiques, état, dates de venue). */
  playerProfile: (serverId: number, uuid: string) =>
    demo(invoke<PlayerProfile>('player_profile', { serverId, uuid: realUuid(uuid) }), (p) => ({
      ...p,
      uuid: fakeUuid(p.uuid),
      name: p.name ? fakePlayer(p.name) : p.name,
    })),

  /** Inventaire d'un joueur, en lecture seule. */
  playerInventory: (serverId: number, uuid: string) =>
    demo(invoke<PlayerInventory>('player_inventory', { serverId, uuid: realUuid(uuid) }), (inv) => ({
      ...inv,
      hotbar: inv.hotbar.map(fakeItem),
      main: inv.main.map(fakeItem),
      armor: inv.armor.map(fakeItem),
      offhand: fakeItem(inv.offhand),
    })),

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
  // Transferts (suivis via l'event sftp://progress, corrélés par transferId) — la commande rend la
  // main aussitôt ; ne pas attendre la fin sur la promesse.
  sftpUpload: (serverId: number, localPath: string, remoteDir: string, transferId: string) =>
    invoke<void>('sftp_upload', { serverId, localPath, remoteDir, transferId }),
  sftpDownload: (serverId: number, remotePath: string, localPath: string, transferId: string) =>
    invoke<void>('sftp_download', { serverId, remotePath, localPath, transferId }),
  sftpDownloadZip: (serverId: number, paths: string[], localZip: string, transferId: string) =>
    invoke<void>('sftp_download_zip', { serverId, paths, localZip, transferId }),
  // Archives (lecture seule) : .zip / .tar / .tar.gz / .gz
  sftpArchiveList: (serverId: number, path: string) =>
    invoke<ArchiveEntry[]>('sftp_archive_list', { serverId, path }),
  sftpArchiveReadText: (serverId: number, path: string, entry: string) =>
    invoke<string>('sftp_archive_read_text', { serverId, path, entry }),
  sftpGzText: (serverId: number, path: string) =>
    invoke<string>('sftp_gz_text', { serverId, path }),
  sftpReadDataUri: (serverId: number, path: string) =>
    invoke<string>('sftp_read_data_uri', { serverId, path }),
  sftpNbtTree: (serverId: number, path: string) =>
    invoke<NbtNode>('sftp_nbt_tree', { serverId, path }),
  sftpInspectRegion: (serverId: number, path: string) =>
    invoke<string>('sftp_inspect_region', { serverId, path }),
  sftpRegionChunks: (serverId: number, path: string) =>
    invoke<RegionChunk[]>('sftp_region_chunks', { serverId, path }),
  sftpRegionChunkTree: (serverId: number, path: string, x: number, z: number) =>
    invoke<NbtNode>('sftp_region_chunk_tree', { serverId, path, x, z }),
  sftpNbtSnbt: (serverId: number, path: string) =>
    invoke<string>('sftp_nbt_snbt', { serverId, path }),
  sftpRegionChunkSnbt: (serverId: number, path: string, x: number, z: number) =>
    invoke<string>('sftp_region_chunk_snbt', { serverId, path, x, z }),
  sftpExtractEntry: (serverId: number, path: string, entry: string, localPath: string) =>
    invoke<void>('sftp_extract_entry', { serverId, path, entry, localPath }),
  sftpDisconnect: (serverId: number) => invoke<void>('sftp_disconnect', { serverId }),
  /** Vérifie le mot de passe SFTP (session ouverte) puis l'enregistre dans le stockage sécurisé. */
  sftpSetPassword: (serverId: number, password: string) =>
    invoke<void>('sftp_set_password', { serverId, password }),
  sftpForgetPassword: (serverId: number) => invoke<void>('sftp_forget_password', { serverId }),
  sftpHasPassword: (serverId: number) => invoke<boolean>('sftp_has_password', { serverId }),
  /** Identité du serveur SFTP : empreinte mémorisée et, après un refus, celle présentée. */
  sftpHostKey: (serverId: number) => invoke<SftpHostKey>('sftp_host_key', { serverId }),

  // --- Tâches planifiées du panel MineStrator (exécutées par l'hébergeur) ---
  panelSchedules: (serverId: number) =>
    demo(invoke<PanelSchedule[]>('panel_schedules', { serverId }), (list) =>
      list.map((s) => ({
        ...s,
        name: fakeText(s.name),
        tasks: s.tasks.map((x) => ({ ...x, payload: x.action === 'command' ? fakeText(x.payload) : x.payload })),
      })),
    ),
  panelScheduleCreate: (serverId: number, input: PanelScheduleInput) =>
    invoke<void>('panel_schedule_create', { serverId, input }),
  panelScheduleUpdate: (serverId: number, scheduleId: number, input: PanelScheduleInput) =>
    invoke<void>('panel_schedule_update', { serverId, scheduleId, input }),
  panelScheduleDelete: (serverId: number, scheduleId: number) =>
    invoke<void>('panel_schedule_delete', { serverId, scheduleId }),
  panelTaskAdd: (serverId: number, scheduleId: number, input: PanelTaskInput) =>
    invoke<void>('panel_task_add', { serverId, scheduleId, input }),
  panelTaskUpdate: (serverId: number, scheduleId: number, taskId: number, input: PanelTaskInput) =>
    invoke<void>('panel_task_update', { serverId, scheduleId, taskId, input }),
  panelTaskDelete: (serverId: number, scheduleId: number, taskId: number) =>
    invoke<void>('panel_task_delete', { serverId, scheduleId, taskId }),
  /** Nouvel ordre des actions (identifiants dans l'ordre d'exécution voulu). */
  panelTasksReorder: (serverId: number, scheduleId: number, taskIds: number[]) =>
    invoke<void>('panel_tasks_reorder', { serverId, scheduleId, taskIds }),
  /** Accepte l'empreinte présentée par le serveur (exactement celle affichée à l'utilisateur). */
  sftpTrustHostKey: (serverId: number, fingerprint: string) =>
    invoke<void>('sftp_trust_host_key', { serverId, fingerprint }),
  /** Exporte un texte vers un service de paste public ; renvoie l'URL. */
  pasteUpload: (service: string, content: string) =>
    invoke<string>('paste_upload', { service, content }),

  /** Réduit l'application dans le tray (elle continue en tâche de fond). */
  hideToTray: () => invoke<void>('hide_to_tray'),
  /** Quitte complètement l'application. */
  quitApp: () => invoke<void>('quit_app'),

  /** Journaux récents anonymisés (rapport de bug). Chaîne vide s'il n'y en a aucun. */
  exportLogs: () => invoke<string>('export_logs'),
  /** Ouvre le dossier des journaux dans le gestionnaire de fichiers. */
  openLogsDir: () => invoke<void>('open_logs_dir'),

  // --- Service d'arrière-plan (supervision machine, voir docs/SERVICE.md) ---
  /** État du service (OS + IPC) et qui supervise. Sans privilège. */
  serviceOverview: () => demo(invoke<ServiceOverview>('service_overview'), fakeServiceOverview),
  /** Active le service (élévation système si besoin) puis lui transmet la clé par IPC. */
  serviceEnable: () => demo(invoke<ServiceOverview>('service_enable'), fakeServiceOverview),
  /** Le service oublie la clé et cesse de superviser ; l'app reprend la main. */
  serviceDisable: () => demo(invoke<ServiceOverview>('service_disable'), fakeServiceOverview),
  /** Désinstalle le service (élévation) ; `purge` efface aussi son historique. */
  serviceUninstall: (purge: boolean) =>
    demo(invoke<ServiceOverview>('service_uninstall', { purge }), fakeServiceOverview),
  /** Redémarre le service via son gestionnaire. */
  serviceRestart: () => invoke<void>('service_restart'),
  /** Met à jour le service (élévation) : sa copie protégée ne suit pas les mises à jour de l'app. */
  serviceUpdate: () => demo(invoke<ServiceOverview>('service_update'), fakeServiceOverview),
  /** Choix « supervision en arrière-plan » fait dans l'installeur (Windows) ; `true` une fois. */
  serviceTakePendingSetup: () => invoke<boolean>('service_take_pending_setup'),
  /** Alertes récentes du service (survenues app fermée), sans notification. */
  serviceAlerts: (sinceSecs: number) => invoke<Alert[]>('service_alerts', { sinceSecs }),

  // --- Automatisations, disponibilité, alertes externes (exécutées par l'app ou le service) ---
  /** Redémarrage automatique et tâches planifiées d'un serveur, pour le compte courant. */
  serverAutomations: (serverId: number) =>
    invoke<ServerAutomations>('server_automations', { serverId }),
  /** Active ou règle le redémarrage automatique après crash. */
  setAutoRestart: (serverId: number, enabled: boolean, maxPerHour: number) =>
    invoke<RestartRule>('set_auto_restart', { serverId, enabled, maxPerHour }),
  /** Crée (`id` vide) ou modifie une tâche planifiée ; le cœur valide et attribue id/compte. */
  saveScheduledTask: (task: ScheduledTask) => invoke<TaskInfo>('save_scheduled_task', { task }),
  deleteScheduledTask: (taskId: string) => invoke<void>('delete_scheduled_task', { taskId }),
  /** Journal des exécutions d'un serveur (app + service), récents d'abord. */
  automationRuns: (serverId: number, limit = 50) =>
    demo(invoke<AutomationRun[]>('automation_runs', { serverId, limit }), (runs) =>
      runs.map((r) => ({
        ...r,
        message: fakeText(r.message),
        details: r.details ? fakeText(r.details) : r.details,
      })),
    ),
  /** Disponibilité 24 h / 7 j / 30 j et coupures, d'après la surveillance de l'app/du service. */
  uptimeReport: (serverId: number) => invoke<UptimeReport>('uptime_report', { serverId }),
  /** Destinations d'alertes : jamais l'URL (secret), seulement sa présence. */
  listWebhooks: () => invoke<WebhookInfo[]>('list_webhooks'),
  /** Crée ou modifie une destination. `url` : nouvelle URL (null = conserver l'actuelle). */
  saveWebhook: (destination: WebhookDestination, url: string | null) =>
    invoke<WebhookInfo>('save_webhook', { destination, url }),
  deleteWebhook: (id: string) => invoke<void>('delete_webhook', { id }),
  /** Envoie un message de test à l'URL saisie, ou à celle enregistrée pour `id`. */
  testWebhook: (kind: WebhookKind, url: string | null, id: string | null) =>
    invoke<void>('test_webhook', { kind, url, id }),

  /**
   * Déconnexion : retire le compte actif (clé et mots de passe SFTP). Renvoie le compte qui prend
   * la relève, `null` s'il n'en reste aucun (retour à l'écran de connexion).
   */
  logout: () => demo(invoke<Account | null>('logout'), (a) => (a ? fakeAccount(a) : a)),
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
  // Service d'arrière-plan : sous-cas traduit dans l'espace de noms dédié `service.errors`.
  if (value.kind === 'service' && value.code) {
    const key = `service.errors.${value.code}`;
    const text = t(key);
    if (text !== key) return text;
  }
  // Typé par le discriminant généré depuis Rust : un `kind` renommé ou retiré casse le typecheck.
  const known: Partial<Record<AppErrorKind, string>> = {
    unauthorized: 'errors.unauthorized',
    forbidden: 'errors.forbidden',
    rate_limited: 'errors.rate_limited',
    network: 'errors.network',
    no_key: 'errors.no_key',
    sftp_auth: 'errors.sftp_auth',
    sftp_password_required: 'errors.sftp_password_required',
    sftp_host_key_changed: 'errors.sftp_host_key_changed',
    account_exists: 'errors.account_exists',
    account_not_found: 'errors.account_not_found',
  };
  const key = known[value.kind];
  return key ? t(key) : value.message;
}
