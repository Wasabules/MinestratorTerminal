/**
 * Types miroirs des réponses du core (sous-ensemble mobile).
 * Repris de desktop/src/lib/types.ts — à factoriser dans un package partagé plus tard.
 */

export interface UserProfile {
  id: number;
  pseudo: string;
  mail: string;
  credits: number;
  mybox_count: number;
}

export interface MyBoxSummary {
  id: number;
  name: string;
  offer: string;
  owner: boolean;
  cpu: number;
  ram: number;
  disk: number;
  days_left: number;
  expired: boolean;
  suspended: boolean;
  trial: boolean;
  bonus: boolean;
  expires_at: string | null;
}

/** Capacités dérivées du jeu (miroir de `crate::games`, calculé en Rust). */
export interface GameCapabilities {
  family: string;
  console_autocomplete: string;
  log_format: string;
  players: boolean;
  mods: string;
  backups: boolean;
}

export interface ServerListItem {
  id: number;
  name: string;
  mybox_id: number;
  egg_name: string;
  egg_icon: string | null;
  /** URL CDN complète de la vignette du jeu (composée par le core). */
  egg_icon_url: string | null;
  /** URL CDN de la bannière large (~200 Kio : ne pas la charger sans l'afficher). */
  egg_header_url: string | null;
  address: string;
  /** active | hibernation | disabled | suspended | expired */
  status: string;
  owner: boolean;
  capabilities: GameCapabilities;
  /** Identifiant d'egg exact du catalogue de l'hébergeur. */
  egg_id: number | null;
  /** URL signée de téléchargement du dernier backup (jeton porteur : ne pas afficher en clair). */
  last_backup_url: string | null;
  /** Avertissement de l'hébergeur à afficher avant un redémarrage. */
  restart_warning: string | null;
  /** Avertissement à afficher avant un arrêt. */
  stop_warning: string | null;
  /**
   * Raison d'indisponibilité, sous forme de clé i18n du panel
   * (`PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`) — à traduire, jamais à afficher brut.
   */
  unavailable_reason: string | null;
  /** Fin d'abonnement du serveur. */
  expires_at: string | null;
  /** Référence courte à donner au support. */
  support_ref: string | null;
}

/** Joueurs en ligne d'un serveur, via l'appel groupé (`live_players_bulk`). */
export interface LivePlayers {
  server_id: number;
  current: number;
  limit: number;
}

/** Entrée du catalogue d'eggs de l'hébergeur (`/site/eggs`). */
export interface Egg {
  id: number;
  name: string;
  icon_url: string | null;
  header_url: string | null;
  cpu: string;
  ram: string;
  disk: string;
  official: boolean;
}


export interface ServersOverview {
  myboxes: MyBoxSummary[];
  servers: ServerListItem[];
}

/** Entrée du journal d'actions tenu par l'hébergeur (couvre aussi le panel web et le staff). */
export interface ServerAction {
  /** `YYYY-MM-DD HH:MM:SS.mmm`. */
  date: string;
  /** Adresse d'origine — utile pour repérer une action qu'on n'a pas faite soi-même. */
  ip: string;
  /** Action du staff de l'hébergeur plutôt que du titulaire. */
  staff: boolean;
  /** Identifiant d'action (`power.restart`…). */
  action: string;
}

export interface ServerDetails {
  id: number;
  name: string;
  hibernation: boolean;
  ws_url: string | null;
  ws_token: string | null;
  /** Journal d'actions, du plus récent au plus ancien. */
  history: ServerAction[];
}

export interface CpuLimits {
  dedicated: number;
  flexcore: number;
}
export interface LimitMb {
  limit: number;
}
export interface Players {
  current: number;
  limit: number;
  list: string[];
}
export interface LiveLight {
  status: string | null;
  cpu: CpuLimits;
  memory: LimitMb;
  disk: LimitMb;
  players: Players | null;
  version: string | null;
  motd: string | null;
  hostname: string | null;
}

export interface MetricSample {
  ts: number;
  cpu: number;
  mem: number;
  mem_limit: number;
  disk: number;
  state: string;
}

export interface SftpEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: number | null;
}

export interface UpdateInfo {
  version: string;
  notes: string;
  apk_url: string;
}

export interface Backup {
  id: number;
  size: number;
  date: string;
}
export interface Snapshot {
  id: number;
  name: string;
  kind: string;
  size: number;
  date: string;
}

export type PowerAction = "start" | "restart" | "stop" | "kill";
export type PlayerAction =
  | "kick"
  | "ban"
  | "unban"
  | "op_add"
  | "op_remove"
  | "whitelist_add"
  | "whitelist_remove";

// --- Events console (taggés par conn_id) ---------------------------------

export interface ConsoleOutput {
  conn_id: string;
  line: string;
}
export interface ConsoleStatus {
  conn_id: string;
  state: string;
}
export interface ConsoleConnection {
  conn_id: string;
  /** connecting | open | reconnecting | closed | hibernated | suspended (terminal) */
  phase: string;
}
export interface ConsoleStats {
  conn_id: string;
  cpu_absolute: number;
  memory_bytes: number;
  memory_limit_bytes: number;
  disk_bytes: number;
  uptime: number;
  state: string;
}

// --- Erreur typée émise par le core --------------------------------------

export type AppErrorKind =
  | "Unauthorized"
  | "Forbidden"
  | "RateLimited"
  | "NotFound"
  | "Network"
  | "Api"
  | "Unexpected"
  | string;

export interface AppError {
  kind: AppErrorKind;
  message: string;
}
