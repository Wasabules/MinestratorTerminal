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
}

export interface ServerListItem {
  id: number;
  name: string;
  mybox_id: number;
  egg_name: string;
  egg_icon: string | null;
  address: string;
  /** active | hibernation | disabled | suspended | expired */
  status: string;
  owner: boolean;
}

export interface ServersOverview {
  myboxes: MyBoxSummary[];
  servers: ServerListItem[];
}

export interface ServerDetails {
  id: number;
  name: string;
  hibernation: boolean;
  ws_url: string | null;
  ws_token: string | null;
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
  /** connecting | open | reconnecting | closed | hibernated */
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
