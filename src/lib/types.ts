/** Miroirs TypeScript des structures renvoyées par le cœur Rust. */

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
  pro: boolean;
}

export interface ServerListItem {
  id: number;
  name: string;
  mybox_id: number;
  egg_name: string;
  egg_icon: string | null;
  address: string;
  /** `active` | `hibernation` | `disabled` | `suspended` | `expired` */
  status: string;
  owner: boolean;
  bedrock: boolean;
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
  /** CPU total en centièmes de cœur (300 = 3 cœurs = 300 %). */
  limit: number;
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

export type PowerAction = 'start' | 'restart' | 'restart10' | 'stop' | 'stop10' | 'kill';

export interface McpConfig {
  enabled: boolean;
  allow_writes: boolean;
}

// --- Copilote (agent LLM multi-fournisseur) ---
export type LlmProvider = 'anthropic' | 'openai_compatible' | 'local_cli';
export type Autonomy = 'suggest_only' | 'apply_on_confirm' | 'auto_safe';

export interface CopilotConfig {
  enabled: boolean;
  provider: LlmProvider;
  /** URL de base LLM ; vide = défaut du fournisseur. */
  base_url: string;
  model: string;
  /** Commande de l'agent CLI local (fournisseur local_cli), ex. « claude ». */
  cli_command: string;
  /** Arguments CLI (le prompt passe par stdin), ex. ["-p"]. */
  cli_args: string[];
  /** Mode agent : branche notre MCP sur la CLI (lecture fichiers/console autonome). */
  cli_agentic: boolean;
  autonomy: Autonomy;
  on_crash: boolean;
  on_threshold: boolean;
  /** Déclenche sur une ligne console ERROR (via le monitor du superviseur). */
  on_error: boolean;
  /** Déclenche sur une ligne console WARN. */
  on_warn: boolean;
  /** Sur surcharge CPU/RAM prolongée, lance une analyse de performance Spark. */
  perf_on_overload: boolean;
  /** Seuil % (CPU ou RAM) de surcharge. */
  perf_overload_pct: number;
  /** Durée minimale (minutes) de surcharge continue avant de déclencher. */
  perf_overload_minutes: number;
  disabled_servers: number[];
  /** Autorise la recherche web (mode CLI/Claude Code : WebSearch/WebFetch). */
  web_search: boolean;
  /** Outils interdits à l'IA (noms MCP sans préfixe). Vide = tous autorisés. */
  disabled_tools: string[];
  /** Effort de raisonnement (natif `--effort` en CLI, indice de prompt en HTTP). */
  effort: 'low' | 'medium' | 'high';
  /** Agent CLI (quand provider = local_cli). */
  cli_agent: 'claude_code' | 'open_code' | 'gemini';
}

/** Disponibilité d'un agent CLI local (Réglages → Copilote). */
export interface CliStatus {
  agent: 'claude_code' | 'open_code' | 'gemini';
  command: string;
  available: boolean;
  version: string | null;
}

export interface PrivacyConfig {
  /** Anonymise les données sensibles envoyées aux agents IA (défaut activé). */
  redact_ai: boolean;
  /** Anonymise aussi l'affichage de la console. */
  redact_console: boolean;
}

export interface ProposedAction {
  tool: string;
  args: Record<string, unknown>;
  label: string;
  /** safe | caution | danger */
  risk: string;
}

export interface CopilotStarted {
  id: string;
  server_id: number;
  server_name: string;
  /** crash | cpu | ram | disk | error | warn | manual | selection | performance */
  trigger: string;
}

export interface CopilotProgress {
  id: string;
  phase: string;
}

export interface ChatReply {
  text: string;
  actions: ProposedAction[];
}

/** Fragment de réponse assistant émis en direct (streaming). `id` = session_id (id d'onglet). */
export interface ChatDelta {
  id: string;
  text: string;
}

export interface Diagnosis {
  id: string;
  server_id: number;
  server_name: string;
  /** crash | cpu | ram | disk | manual */
  trigger: string;
  /** warning | critical */
  severity: string;
  summary: string;
  cause: string;
  suggested_fix: string;
  actions: ProposedAction[];
  ts: number;
}

export interface SupervisorConfig {
  enabled: boolean;
  crash_detection: boolean;
  expiry_alerts: boolean;
  cpu_threshold: number;
  ram_threshold: number;
  disk_threshold: number;
  disabled_servers: number[];
}

export interface Alert {
  server_id: number;
  server_name: string;
  /** crash | cpu | ram | disk | expiry */
  kind: string;
  /** warning | critical */
  severity: string;
  message: string;
  ts: number;
}

export interface MetricSample {
  ts: number;
  cpu: number;
  mem: number;
  mem_limit: number;
  disk: number;
  state: string;
}

export type PlayerAction =
  | 'kick'
  | 'ban'
  | 'unban'
  | 'op_add'
  | 'op_remove'
  | 'whitelist_add'
  | 'whitelist_remove';

export interface SftpEntry {
  name: string;
  path: string;
  is_dir: boolean;
  size: number;
  modified: number | null;
}

// --- Marketplace (mods & plugins) ---
export type MarketKind = 'mods' | 'plugins';
export type MarketSource = 'modrinth' | 'curseforge' | 'spigot';

export interface MarketItem {
  id: string;
  slug: string;
  name: string;
  tag: string;
  downloads: number;
  icon_url: string | null;
  game_versions: string[];
  loaders: string[];
  source: string;
  premium: boolean;
  external_url: string | null;
}

export interface MarketPage {
  items: MarketItem[];
  total_hits: number | null;
}

export interface MarketVersion {
  id: string;
  version_number: string;
  name: string;
  game_versions: string[];
  loaders: string[];
  release_type: string;
  downloads: number | null;
  date_published: string;
  filename: string | null;
  file_size: number | null;
}

export interface InstalledItem {
  name: string;
  filename: string;
  version: string;
  enabled: boolean;
  loader: string;
}

// --- Filet de sécurité : backups (quotidiens auto) & snapshots (à la demande) ---
export interface Backup {
  id: number;
  size: number; // octets
  date: string; // « YYYY-MM-DD HH:MM:SS »
}

export interface Snapshot {
  id: number;
  name: string;
  kind: string;
  size: number;
  date: string;
  status: number; // 1 = prêt
  is_legacy: boolean;
  progress: number | null; // renseigné pendant une création en cours
  job_id: number | null;
}

// --- Events console (taggés par conn_id) ---
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

/** Discriminant d'erreur émis par `AppError` côté Rust. */
export type AppErrorKind =
  | 'unauthorized'
  | 'forbidden'
  | 'rate_limited'
  | 'no_key'
  | 'network'
  | 'unexpected'
  | 'keyring'
  | 'api';

export interface AppError {
  kind: AppErrorKind;
  message: string;
}
