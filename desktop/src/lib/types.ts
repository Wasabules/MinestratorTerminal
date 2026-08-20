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
  trial: boolean;
  bonus: boolean;
  /** Date d'expiration exacte (`days_left` n'en est que l'arrondi). */
  expires_at: string | null;
}

/** Capacités dérivées du jeu (quelles vues/fonctions exposer). Miroir de `crate::games`. */
export interface GameCapabilities {
  /** `minecraft` | `satisfactory` | `generic` */
  family: string;
  /** `minecraft_java` | `minecraft_bedrock` | `none` */
  console_autocomplete: string;
  /** `minecraft` | `unreal` | `generic` — détection des niveaux de log (filtres + couleurs). */
  log_format: string;
  players: boolean;
  /** `minecraft` | `ficsit` | `none` */
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
  /** `active` | `hibernation` | `disabled` | `suspended` | `expired` */
  status: string;
  owner: boolean;
  bedrock: boolean;
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
  capabilities: GameCapabilities;
}

/** Joueurs en ligne d'un serveur, via l'appel groupé (`live_players_bulk`). */
export interface LivePlayers {
  server_id: number;
  current: number;
  limit: number;
}

/**
 * Notification de l'hébergeur (incident, crédits, changement d'offre). Livrée avec le profil :
 * il n'existe pas d'endpoint dédié accessible avec une clé du panel, ni de marquage « lu ».
 */
export interface UserNotification {
  id: string;
  /** Catégorie de l'hébergeur : `Information`, `Erreur`, `StratorAI`… */
  kind: string;
  /** Sévérité au vocabulaire du panel : `error` | `warning` | `success` | `primary`. */
  color: string;
  message: string;
  date: string;
  read: boolean;
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

/**
 * Entrée du journal d'audit d'un serveur (`GET /server/{id}/activity`). Couvre les actions
 * faites hors de l'app — panel web, staff de l'hébergeur — et nomme leur auteur.
 */
export interface ActivityEntry {
  id: number;
  /** `power.restart`, `console.command`, `sftp.write`… */
  action: string;
  /** Famille : `power`, `console`, `sftp`… */
  category: string;
  /** `success` ou l'échec renvoyé par l'hôte. */
  result: string;
  /** Nom de l'auteur, quand l'API le connaît. */
  actor: string | null;
  actor_type: string;
  /** Action du staff de l'hébergeur plutôt que du compte. */
  staff: boolean;
  ip: string | null;
  date: string;
  /** Détail lisible (ex. la commande console exécutée). */
  context: string | null;
}

export interface ActivityPage {
  items: ActivityEntry[];
  total: number;
  has_more: boolean;
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
  /** Utiliser le MCP officiel MineStrator (gestion) en plus de notre MCP local (SFTP + exclusifs). */
  use_official_mcp: boolean;
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

/** Une entrée à l'intérieur d'une archive (.zip/.tar/.tar.gz), ouverture en lecture seule. */
export interface ArchiveEntry {
  name: string;
  size: number;
  is_dir: boolean;
}

/**
 * Nœud d'un arbre NBT typé (inspecteur `.dat`/chunk, lecture seule). `tag` conserve le type NBT
 * exact : `Byte|Short|Int|Long|Float|Double|String|ByteArray|IntArray|LongArray|List|Compound`.
 */
export interface NbtNode {
  name?: string;
  tag: string;
  value?: string;
  len?: number;
  children?: NbtNode[];
}

/** Chunk généré d'une région `.mca` (coordonnées de chunk globales, taille, date, corruption). */
export interface RegionChunk {
  x: number;
  z: number;
  size: number;
  /** Dernière écriture (epoch secondes ; 0 si absente). */
  timestamp: number;
  corrupt: boolean;
}

/** Progression d'un transfert SFTP (event `sftp://progress`), pour le gestionnaire de transferts. */
export interface SftpProgress {
  id: string;
  name: string;
  direction: 'up' | 'down';
  done: number;
  total: number;
  status: 'active' | 'done' | 'error';
  error: string | null;
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

// --- Mods Satisfactory (ficsit.app / SMR) ---
export interface FicsitMod {
  id: string;
  mod_reference: string;
  name: string;
  short_description: string;
  downloads: number;
  logo: string;
}

export interface FicsitModPage {
  mods: FicsitMod[];
  count: number;
}

export interface FicsitTarget {
  target_name: string;
  link: string;
  hash: string;
  size: number;
}

export interface FicsitDep {
  mod_id: string;
  condition: string;
  optional: boolean;
}

export interface FicsitVersion {
  id: string;
  version: string;
  sml_version: string;
  size: number;
  hash: string;
  link: string;
  targets: FicsitTarget[];
  dependencies: FicsitDep[];
}

export interface SmlTarget {
  target_name: string;
  link: string;
}

export interface SmlVersion {
  id: string;
  version: string;
  satisfactory_version: number;
  targets: SmlTarget[];
}

export interface FicsitInstalledMod {
  reference: string;
  name: string;
  enabled: boolean;
}

// --- Marketplaces de mods multi-sources (Thunderstore / Factorio / uMod) — modèles normalisés ---
export interface MarketMod {
  reference: string;
  name: string;
  description: string;
  downloads: number;
  icon_url: string;
  /** `thunderstore` | `factorio` | `umod` */
  source: string;
}

export interface MarketModPage {
  mods: MarketMod[];
  count: number;
  has_more: boolean;
}

export interface MarketModVersion {
  version: string;
  game_version: string;
  dependencies: string[];
}

/** Un mod installé sur le serveur (normalisé), avec son état activé. */
export interface MarketInstalledMod {
  reference: string;
  name: string;
  enabled: boolean;
}

// --- Réglages par jeu (Paramètres → Jeux) ---
export interface FactorioSettings {
  username: string;
}
export interface GameSettings {
  factorio: FactorioSettings;
}

export interface ModInstallProgress {
  id: string;
  /** `resolving` | `downloading` | `stopping` | `uploading` | `restarting` | `done` | `error` */
  phase: string;
  mod_name: string;
  done: number;
  total: number;
  /** `active` | `done` | `error` */
  status: string;
  error: string | null;
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
