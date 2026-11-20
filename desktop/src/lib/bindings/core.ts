// Fichier GÉNÉRÉ depuis les structures Rust (crates/minestrator-core, feature `ts`) par ts-rs.
// Ne pas modifier à la main : changer le type Rust, puis lancer `cargo bindings`.
// `cargo bindings-check` (CI) échoue si ce fichier ne correspond plus au Rust.

// --- Erreurs (error.rs) ---

/**
 * Forme JSON d'une [`Error`] reçue par les frontends (type TypeScript `AppError`). La
 * sérialisation d'[`Error`] passe par cette structure : le type généré ne peut pas diverger.
 */
export type AppError = {
  kind: AppErrorKind,
  /**
   * Message lisible (français), affiché quand le frontend n'a pas de traduction pour `kind`.
   */
  message: string,
  /**
   * Sous-cas stable : pour une erreur `service` (`unreachable`, `forbidden`,
   * `elevation_cancelled`…), traduit par l'UI via `service.errors.*` ; pour une erreur `api`, le
   * code renvoyé par MineStrator (`API_MYBOX_NOT_ENOUGH_CPU`…), que l'UI peut traduire. Absent
   * du JSON pour les autres erreurs.
   */
  code?: string,
};

/**
 * Discriminant d'une [`Error`], tel qu'il part dans le champ `kind` du JSON (type TypeScript
 * `AppErrorKind`). Un variant d'[`Error`] ajouté doit être rattaché ici via [`Error::error_kind`].
 */
export type AppErrorKind = "unauthorized" | "forbidden" | "rate_limited" | "no_key" | "network" | "unexpected" | "keyring" | "api" | "sftp_auth" | "sftp_password_required" | "sftp_host_key_changed" | "account_exists" | "account_not_found" | "service" | "invalid";

// --- Compte, serveurs, journal (models.rs, games.rs) ---

export type UserProfile = {
  id: number,
  pseudo: string,
  mail: string,
  credits: number,
  mybox_count: number,
};

/**
 * Notification exposée aux frontends.
 */
export type UserNotification = {
  id: string,
  /**
   * Catégorie donnée par l'hébergeur (`Information`, `Erreur`, `StratorAI`…).
   */
  kind: string,
  /**
   * Sévérité au vocabulaire du panel : `error` | `warning` | `success` | `primary`.
   */
  color: string,
  message: string,
  date: string,
  read: boolean,
};

export type ServersOverview = {
  myboxes: Array<MyBoxSummary>,
  servers: Array<ServerListItem>,
};

export type MyBoxSummary = {
  id: number,
  name: string,
  offer: string,
  owner: boolean,
  cpu: number,
  ram: number,
  disk: number,
  days_left: number,
  expired: boolean,
  suspended: boolean,
  pro: boolean,
  trial: boolean,
  bonus: boolean,
  /**
   * Date d'expiration brute, pour un affichage exact (`days_left` reste l'arrondi).
   */
  expires_at: string | null,
};

export type ServerListItem = {
  id: number,
  name: string,
  mybox_id: number,
  egg_name: string,
  /**
   * Nom de fichier brut renvoyé par l'API (`minecraft.webp`) — conservé pour le debug.
   */
  egg_icon: string | null,
  /**
   * URL CDN complète de la vignette du jeu, prête à poser dans un `<img>`.
   */
  egg_icon_url: string | null,
  /**
   * URL CDN de la bannière large du jeu (~200 Kio : ne la charger que si elle est affichée).
   */
  egg_header_url: string | null,
  address: string,
  status: string,
  owner: boolean,
  bedrock: boolean,
  /**
   * Identifiant d'egg exact (catalogue `/site/eggs`), quand l'API le fournit.
   */
  egg_id: number | null,
  /**
   * URL signée de téléchargement du dernier backup automatique (`None` si aucun).
   */
  last_backup_url: string | null,
  /**
   * Avertissement à afficher avant un redémarrage (`None` si l'hébergeur n'en a pas défini).
   */
  restart_warning: string | null,
  /**
   * Avertissement à afficher avant un arrêt.
   */
  stop_warning: string | null,
  /**
   * Raison d'indisponibilité, sous forme de **clé i18n du panel**
   * (`PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`). Vide pour un serveur sain.
   */
  unavailable_reason: string | null,
  /**
   * Fin d'abonnement du serveur.
   */
  expires_at: string | null,
  /**
   * Référence courte à donner au support.
   */
  support_ref: string | null,
  /**
   * Capacités dérivées du jeu (quelles vues/fonctions exposer). Voir [`crate::games`].
   */
  capabilities: GameCapabilities,
};

/**
 * Ce qu'un serveur d'un jeu donné sait faire, du point de vue de l'app. Source unique : tout
 * (vues, console, IA) dérive de cette structure.
 */
export type GameCapabilities = {
  /**
   * Famille de jeu : `"minecraft"` | `"satisfactory"` | `"generic"`.
   */
  family: string,
  /**
   * Autocomplétion console : `"minecraft_java"` | `"minecraft_bedrock"` | `"none"`.
   */
  console_autocomplete: string,
  /**
   * Format des logs pour la détection des niveaux (filtres + coloration console) :
   * `"minecraft"` (`[…/ERROR]`, SEVERE) | `"unreal"` (Satisfactory : `LogX: Error/Warning/Display`)
   * | `"generic"` (ERROR/WARN/INFO insensible à la casse).
   */
  log_format: string,
  /**
   * Vue Joueurs + carte joueurs de l'Aperçu + commandes joueur de la console.
   */
  players: boolean,
  /**
   * Joueurs déjà venus (liste, temps de jeu, fiche, inventaire), lus par l'hébergeur dans le
   * dossier `playerdata/` du monde : Minecraft **Java** seulement (Bedrock range ses joueurs
   * dans la base du monde, que l'hébergeur ne lit pas).
   */
  player_data: boolean,
  /**
   * Marketplace de mods : `"minecraft"` (Modrinth/CF/Spigot via MineStrator) | `"ficsit"` | `"none"`.
   */
  mods: string,
  /**
   * Sauvegardes/snapshots (true partout aujourd'hui ; champ prévu pour le futur).
   */
  backups: boolean,
  /**
   * Vue Paramètres : réglages du serveur (`server.properties`) lus et modifiés via l'API, sans
   * SFTP. Minecraft (Java et Bedrock) : le seul jeu dont l'hôte expose ce fichier.
   */
  properties: boolean,
  /**
   * Commande console qui diffuse un message à tous les joueurs (`{message}` = le texte), pour
   * les annonces avant un redémarrage programmé. `null` : le jeu n'en a pas, aucune annonce.
   */
  broadcast: string | null,
};

/**
 * Compte de joueurs de plusieurs serveurs en **un seul appel**. Attention : contrairement à
 * `GET /server/{id}/live/light`, la réponse ne porte QUE les joueurs — ni CPU, ni disque, ni
 * statut. C'est une vue de liste, pas un remplaçant du `live_light` complet.
 */
export type LivePlayers = {
  server_id: number,
  current: number,
  limit: number,
};

/**
 * Entrée du catalogue d'eggs de l'hébergeur (37 au 2026-08-19) : la table de référence qui
 * relie un `id_egg` à un jeu, sans passer par des mots-clés dans un nom.
 */
export type Egg = {
  id: number,
  name: string,
  icon_url: string | null,
  header_url: string | null,
  /**
   * Ressources minimales recommandées (cpu en cœurs, ram/disk en Go).
   */
  cpu: string,
  ram: string,
  disk: string,
  official: boolean,
};

/**
 * Entrée du journal d'audit d'un serveur. Bien plus riche que le `history[]` de
 * `GET /server/{id}` (qui n'a que date/ip/is_staff/action) : elle nomme l'auteur, distingue
 * la catégorie et le résultat, et porte un contexte — la commande tapée, par exemple.
 */
export type ActivityEntry = {
  id: number,
  /**
   * Identifiant d'action : `power.restart`, `console.command`, `sftp.write`…
   */
  action: string,
  /**
   * Famille d'action : `power`, `console`, `sftp`…
   */
  category: string,
  /**
   * `success` ou l'échec renvoyé par l'hôte.
   */
  result: string,
  /**
   * Nom de l'auteur. L'API le fournit ici, contrairement au `history[]` du détail serveur.
   */
  actor: string | null,
  /**
   * `user`, et vraisemblablement d'autres valeurs côté hébergeur (système, API…).
   */
  actor_type: string,
  /**
   * Action du staff de l'hébergeur plutôt que du compte.
   */
  staff: boolean,
  ip: string | null,
  /**
   * `YYYY-MM-DD HH:MM:SS.mmm`.
   */
  date: string,
  /**
   * Détail lisible quand l'API en fournit un (ex. la commande console exécutée).
   * ⚠️ Peut contenir ce que l'utilisateur a tapé : à passer par [`crate::redact`] avant
   * tout envoi vers un LLM ou un service de paste.
   */
  context: string | null,
};

/**
 * Page de journal d'audit. `total` peut être très grand (2 173 entrées sur un serveur de test) :
 * l'appelant pagine avec `limit`/`offset`.
 */
export type ActivityPage = {
  items: Array<ActivityEntry>,
  total: number,
  has_more: boolean,
};

export type ServerDetails = {
  id: number,
  name: string,
  hibernation: boolean,
  ws_url: string | null,
  ws_token: string | null,
};

export type LiveLight = {
  status: string | null,
  cpu: CpuLimits,
  memory: LimitMb,
  disk: LimitMb,
  players: Players | null,
  version: string | null,
  motd: string | null,
  hostname: string | null,
};

export type CpuLimits = {
  dedicated: number,
  flexcore: number,
  limit: number,
};

export type LimitMb = {
  limit: number,
};

export type Players = {
  current: number,
  limit: number,
  list: Array<string>,
};

/**
 * État temps réel complet d'un serveur.
 */
export type ServerLive = {
  /**
   * `online` | `starting` | `stopping` | `offline`.
   */
  state: string,
  /**
   * Statut interne de l'hôte (`null` si éteint sans état remonté).
   */
  status: string | null,
  stats: LiveStats,
};

export type LiveStats = {
  /**
   * Absent si le serveur est éteint (de même pour `memory`, `disk`, `network`).
   */
  cpu: LiveCpu | null,
  memory: LiveUsage | null,
  disk: LiveUsage | null,
  network: LiveNetwork | null,
  /**
   * Présent seulement serveur `online`.
   */
  uptime: LiveUptime | null,
  /**
   * Présents seulement serveur `online`, et selon le jeu.
   */
  players: Players | null,
  version: string | null,
  motd: string | null,
  hostname: string | null,
};

/**
 * CPU en **centièmes de cœur** (100 = 1 cœur).
 */
export type LiveCpu = {
  /**
   * Consommation instantanée.
   */
  current: number,
  dedicated: number,
  /**
   * CPU partagé supplémentaire de l'offre.
   */
  flexcore: number,
  /**
   * Total disponible (dédié + FlexCore).
   */
  limit: number,
  percent: number,
  /**
   * La consommation dépasse le CPU dédié : le serveur puise dans le FlexCore.
   */
  is_bursting: boolean,
};

/**
 * RAM ou disque, en **Mo**.
 */
export type LiveUsage = {
  current: number,
  limit: number,
  percent: number,
};

/**
 * Compteurs réseau **cumulés** (octets) remontés par le daemon de l'hôte.
 */
export type LiveNetwork = {
  rx_bytes: number,
  tx_bytes: number,
};

export type LiveUptime = {
  total_seconds: number,
};

/**
 * Réglages du serveur (`server.properties` pour Minecraft), lus via l'API : ni SFTP ni droit de
 * fichier requis (permission `settings_view`). Modification une clé à la fois, effet au prochain
 * démarrage.
 */
export type ServerProperties = {
  /**
   * Type de réglages renvoyé par l'hôte (`server_properties`).
   */
  kind: string,
  /**
   * Clé → valeur, triées par clé ; valeurs en texte, telles qu'écrites dans le fichier.
   */
  properties: { [key in string]: string },
};

/**
 * Détail d'une MyBox : abonnement, ressources restantes, offre et serveurs. Les noms de champs
 * de l'API sont normalisés (`rename(deserialize)` : n'agit qu'à la lecture, le JSON exposé garde
 * nos noms ; l'offre porte à la fois `disk` et `disk_total`, d'où pas d'`alias`).
 */
export type MyBoxDetails = {
  mybox: MyBoxInfo,
  offer: MyBoxOffer,
  servers: Array<MyBoxServer>,
};

export type MyBoxInfo = {
  id: number,
  name: string,
  /**
   * Échéance brute (`AAAA-MM-JJ hh:mm:ss`).
   */
  expires_at: string,
  /**
   * Jours avant l'échéance (0 si expirée).
   */
  days_left: number,
  /**
   * Jours avant la **suppression** — seulement pour une MyBox expirée.
   */
  deletion_days: number | null,
  /**
   * Faux pour un sous-utilisateur.
   */
  owner: boolean,
  /**
   * Renouvellement automatique actif : l'échéance n'est pas une menace.
   */
  auto_renew: boolean,
  free: boolean,
  trial: boolean,
  pro: boolean,
  expired: boolean,
  suspended: boolean,
  /**
   * Cœurs CPU encore allouables à un serveur.
   */
  cpu_remaining: number,
  /**
   * RAM (Go) encore allouable.
   */
  ram_remaining: number,
};

/**
 * Offre de la MyBox (ressources totales).
 */
export type MyBoxOffer = {
  name: string,
  price: number,
  /**
   * Cœurs CPU.
   */
  cpu: number,
  /**
   * RAM (Go).
   */
  ram: number,
  /**
   * Disque total, extensions comprises (Go).
   */
  disk: number,
  /**
   * Nombre de bases de données autorisées.
   */
  databases: number,
  /**
   * Taille maximale par base (Go).
   */
  database_size: number,
  /**
   * Nombre maximal de serveurs **actifs** (`pods` ; les serveurs en pause n'en consomment pas).
   * Voir `docs/api/MYBOX.md`.
   */
  max_servers: number,
};

/**
 * Serveur d'une MyBox, avec les ressources qui lui sont allouées.
 */
export type MyBoxServer = {
  id: number,
  name: string,
  egg_name: string,
  /**
   * Cœurs CPU alloués.
   */
  cpu: number,
  /**
   * RAM allouée (Go).
   */
  ram: number,
  disabled: boolean,
  location: string,
};

/**
 * Un échantillon de métriques (renvoyé au frontend pour les graphes).
 *
 * `Deserialize` : l'historique collecté par le service d'arrière-plan transite par IPC.
 */
export type MetricSample = {
  ts: number,
  cpu: number,
  mem: number,
  mem_limit: number,
  disk: number,
  state: string,
};

// --- Joueurs déjà venus, fiche et inventaire (players.rs) ---

/**
 * Statut de modération d'un joueur (lu par l'hébergeur dans `ops.json`, `whitelist.json` et
 * `banned-players.json`).
 */
export type PlayerFlags = {
  op: boolean,
  whitelisted: boolean,
  banned: boolean,
};

/**
 * Joueur déjà venu sur le serveur.
 */
export type KnownPlayer = {
  uuid: string,
  /**
   * Pseudo résolu par l'hébergeur ; `null` s'il n'a pas pu l'être.
   */
  name: string | null,
  /**
   * Connecté au moment de la lecture (si le serveur sait le dire, cf. `online_known`).
   */
  online: boolean,
  /**
   * Temps de jeu **cumulé** depuis la première venue, en secondes.
   */
  play_time_secs: number | null,
  /**
   * Dernière venue (secondes Unix) : dernière sauvegarde du fichier du joueur.
   */
  last_seen: number | null,
  flags: PlayerFlags,
};

/**
 * Tri de la liste des joueurs.
 */
export type PlayerSort = "last_seen" | "play_time" | "name";

/**
 * Filtre de la liste des joueurs.
 */
export type PlayerFilter = "all" | "online" | "op" | "whitelisted" | "banned";

/**
 * Requête de l'UI sur la liste des joueurs.
 */
export type PlayersQuery = {
  /**
   * Sous-chaîne du pseudo (insensible à la casse) ou début d'UUID ; vide : tous.
   */
  search: string,
  sort: PlayerSort,
  filter: PlayerFilter,
  offset: number,
  /**
   * Taille de page (1 à 200).
   */
  limit: number,
};

/**
 * Petites statistiques sur l'ensemble des joueurs déjà venus (indépendantes de la recherche).
 */
export type PlayerStats = {
  /**
   * Joueurs uniques.
   */
  total: number,
  online: number,
  /**
   * Venus dans les dernières 24 h, 7 jours, 30 jours.
   */
  active_24h: number,
  active_7d: number,
  active_30d: number,
  /**
   * Temps de jeu cumulé de tous les joueurs, en secondes.
   */
  total_play_time_secs: number,
  op: number,
  whitelisted: number,
  banned: number,
  /**
   * Les joueurs au plus grand temps de jeu cumulé (5 au plus).
   */
  top_play_time: Array<KnownPlayer>,
};

/**
 * Page de la liste des joueurs, avec les statistiques de l'ensemble.
 */
export type PlayersPage = {
  /**
   * Joueurs correspondant à la recherche et au filtre.
   */
  total: number,
  offset: number,
  players: Array<KnownPlayer>,
  stats: PlayerStats,
  /**
   * Le dossier `playerdata` du monde a été trouvé (sinon : monde pas encore créé, ou introuvable).
   */
  playerdata_found: boolean,
  /**
   * D'autres mondes ont leur propre dossier `playerdata` (plugin multi-mondes) : seuls les
   * joueurs du monde principal sont listés.
   */
  other_worlds: boolean,
  /**
   * Le serveur sait dire qui est connecté (sinon `online` vaut toujours `false`).
   */
  online_known: boolean,
  /**
   * Plus de joueurs que l'app n'en lit : seuls les plus récemment venus sont listés.
   */
  truncated: boolean,
};

/**
 * Distance parcourue selon un mode de déplacement.
 */
export type PlayerDistance = {
  /**
   * Mode, parmi [`DISTANCE_KINDS`] (`walk`, `sprint`, `fly`…).
   */
  kind: string,
  /**
   * En blocs (mètres).
   */
  blocks: number,
};

/**
 * Fiche d'un joueur. Tout est facultatif : l'hébergeur ne renvoie que ce que les fichiers du
 * joueur contiennent (une statistique jamais incrémentée est absente).
 */
export type PlayerProfile = {
  uuid: string,
  name: string | null,
  flags: PlayerFlags,
  /**
   * Première venue, dernière connexion, dernière venue (secondes Unix).
   */
  first_played: number | null,
  last_login: number | null,
  last_seen: number | null,
  /**
   * Temps de jeu cumulé, en secondes.
   */
  play_time_secs: number | null,
  sessions: number | null,
  deaths: number | null,
  mob_kills: number | null,
  player_kills: number | null,
  /**
   * `survival` | `creative` | `adventure` | `spectator`.
   */
  game_mode: string | null,
  /**
   * Points de vie (2 par cœur) ; `max_health` absent = 20.
   */
  health: number | null,
  max_health: number | null,
  /**
   * Faim, de 0 à 20.
   */
  food_level: number | null,
  xp_level: number | null,
  /**
   * Distances parcourues, de la plus grande à la plus petite (nulles exclues).
   */
  distances: Array<PlayerDistance>,
};

/**
 * Enchantement d'un objet.
 */
export type ItemEnchantment = {
  /**
   * Identifiant sans l'espace de noms `minecraft:` (`sharpness`).
   */
  id: string,
  /**
   * Nom lisible (`Sharpness`).
   */
  name: string,
  level: number,
};

/**
 * Objet d'un emplacement d'inventaire.
 */
export type InventoryItem = {
  /**
   * Identifiant, sans `minecraft:` pour les objets vanilla (`diamond_sword`, `create:wrench`).
   */
  id: string,
  /**
   * Nom personnalisé (renommé à l'enclume) ou, à défaut, nom tiré de l'identifiant.
   */
  name: string,
  custom_name: boolean,
  count: number,
  enchantments: Array<ItemEnchantment>,
  /**
   * Usure restante, de 0 (cassé) à 1 ; `null` si l'objet n'est pas abîmé ou pas usable.
   */
  durability: number | null,
  /**
   * Icône (objets vanilla seulement).
   */
  icon_url: string | null,
};

/**
 * Inventaire d'un joueur, lu dans son fichier `playerdata`.
 */
export type PlayerInventory = {
  /**
   * Barre d'action (9 emplacements).
   */
  hotbar: Array<InventoryItem | null>,
  /**
   * Sac (27 emplacements, trois rangées de neuf).
   */
  main: Array<InventoryItem | null>,
  /**
   * Casque, plastron, jambières, bottes.
   */
  armor: Array<InventoryItem | null>,
  offhand: InventoryItem | null,
  /**
   * Le joueur est connecté : le fichier date de la dernière sauvegarde du monde, l'inventaire
   * réel peut avoir changé depuis. `null` : l'hébergeur ne sait pas.
   */
  online: boolean | null,
  /**
   * Date de la sauvegarde lue (secondes Unix).
   */
  saved_at: number | null,
  /**
   * Version des données du jeu (`DataVersion`).
   */
  data_version: number | null,
};

// --- Historique croisé local + hébergeur (history.rs) ---

/**
 * Historique fusionné d'un serveur sur `[from, to]`.
 */
export type MetricsHistory = {
  server_id: number,
  from: number,
  to: number,
  /**
   * Pas des points (s) : la tranche de regroupement, sinon le pas de mesure le plus large
   * présent. Un écart nettement supérieur entre deux points est un trou de données.
   */
  step_s: number,
  /**
   * Points chronologiques, origine comprise.
   */
  points: Array<HistoryPoint>,
  /**
   * Joueurs connectés (hébergeur seulement), même regroupement (maximum par tranche).
   */
  players: Array<PlayersPoint>,
  /**
   * Temps couvert par la surveillance locale dans la fenêtre (s).
   */
  local_s: number,
  /**
   * Temps comblé par les mesures de l'hébergeur (s).
   */
  host_s: number,
  /**
   * Les statistiques de l'hébergeur ont pu être lues (sinon : local seul, ou cache partiel).
   */
  host_ok: boolean,
};

/**
 * Un point de l'historique fusionné (unités du local).
 */
export type HistoryPoint = {
  ts: number,
  /**
   * CPU en % d'un cœur (100 = un cœur).
   */
  cpu: number,
  /**
   * RAM, en octets.
   */
  mem: number,
  /**
   * Limite de RAM, en octets (0 si inconnue).
   */
  mem_limit: number,
  /**
   * Disque, en octets.
   */
  disk: number,
  /**
   * État Wings du point local ; `running` pour une mesure de l'hébergeur.
   */
  state: string,
  source: HistorySource,
};

/**
 * Origine d'un point d'historique.
 */
export type HistorySource = "local" | "host";

/**
 * Joueurs connectés (statistiques de l'hébergeur).
 */
export type PlayersPoint = {
  ts: number,
  count: number,
};

// --- Gestion des MyBox (mybox.rs) ---

/**
 * Une ressource allouable de la MyBox. CPU : **cœurs** ; RAM : **Go**.
 */
export type ResourceUsage = {
  /**
   * Total de l'offre, options comprises.
   */
  total: number,
  /**
   * Somme des allocations des serveurs **actifs** (les serveurs en pause ne comptent pas).
   */
  allocated: number,
  /**
   * Encore allouable à un serveur (jamais plus que ce que l'API annonce).
   */
  remaining: number,
};

/**
 * Disque de la MyBox : **partagé** par tous ses serveurs, rien n'est réservé par serveur.
 */
export type MyBoxDisk = {
  /**
   * Capacité totale (Go), options comprises.
   */
  total_gb: number,
  /**
   * Occupation actuelle de tous les serveurs (Mo) ; `None` si l'API ne l'a pas fournie.
   */
  used_mb: number | null,
};

/**
 * Emplacements de serveurs (`pods`). `0` comme maximum = pas de limite connue (MyBox Pro).
 */
export type MyBoxSlots = {
  max_active: number,
  active: number,
  max_paused: number,
  paused: number,
};

/**
 * Serveur d'une MyBox, avec ce qui lui est alloué.
 */
export type MyBoxServerSlot = {
  id: number,
  name: string,
  egg_id: number,
  egg_name: string,
  /**
   * Cœurs alloués.
   */
  cpu: number,
  /**
   * Go de RAM alloués.
   */
  ram: number,
  /**
   * En pause (« désactivé » dans l'API) : ne consomme ni CPU ni RAM, ses fichiers sont gardés.
   */
  paused: boolean,
  expired: boolean,
  /**
   * Suspendu : l'API le signale aussi pour **tout** serveur en pause.
   */
  suspended: boolean,
  /**
   * Ville de la localisation (`Toulouse`…).
   */
  location: string,
  country_iso: string,
  /**
   * Position dans la MyBox (ordre du panel).
   */
  order: number,
  /**
   * Référence courte à donner au support.
   */
  support_ref: string,
};

/**
 * Capacité d'une MyBox : ressources totales, allouées et restantes, emplacements, serveurs.
 */
export type MyBoxCapacity = {
  id: number,
  name: string,
  /**
   * Nom de l'offre (`MyBox 32`…).
   */
  offer: string,
  /**
   * Faux pour un sous-utilisateur.
   */
  owner: boolean,
  /**
   * Permissions de sous-utilisateur sur la MyBox (`server_create`…), vides pour le propriétaire.
   */
  permissions: Array<string>,
  /**
   * MyBox Pro : ressources sur mesure, non bornées par l'offre.
   */
  pro: boolean,
  free: boolean,
  trial: boolean,
  expired: boolean,
  suspended: boolean,
  /**
   * Cœurs FlexCore : rafale partagée ajoutée à la limite de chaque serveur, **non allouable**.
   */
  flexcore: number,
  cpu: ResourceUsage,
  ram: ResourceUsage,
  disk: MyBoxDisk,
  slots: MyBoxSlots,
  /**
   * Serveurs actifs (dans l'ordre du panel), puis serveurs en pause.
   */
  servers: Array<MyBoxServerSlot>,
  /**
   * Ce qui empêche d'y créer un serveur maintenant (vide = possible) : `expired`, `suspended`,
   * `forbidden`, `slots_full`, `no_cpu`, `no_ram`.
   */
  blockers: Array<string>,
};

/**
 * Champ à renseigner à la création d'un serveur de ce jeu (variable d'environnement de l'egg).
 */
export type EggField = {
  /**
   * Clé envoyée à l'API (`STEAM_USER`…).
   */
  key: string,
  label: string,
  description: string,
};

/**
 * Jeu (egg) proposé à la création.
 */
export type CatalogEgg = {
  id: number,
  name: string,
  /**
   * `games`, `programming`, `databases`, `custom` (PimpMyBox), `experimental` ou `other`.
   */
  category: string,
  /**
   * Type expérimental : non testé par l'hébergeur, sans support garanti ; envoyé à part.
   */
  experimental: boolean,
  icon_url: string | null,
  /**
   * Ressources minimales recommandées : cœurs, Go de RAM, Go de disque (0 = inconnu).
   */
  cpu: number,
  ram: number,
  disk: number,
  /**
   * Disque nécessaire à l'installation (Go, 0 = inconnu).
   */
  install_disk: number,
  /**
   * Niveau de certification de l'hébergeur (1 à 3, 0 = aucun).
   */
  certification: number,
  required_fields: Array<EggField>,
};

/**
 * Localisation (datacenter) où créer un serveur.
 */
export type CatalogLocation = {
  id: number,
  city: string,
  country: string,
  country_iso: string,
  in_stock: boolean,
};

/**
 * Catalogue de création : jeux (officiels, personnalisés, expérimentaux) et localisations.
 */
export type ServerCatalog = {
  eggs: Array<CatalogEgg>,
  locations: Array<CatalogLocation>,
};

/**
 * Valeur d'un champ requis. `Debug` masque la valeur : ce peut être un mot de passe (Steam…).
 */
export type FieldValue = {
  key: string,
  value: string,
};

/**
 * Demande de création d'un serveur dans une MyBox.
 */
export type CreateServerInput = {
  mybox_id: number,
  name: string,
  /**
   * Identifiant du jeu dans le catalogue ([`CatalogEgg::id`]).
   */
  egg_id: number,
  /**
   * Le jeu vient du catalogue **expérimental** (identifiants distincts).
   */
  experimental: boolean,
  /**
   * Localisation ([`CatalogLocation::id`]) ; ignorée pour une MyBox Pro.
   */
  location_id: number | null,
  /**
   * Cœurs (entier ≥ 1).
   */
  cpu: number,
  /**
   * Go de RAM (entier ≥ 1).
   */
  ram: number,
  /**
   * Champs requis du jeu.
   */
  fields: Array<FieldValue>,
};

/**
 * Serveur créé.
 */
export type CreatedServer = {
  mybox_id: number,
  server_id: number,
  /**
   * Adresse de connexion attribuée (peut être vide le temps de l'installation).
   */
  ip: string,
};

/**
 * Résultat d'une mise en pause ou d'une reprise.
 */
export type PauseOutcome = {
  /**
   * Reprise asynchrone en cours (fichiers remis en place) : suivre `job_id` via `backup_job`.
   */
  resuming: boolean,
  job_id: number | null,
};

/**
 * Snapshot lancée avant une suppression.
 */
export type SnapshotJob = {
  job_id: number,
  snapshot_name: string,
  /**
   * Snapshots déjà présentes sur le compte, et limite.
   */
  current_count: number,
  limit: number,
};

/**
 * Avancement d'un job de sauvegarde (snapshot, reprise d'un serveur en pause).
 */
export type BackupJob = {
  /**
   * `pending`, `running`, `completed`, `failed` ou `cancelled`.
   */
  status: string,
  /**
   * Pourcentage (0 à 100).
   */
  progress: number,
  error: string | null,
};

/**
 * Serveur à placer (plan de placement).
 */
export type PlacementRequest = {
  /**
   * Libellé libre (« survie », « lobby »…), repris dans le plan.
   */
  label: string,
  /**
   * Cœurs.
   */
  cpu: number,
  /**
   * Go de RAM.
   */
  ram: number,
  /**
   * Disque libre souhaité dans la MyBox (Go, 0 = sans exigence). Le disque est partagé.
   */
  disk_gb: number,
};

/**
 * Où placer un serveur demandé.
 */
export type PlacementChoice = {
  /**
   * Position de la demande dans la liste reçue.
   */
  index: number,
  label: string,
  /**
   * MyBox retenue (`None` : aucune ne peut l'accueillir).
   */
  mybox_id: number | null,
  mybox_name: string | null,
  /**
   * Cœurs et Go encore libres dans cette MyBox **après** ce placement (et les précédents).
   */
  cpu_left: number,
  ram_left: number,
  /**
   * Pourquoi rien ne convient (par MyBox), ou remarques.
   */
  reasons: Array<string>,
};

/**
 * Plan de placement : proposition seulement, rien n'est modifié.
 */
export type PlacementPlan = {
  /**
   * `pack` (remplir au plus juste) ou `spread` (répartir la charge).
   */
  strategy: string,
  placed: number,
  unplaced: number,
  /**
   * Une entrée par demande, dans l'ordre reçu.
   */
  choices: Array<PlacementChoice>,
  /**
   * Pistes pour ce qui ne rentre pas (mises en pause qui libéreraient assez).
   */
  suggestions: Array<string>,
};

// --- Sauvegardes (models.rs) ---

/**
 * Un backup **quotidien automatique** d'un serveur (`GET /server/{id}/backups`, récents d'abord).
 * On ne peut que les lister et les restaurer (pas de création à la demande côté API).
 */
export type Backup = {
  id: number,
  /**
   * Taille en octets.
   */
  size: number,
  /**
   * Horodatage « YYYY-MM-DD HH:MM:SS ».
   */
  date: string,
};

/**
 * Un **snapshot** créé à la demande (`GET /user/{id}/snapshots`). Point de sauvegarde rapide —
 * le bon filet AVANT une correction risquée (il est plus rapide à produire qu'un backup).
 *
 * Désérialisé via [`SnapshotRaw`] : l'API (doc OpenAPI officielle) renvoie l'id sous forme de
 * **chaîne `job_{job_id}`** pendant une création, et la taille en **Ko**.
 */
export type Snapshot = {
  /**
   * Id du snapshot ; `None` tant qu'il est en cours de création (pas encore restaurable ni
   * supprimable) — `job_id` est alors renseigné.
   */
  id: number | null,
  name: string,
  /**
   * Type d'univers (« Minecraft »). Champ `type` de l'API, renommé (mot réservé).
   */
  kind: string,
  /**
   * Taille en **octets** (convertie depuis les Ko de l'API).
   */
  size: number,
  date: string,
  /**
   * État (doc OpenAPI) : 0 = en attente, 1 = disponible, 2 = en création.
   */
  status: number,
  is_legacy: boolean,
  /**
   * Présents pour une création EN COURS de traitement (permet le suivi par polling) ; absents
   * pour un snapshot déjà prêt.
   */
  progress: number | null,
  job_id: number | null,
};

// --- SFTP, archives, NBT, régions (models.rs, archive.rs, nbt.rs, lib.rs) ---

export type SftpEntry = {
  name: string,
  path: string,
  is_dir: boolean,
  size: number,
  modified: number | null,
};

/**
 * Identité du serveur SFTP d'un serveur, pour la confirmer après un changement d'empreinte.
 */
export type SftpHostKey = {
  /**
   * `hôte:port`.
   */
  host: string,
  /**
   * Empreinte mémorisée (`SHA256:…`), absente avant le premier contact.
   */
  known_fingerprint: string | null,
  known_algorithm: string | null,
  /**
   * Empreinte présentée par le serveur lors du dernier refus, à accepter ou non.
   */
  presented_fingerprint: string | null,
  presented_algorithm: string | null,
};

// --- Tâches planifiées du panel MineStrator (panel_schedules.rs) ---

/**
 * Expression cron en cinq champs, telle que l'API la renvoie et l'attend.
 */
export type PanelCron = {
  minute: string,
  hour: string,
  day_of_month: string,
  month: string,
  day_of_week: string,
};

/**
 * Une action d'une tâche planifiée.
 */
export type PanelTask = {
  id: number,
  /**
   * Ordre d'exécution (croissant).
   */
  sequence_id: number,
  /**
   * `command` ou `power`.
   */
  action: string,
  /**
   * Texte de la commande, ou action d'alimentation.
   */
  payload: string,
  /**
   * Délai en secondes après l'action précédente (ou le déclenchement, pour la première).
   */
  time_offset: number,
  continue_on_failure: boolean,
};

/**
 * Une tâche planifiée du panel.
 */
export type PanelSchedule = {
  id: number,
  name: string,
  cron: PanelCron,
  is_active: boolean,
  only_when_online: boolean,
  /**
   * Exécution en cours côté hébergeur.
   */
  is_processing: boolean,
  next_run_at: string | null,
  last_run_at: string | null,
  /**
   * Actions, triées par `sequence_id` à la lecture.
   */
  tasks: Array<PanelTask>,
};

/**
 * Saisie d'une tâche planifiée (création ou modification).
 */
export type PanelScheduleInput = {
  name: string,
  cron: PanelCron,
  is_active: boolean,
  only_when_online: boolean,
};

/**
 * Saisie d'une action (ajout ou modification).
 */
export type PanelTaskInput = {
  /**
   * `command` ou `power`.
   */
  action: string,
  payload: string,
  time_offset: number,
  continue_on_failure: boolean,
};

/**
 * Une entrée d'archive (pour l'UI, façon [`crate::models::SftpEntry`] mais interne à l'archive).
 */
export type ArchiveEntry = {
  name: string,
  size: number,
  is_dir: boolean,
};

/**
 * Nœud d'un arbre NBT typé, sérialisé vers l'UI (inspecteur repliable).
 *
 * Les champs vides sont **omis** du JSON (pas `null`) : ils sont donc optionnels (`?:`) côté
 * TypeScript (`ts(optional)`, que ts-rs ne déduit pas d'un `skip_serializing_if` sans `default`).
 */
export type NbtNode = {
  /**
   * Clé dans le compound parent ; absent pour la racine et les éléments de liste.
   */
  name?: string,
  /**
   * Type NBT : `Byte|Short|Int|Long|Float|Double|String|ByteArray|IntArray|LongArray|List|Compound`.
   */
  tag: string,
  /**
   * Valeur affichable (scalaires + aperçu des tableaux) ; absent pour List/Compound.
   */
  value?: string,
  /**
   * Nombre d'enfants (List/Compound) ou d'éléments (tableaux).
   */
  len?: number,
  /**
   * Sous-nœuds (List/Compound uniquement).
   */
  children?: Array<NbtNode>,
};

/**
 * Chunk généré d'une région (coordonnées GLOBALES, taille, date, corruption), pour la carte UI.
 */
export type RegionChunk = {
  x: number,
  z: number,
  size: number,
  /**
   * Dernière écriture (epoch secondes ; 0 si absente).
   */
  timestamp: number,
  corrupt: boolean,
};

// --- Fichiers côté serveur : compression, extraction, espace disque (server_files.rs) ---

/**
 * Résultat d'une compression côté serveur.
 */
export type CompressOutcome = {
  /**
   * Chemin SFTP absolu de l'archive.
   */
  archive_path: string,
  /**
   * `true` : la compression continue sur le serveur (suivi arrêté) ; l'archive n'est pas
   * encore complète.
   */
  still_running: boolean,
  /**
   * Fichiers compressés.
   */
  file_count: number,
  /**
   * Taille de l'archive (octets, lue par SFTP ; 0 si inconnue).
   */
  size: number,
  /**
   * Fichiers ignorés (archive partielle).
   */
  skipped: number,
};

/**
 * Résultat d'une extraction.
 */
export type ExtractOutcome = {
  /**
   * Dossier où l'archive a été extraite (chemin SFTP absolu).
   */
  destination: string,
  /**
   * `true` : extraite par le serveur ; `false` : repli par l'appareil (SFTP).
   */
  on_server: boolean,
  /**
   * Fichiers écrits (connu seulement pour le repli par l'appareil).
   */
  files: number | null,
  /**
   * Fichiers ignorés par le serveur (extraction partielle).
   */
  skipped: number,
};

/**
 * Ce qui occupe le plus de place dans un dossier, du plus gros au plus petit.
 */
export type DiskUsage = {
  /**
   * Dossier analysé (chemin SFTP absolu, `/` pour la racine).
   */
  path: string,
  /**
   * Taille totale du dossier (octets, récursif).
   */
  total_size: number,
  /**
   * Plus grosses entrées directes du dossier, triées par taille décroissante (bornées par
   * `limit` : la somme peut être inférieure au total).
   */
  items: Array<DiskUsageItem>,
};

/**
 * Une entrée de l'analyse du disque (taille récursive pour un dossier).
 */
export type DiskUsageItem = {
  /**
   * Chemin SFTP absolu.
   */
  path: string,
  name: string,
  is_dir: boolean,
  /**
   * Octets (récursif pour un dossier).
   */
  size: number,
};

/**
 * Espace disque d'un serveur : occupation comptée par l'hébergeur et limite de l'offre.
 */
export type DiskSpace = {
  /**
   * Occupation **comptée** (octets) : le disque brut, moins la franchise des fichiers de base
   * du jeu quand elle s'applique (`GET /server/{id}/disk/total`).
   */
  used_bytes: number,
  /**
   * Occupation brute du disque (octets).
   */
  raw_bytes: number,
  /**
   * Franchise des fichiers de base du jeu (octets), déduite si `allowance_applied`.
   */
  base_game_bytes: number,
  allowance_applied: boolean,
  /**
   * Limite du serveur (octets ; franchise comprise). 0 si inconnue.
   */
  limit_bytes: number,
  /**
   * Espace de la MyBox, partagé par ses serveurs (absent si indisponible).
   */
  mybox: MyBoxDiskSpace | null,
};

/**
 * Espace disque d'une MyBox : occupation de tous ses serveurs face au disque de l'offre.
 */
export type MyBoxDiskSpace = {
  used_bytes: number,
  /**
   * Disque de l'offre, extensions comprises (octets). 0 si inconnu.
   */
  limit_bytes: number,
};

// --- Events temps réel (events.rs) ---

export type ConsoleOutput = {
  conn_id: string,
  line: string,
};

export type ConsoleStatus = {
  conn_id: string,
  state: string,
};

export type ConsoleConnection = {
  conn_id: string,
  /**
   * connecting | open | reconnecting | closed | hibernated | suspended (terminal : serveur
   * suspendu ou expiré, plus de reconnexion)
   */
  phase: string,
};

export type ConsoleStats = {
  conn_id: string,
  cpu_absolute: number,
  memory_bytes: number,
  memory_limit_bytes: number,
  disk_bytes: number,
  uptime: number,
  state: string,
};

/**
 * Progression d'un transfert SFTP (upload / download / zip) ou d'une opération de fichiers sur le
 * serveur (compression, extraction), pour le gestionnaire de transferts.
 * `id` = identifiant du transfert (généré côté front pour corréler). `done`/`total` en octets
 * (`total = 0` : avancement indéterminé, `done` = octets déjà traités s'il est connu).
 */
export type SftpProgress = {
  id: string,
  /**
   * Nom lisible (fichier, ou nom du .zip pour un téléchargement groupé).
   */
  name: string,
  /**
   * `up` (téléversement) | `down` (téléchargement) | `server` (opération sur le serveur, sans
   * transfert : compression, extraction).
   */
  direction: string,
  done: number,
  total: number,
  /**
   * `active` | `done` | `error`.
   */
  status: string,
  /**
   * Message si `status = error`.
   */
  error: string | null,
  /**
   * Étape d'une opération en plusieurs temps (`None` pour un transfert simple) : `queued`
   * (en attente d'une autre compression), `compressing` (archive en cours de création sur le
   * serveur), `extracting` (extraction sur le serveur), `downloading`, `local_zip` (repli :
   * archive construite par l'appareil), `cleanup` (suppression de l'archive temporaire).
   */
  phase: string | null,
  /**
   * Fichiers déjà traités par le serveur (compression) ; 0 sinon.
   */
  files: number,
  /**
   * Fichiers ignorés par le serveur (archive ou extraction partielle), publié avec la fin.
   */
  skipped: number,
};

/**
 * Progression d'une installation de mod Satisfactory (ficsit → SFTP). `id` = id de transfert
 * (généré côté front). `phase` ∈ {resolving, downloading, stopping, uploading, restarting, done,
 * error}. `done`/`total` = nombre d'artefacts (mod + SML + dépendances).
 */
export type ModInstallProgress = {
  id: string,
  phase: string,
  mod_name: string,
  done: number,
  total: number,
  /**
   * `active` | `done` | `error`.
   */
  status: string,
  error: string | null,
};

/**
 * Alerte émise par le superviseur (crash, seuils, expiration…).
 *
 * `Deserialize` : le service d'arrière-plan transmet ses alertes à l'app par IPC
 * (voir [`crate::service`]).
 */
export type Alert = {
  /**
   * ID serveur, ou ID MyBox pour une alerte d'expiration.
   */
  server_id: number,
  server_name: string,
  /**
   * `crash` | `cpu` | `ram` | `disk` | `expiry` | `auto_restart` | `task`
   */
  kind: string,
  /**
   * `info` | `warning` | `critical`
   */
  severity: string,
  message: string,
  ts: number,
  /**
   * Détail joint, déjà anonymisé (`redact`) : dernières lignes de console au moment d'un crash,
   * erreur d'une tâche planifiée… Absent du JSON quand il n'y en a pas — une app ou un service
   * de version antérieure lit et écrit donc la même forme qu'avant.
   */
  details?: string,
};

/**
 * Émis au **lancement** d'un diagnostic (avant l'appel LLM) : permet à l'UI d'afficher un
 * indicateur « analyse en cours ». L'`id` correspond à celui du [`Diagnosis`] final.
 */
export type CopilotStarted = {
  id: string,
  server_id: number,
  server_name: string,
  /**
   * `crash` | `cpu` | `ram` | `disk` | `error` | `warn` | `manual` | `selection` | `performance`
   */
  trigger: string,
};

/**
 * Étape d'avancement d'une analyse en cours (pour le log/détail dans l'UI). `id` = celui du
 * [`CopilotStarted`]/[`Diagnosis`] correspondant.
 */
export type CopilotProgress = {
  id: string,
  phase: string,
};

/**
 * Fragment de texte de réponse de l'assistant, émis en direct (streaming). `id` = session_id
 * (id d'onglet). Le front accumule ces fragments dans la bulle en cours ; la réponse finale
 * (renvoyée par `chat_send`) reste la source d'autorité.
 */
export type ChatDelta = {
  id: string,
  text: string,
};

/**
 * Rapport de diagnostic produit par le Copilote (agent Claude) suite à un incident.
 */
export type Diagnosis = {
  /**
   * Identifiant unique (`{server_id}-{ts}`).
   */
  id: string,
  server_id: number,
  server_name: string,
  /**
   * `crash` | `cpu` | `ram` | `disk` | `manual`
   */
  trigger: string,
  /**
   * `warning` | `critical`
   */
  severity: string,
  /**
   * Résumé en une phrase.
   */
  summary: string,
  /**
   * Cause probable (markdown).
   */
  cause: string,
  /**
   * Correctif détaillé (markdown).
   */
  suggested_fix: string,
  /**
   * Actions concrètes proposées (0 à quelques-unes).
   */
  actions: Array<ProposedAction>,
  ts: number,
};

/**
 * Une action concrète proposée par le Copilote, applicable via la couche d'outils.
 * L'exécution dépend du niveau d'autonomie choisi (jamais exécutée en mode « suggérer »).
 */
export type ProposedAction = {
  /**
   * Nom d'outil (`power_action`, `send_command`, `write_file`, …).
   */
  tool: string,
  /**
   * Arguments de l'outil (inclut `server_id`).
   */
  args: JsonValue,
  /**
   * Description lisible de l'action.
   */
  label: string,
  /**
   * `safe` (réversible/sans perte) | `caution` (modifie une config) | `danger` (destructif).
   */
  risk: string,
};

export type JsonValue = number | string | boolean | Array<JsonValue> | { [key in string]: JsonValue } | null;

// --- Réglages persistés (supervisor.rs, mcp.rs, redact.rs, mods.rs) ---

/**
 * Réglages du superviseur (modifiables depuis l'UI, persistés).
 *
 * `#[serde(default)]` : un champ absent du `supervisor.json` (ajouté par une version ultérieure)
 * prend sa valeur par défaut, au lieu de rendre tout le fichier illisible — ce qui ramenait
 * silencieusement TOUS les réglages de l'utilisateur aux valeurs d'usine.
 */
export type SupervisorConfig = {
  enabled: boolean,
  crash_detection: boolean,
  expiry_alerts: boolean,
  cpu_threshold: number,
  ram_threshold: number,
  disk_threshold: number,
  /**
   * Serveurs exclus du monitoring.
   */
  disabled_servers: Array<number>,
};

/**
 * Réglages du serveur MCP (modifiables depuis l'UI, persistés dans `mcp.json`).
 */
export type McpConfig = {
  enabled: boolean,
  /**
   * Autorise les actions modifiantes (power, commandes, modération, écriture de fichiers).
   */
  allow_writes: boolean,
  /**
   * Passerelle vers le MCP officiel MineStrator : relaie ses outils (tous toolsets sauf
   * `files`) sur cette même connexion, la clé restant au trousseau. Désactivée par défaut (voir
   * `official_relay.rs`) ; la variable `MINESTRATOR_MCP_OFFICIAL` prime sur ce réglage.
   */
  official_gateway: boolean,
  /**
   * Groupes d'outils exposés (`core`, `files`, `console`…, voir `mcp_catalog.rs`) ; vide = tous.
   * L'argument `--toolsets=…` puis la variable `MINESTRATOR_MCP_TOOLSETS` priment.
   */
  toolsets: Array<string>,
  /**
   * Portée des écritures des agents : `all` (tout le compte, défaut) ou `builder` (mode
   * constructeur : seulement les serveurs créés pendant la session MCP et `allowed_servers`).
   * `MINESTRATOR_MCP_WRITE_SCOPE` prime sur ce réglage (voir `agent_guard.rs`).
   */
  write_scope: WriteScope,
  /**
   * Serveurs modifiables en mode constructeur en plus de ceux créés par la session.
   * `MINESTRATOR_MCP_ALLOWED_SERVERS` (identifiants séparés par des virgules) la remplace.
   */
  allowed_servers: Array<number>,
};

/**
 * Réglages de confidentialité (persistés dans `privacy.json`).
 */
export type PrivacyConfig = {
  /**
   * Masque les données sensibles AVANT de les envoyer aux agents IA (Copilote, Assistant,
   * clients MCP externes). Activé par défaut : rien de sensible ne part vers un LLM sans le vouloir.
   */
  redact_ai: boolean,
  /**
   * Masque aussi les données sensibles dans l'AFFICHAGE de la console (temps réel + préchargement).
   */
  redact_console: boolean,
};

/**
 * Réglages par jeu (partie **non-secrète**, persistée `game_settings.json`). Les secrets (token
 * Factorio) vont au trousseau (voir `secrets::store_game_secret`). Extensible : un champ par jeu.
 */
export type GameSettings = {
  factorio: FactorioSettings,
};

/**
 * Réglages Factorio : le `username` du compte factorio.com (le token de download va au trousseau).
 */
export type FactorioSettings = {
  username: string,
};

// --- Garde des agents MCP : journal, changesets, mode constructeur (agent_guard.rs) ---

/**
 * Portée des écritures des agents connectés au serveur MCP (réglage `write_scope` de
 * `mcp.json`, surchargé par `MINESTRATOR_MCP_WRITE_SCOPE`).
 */
export type WriteScope = "all" | "builder";

/**
 * Résultat d'un appel journalisé.
 */
export type ActionOutcome = "ok" | "error" | "denied";

/**
 * Un appel d'outil reçu par le serveur MCP, tel que journalisé.
 */
export type AgentAction = {
  id: number,
  /**
   * Début de l'appel (secondes Unix).
   */
  ts: number,
  /**
   * Session (et changeset) de l'appel : `mcp:…` ou `copilote:<serveur>:<jour>`.
   */
  session: string,
  /**
   * Nom du client MCP, s'il est connu.
   */
  client: string | null,
  tool: string,
  /**
   * Outil modifiant.
   */
  write: boolean,
  /**
   * Serveurs désignés par les arguments.
   */
  servers: Array<number>,
  /**
   * Arguments résumés et anonymisés (valeurs tronquées, contenus remplacés par leur taille).
   */
  args: JsonValue,
  outcome: ActionOutcome,
  /**
   * Motif court d'une erreur ou d'un refus (anonymisé).
   */
  detail: string | null,
  duration_ms: number,
  /**
   * Étape de changeset créée par l'appel, s'il y en a une.
   */
  changeset: string | null,
  step: number | null,
};

/**
 * Filtre du journal (tous les champs sont facultatifs).
 */
export type AgentActionQuery = {
  server_id: number | null,
  session: string | null,
  /**
   * Depuis (secondes Unix).
   */
  since: number | null,
  only_writes: boolean,
  /**
   * Lignes renvoyées (défaut 50, max 500).
   */
  limit: number | null,
};

/**
 * Origine d'un changeset.
 */
export type ChangeOrigin = "mcp" | "copilot" | "app";

/**
 * État d'une étape vis-à-vis de l'annulation.
 */
export type StepState = "active" | "undone" | "skipped";

/**
 * Une modification d'agent, dans son changeset.
 */
export type ChangeStep = {
  /**
   * Numéro d'ordre dans le changeset (1 = la plus ancienne).
   */
  seq: number,
  ts: number,
  tool: string,
  server_id: number | null,
  /**
   * Cible lisible (chemin, réglage…), vide si l'outil n'en a pas.
   */
  target: string,
  undoable: boolean,
  /**
   * Pourquoi l'étape n'est pas annulable, ou avertissement (annulation partielle).
   */
  note: string | null,
  /**
   * L'action a échoué en ayant modifié une partie de ses cibles.
   */
  failed: boolean,
  state: StepState,
  undone_at: number | null,
};

/**
 * Modifications d'une session d'agent.
 */
export type Changeset = {
  id: string,
  origin: ChangeOrigin,
  client: string | null,
  created: number,
  updated: number,
  servers: Array<number>,
  /**
   * Octets de copies gardées pour l'annulation.
   */
  bytes: number,
  steps: Array<ChangeStep>,
};

/**
 * Issue de l'annulation d'une étape.
 */
export type RollbackStatus = "undone" | "skipped" | "refused" | "failed";

export type RollbackStep = {
  seq: number,
  tool: string,
  target: string,
  server_id: number | null,
  status: RollbackStatus,
  message: string,
};

/**
 * Rapport d'annulation, étape par étape (de la plus récente à la plus ancienne).
 */
export type RollbackReport = {
  changeset: string,
  steps: Array<RollbackStep>,
  /**
   * Arrêtée avant la fin (étape refusée ou en échec) : les plus anciennes restent en place.
   */
  stopped: boolean,
};

// --- Copilote (copilot.rs, llm.rs, cli_agent.rs) ---

/**
 * Réglages du Copilote (persistés dans `copilot.json`).
 */
export type CopilotConfig = {
  enabled: boolean,
  provider: LlmProvider,
  /**
   * URL de base LLM ; vide = valeur par défaut du fournisseur.
   */
  base_url: string,
  model: string,
  /**
   * Commande de l'agent CLI local (fournisseur `LocalCli`), ex. `claude`.
   */
  cli_command: string,
  /**
   * Arguments de la commande CLI (le prompt est passé sur stdin), ex. `["-p"]`.
   */
  cli_args: Array<string>,
  /**
   * Mode agent : branche notre serveur MCP sur la CLI (Claude Code) pour qu'elle lise
   * fichiers/console/métriques elle-même et croise, au lieu d'un prompt pré-rempli.
   */
  cli_agentic: boolean,
  autonomy: Autonomy,
  on_crash: boolean,
  on_threshold: boolean,
  /**
   * Déclenche un diagnostic sur une ligne console classée ERROR (via le monitor).
   */
  on_error: boolean,
  /**
   * Déclenche un diagnostic sur une ligne console classée WARN (plus bruyant).
   */
  on_warn: boolean,
  /**
   * Sur **surcharge prolongée** CPU/RAM (pas un simple pic), lance une **analyse de
   * performance Spark** : rapports Spark + profiler, puis suggestions.
   */
  perf_on_overload: boolean,
  /**
   * Seuil (% CPU ou RAM) à partir duquel on considère le serveur « surchargé ».
   */
  perf_overload_pct: number,
  /**
   * Durée minimale (minutes) de surcharge CONTINUE avant de déclencher (évite les pics).
   */
  perf_overload_minutes: number,
  disabled_servers: Array<number>,
  /**
   * Autorise l'agent à faire des **recherches web** (mode CLI/Claude Code : outils
   * WebSearch/WebFetch). Sans effet en mode API HTTP (pas d'outil web). Défaut : activé.
   */
  web_search: boolean,
  /**
   * Outils **interdits** à l'IA (Copilote/Assistant) — noms MCP sans préfixe (ex.
   * `power_action`, `delete_path`). Vide = tous autorisés (selon lecture/écriture + autonomie).
   * Permet un contrôle FIN des permissions quelle que soit la source (API HTTP ou CLI).
   */
  disabled_tools: Array<string>,
  /**
   * Effort de raisonnement demandé au modèle.
   */
  effort: Effort,
  /**
   * Agent CLI utilisé quand `provider = LocalCli` (Claude Code / OpenCode / Gemini).
   */
  cli_agent: CliAgent,
  /**
   * Utiliser le **MCP officiel** MineStrator (gestion serveur) en plus de notre MCP local (SFTP +
   * outils exclusifs). Défaut activé : l'officiel, maintenu par l'hébergeur, est plus fiable pour
   * la gestion. Désactivé ⇒ le Copilote n'utilise que notre catalogue local complet.
   */
  use_official_mcp: boolean,
};

/**
 * Fournisseur LLM. Le transport `OpenaiCompatible` est volontairement générique
 * (URL de base configurable) pour couvrir tout service exposant l'API OpenAI.
 */
export type LlmProvider = "anthropic" | "openai_compatible" | "local_cli";

/**
 * Niveau d'autonomie du Copilote (choisi dans les réglages).
 */
export type Autonomy = "suggest_only" | "apply_on_confirm" | "auto_safe";

/**
 * Effort de raisonnement demandé au modèle. Mappé par fournisseur : niveau natif `--effort`
 * pour Claude Code (CLI), indice de prompt pour l'API HTTP.
 */
export type Effort = "low" | "medium" | "high";

/**
 * Agent CLI sélectionné. Le binaire concret vient de `cli_command` (surchargeable) ; l'enum
 * détermine flags, config MCP et parsing.
 */
export type CliAgent = "claude_code" | "open_code" | "gemini";

/**
 * Statut de disponibilité d'un agent CLI (exposé à l'UI Réglages → Copilote).
 */
export type CliStatus = {
  agent: CliAgent,
  /**
   * Binaire sondé (commande par défaut de l'agent).
   */
  command: string,
  /**
   * Présent et répondant à `--version` ?
   */
  available: boolean,
  /**
   * Version brute rapportée (1re ligne de sortie), si disponible.
   */
  version: string | null,
};

/**
 * Réponse d'un tour de chat : texte + actions proposées (vides en mode autonome, où l'agent agit).
 */
export type ChatReply = {
  text: string,
  actions: Array<ProposedAction>,
};

// --- Marketplace Minecraft (models.rs) ---

export type MarketItem = {
  /**
   * Identifiant natif de la source (slug Modrinth via `slug`, id numérique CF/Spigot).
   */
  id: string,
  slug: string,
  name: string,
  tag: string,
  downloads: number,
  icon_url: string | null,
  game_versions: Array<string>,
  loaders: Array<string>,
  source: string,
  /**
   * Payant/restreint → non installable via MineStrator (l'API renvoie 403).
   */
  premium: boolean,
  /**
   * Téléchargement externe (SpigotMC) → non installable non plus.
   */
  external: boolean,
  external_url: string | null,
};

export type MarketPage = {
  items: Array<MarketItem>,
  total_hits: number | null,
};

export type MarketVersion = {
  id: string,
  version_number: string,
  name: string,
  game_versions: Array<string>,
  loaders: Array<string>,
  release_type: string,
  downloads: number | null,
  date_published: string,
  filename: string | null,
  file_size: number | null,
};

export type InstalledItem = {
  name: string,
  filename: string,
  version: string,
  enabled: boolean,
  loader: string,
};

// --- Mises à jour des mods, plugins et datapacks (updates.rs) ---

/**
 * Type de contenu Minecraft géré par l'hébergeur.
 */
export type ContentKind = "mods" | "plugins" | "datapacks";

/**
 * Mise à jour applicable d'un élément installé.
 */
export type ContentUpdate = {
  kind: ContentKind,
  /**
   * Clé de l'élément installé : nom de fichier (mods, plugins) ou nom du datapack. Sert à
   * l'associer à la liste des installés et à demander son application.
   */
  key: string,
  name: string,
  /**
   * Désactivé sur le serveur : la nouvelle version, elle, sera installée active.
   */
  enabled: boolean,
  /**
   * Version installée (vide si l'hébergeur ne la connaît pas, ex. datapacks).
   */
  current_version: string,
  new_version: string,
  /**
   * Nom du fichier qui remplacera l'actuel.
   */
  new_filename: string,
  /**
   * `modrinth` | `curseforge`.
   */
  source: string,
};

/**
 * Pourquoi une proposition de l'hébergeur n'est pas appliquée.
 */
export type IgnoreReason = "other_platform" | "same_version" | "older_version" | "incomplete" | "up_to_date";

/**
 * Proposition de l'hébergeur écartée par le garde-fou local.
 */
export type IgnoredUpdate = {
  kind: ContentKind,
  key: string,
  name: string,
  current_version: string,
  new_version: string,
  new_filename: string,
  reason: IgnoreReason,
};

/**
 * Échec de la vérification d'un type (les autres types restent valables).
 */
export type UpdateCheckError = {
  kind: ContentKind,
  error: AppError,
};

/**
 * Résultat de la vérification des mises à jour d'un serveur.
 */
export type ContentUpdates = {
  /**
   * Version de Minecraft détectée par l'hébergeur ; `null` : inconnue, rien n'a pu être vérifié.
   */
  mc_version: string | null,
  /**
   * Plateforme détectée (`paper`, `fabric`…) quand l'hébergeur la donne.
   */
  platform: string | null,
  /**
   * Horodatage de la vérification (secondes Unix).
   */
  checked_at: number,
  /**
   * Éléments installés soumis à l'hébergeur.
   */
  examined: number,
  /**
   * Éléments que l'hébergeur n'a pas identifiés (projet perso, source inconnue) : leurs mises à
   * jour ne peuvent pas être suivies.
   */
  untracked: number,
  updates: Array<ContentUpdate>,
  ignored: Array<IgnoredUpdate>,
  errors: Array<UpdateCheckError>,
};

/**
 * Élément à mettre à jour (argument de [`crate::Core::apply_content_updates`]).
 */
export type UpdateSelection = {
  kind: ContentKind,
  key: string,
};

/**
 * Issue de la mise à jour d'un élément.
 */
export type UpdateStatus = "updated" | "old_kept" | "failed" | "skipped";

export type UpdateOutcome = {
  kind: ContentKind,
  key: string,
  name: string,
  from_version: string,
  to_version: string,
  status: UpdateStatus,
  reason: IgnoreReason | null,
  error: AppError | null,
};

/**
 * Bilan d'une mise à jour groupée.
 */
export type UpdateReport = {
  outcomes: Array<UpdateOutcome>,
  /**
   * Au moins un fichier a changé : le serveur doit redémarrer pour charger les nouvelles versions.
   */
  restart_needed: boolean,
};

// --- Mods Satisfactory (ficsit.rs) ---

/**
 * Un mod du catalogue.
 */
export type FicsitMod = {
  id: string,
  mod_reference: string,
  name: string,
  short_description: string,
  downloads: number,
  logo: string,
};

/**
 * Page de résultats de recherche.
 */
export type FicsitModPage = {
  mods: Array<FicsitMod>,
  count: number,
};

/**
 * Une version publiée d'un mod.
 */
export type FicsitVersion = {
  id: string,
  version: string,
  sml_version: string,
  size: number,
  hash: string,
  link: string,
  targets: Array<FicsitTarget>,
  dependencies: Array<FicsitDep>,
};

/**
 * Artefact d'une version pour une cible donnée (Windows / WindowsServer / LinuxServer).
 */
export type FicsitTarget = {
  target_name: string,
  link: string,
  hash: string,
  size: number,
};

/**
 * Dépendance d'une version (`mod_id` = mod_reference de la dépendance ; `SML` pour le loader).
 */
export type FicsitDep = {
  mod_id: string,
  condition: string,
  optional: boolean,
};

/**
 * Une version du Satisfactory Mod Loader.
 */
export type SmlVersion = {
  id: string,
  version: string,
  satisfactory_version: number,
  targets: Array<SmlTarget>,
};

/**
 * Artefact SML pour une cible (le lien pointe vers une release GitHub).
 */
export type SmlTarget = {
  target_name: string,
  link: string,
};

/**
 * Un mod présent dans le dossier `Mods/` du serveur (déduit du listing SFTP).
 */
export type FicsitInstalledMod = {
  /**
   * Référence du mod = nom du dossier (ex. `RefinedPower`).
   */
  reference: string,
  name: string,
  /**
   * Un mod désactivé a son dossier suffixé `.disabled`.
   */
  enabled: boolean,
};

/**
 * Un mod à installer (référence + version choisie) — élément d'une installation par lot.
 */
export type FicsitInstallItem = {
  reference: string,
  versionId: string,
};

// --- Mods multi-sources : Thunderstore, Factorio, uMod (mods.rs) ---

/**
 * Un mod du catalogue, quelle que soit la source.
 */
export type MarketMod = {
  /**
   * Référence stable, source-spécifique (sert à récupérer les versions / installer).
   */
  reference: string,
  name: string,
  description: string,
  downloads: number,
  icon_url: string,
  /**
   * `thunderstore` | `factorio` | `umod` | …
   */
  source: string,
};

/**
 * Page de résultats.
 */
export type MarketModPage = {
  mods: Array<MarketMod>,
  count: number,
  has_more: boolean,
};

/**
 * Une version publiée d'un mod (normalisée).
 */
export type MarketModVersion = {
  version: string,
  /**
   * Contrainte/compatibilité de version de jeu (SML, `factorio_version`…), ou vide.
   */
  game_version: string,
  /**
   * Dépendances lisibles (hors loader/framework), pour affichage.
   */
  dependencies: Array<string>,
};

/**
 * Un mod installé sur le serveur (normalisé), avec son état activé.
 */
export type MarketInstalledMod = {
  reference: string,
  name: string,
  enabled: boolean,
};

/**
 * Un mod à installer (référence + version voulue ; version vide = dernière).
 */
export type ModInstallItem = {
  reference: string,
  version: string,
};

// --- Service d'arrière-plan (service.rs, supervisor.rs) ---

/**
 * Vue d'ensemble pour la section Réglages « Service d'arrière-plan ».
 */
export type ServiceOverview = {
  /**
   * OS pris en charge (Windows, Linux systemd, macOS).
   */
  supported: boolean,
  /**
   * Binaire du service livré avec l'app.
   */
  binary_available: boolean,
  /**
   * Le service exécute une autre version que l'app : l'app s'est mise à jour, pas la copie
   * protégée du binaire que le service exécute (sa mise à jour exige une élévation).
   */
  outdated: boolean,
  /**
   * `not_installed` | `stopped` | `running` | `unknown`
   */
  install_state: 'not_installed' | 'stopped' | 'running' | 'unknown',
  /**
   * `ok` | `unreachable` | `forbidden` | `timeout` | `protocol`
   */
  link: string,
  status: ServiceStatus | null,
  /**
   * Qui supervise : `app` | `service`
   */
  supervision: 'app' | 'service',
  /**
   * Le service surveille-t-il le compte de l'app ? (`None` si l'un des deux est inconnu)
   */
  same_account: boolean | null,
  /**
   * `uac` | `pkexec` | `osascript` | `none`
   */
  elevation: 'uac' | 'pkexec' | 'osascript' | 'none',
  /**
   * Commande d'activation à lancer soi-même (repli sans élévation graphique).
   */
  manual_enable: string | null,
};

/**
 * État du service, tel qu'il le rapporte.
 */
export type ServiceStatus = {
  protocol: number,
  version: string,
  pid: number,
  started_at: number,
  /**
   * Lancé par le gestionnaire de services (sinon : `minestrator-daemon run` au premier plan).
   */
  managed: boolean,
  has_key: boolean,
  /**
   * Compte MineStrator surveillé (résolu après validation de la clé).
   */
  account_id: number | null,
  account_name: string | null,
  supervisor_enabled: boolean,
  data_dir: string,
  /**
   * Push FCM configuré.
   */
  fcm: boolean,
  /**
   * Dernier tour de poll réussi (`None` = aucun depuis le démarrage).
   */
  last_poll: number | null,
  /**
   * Serveurs sous connexion monitor.
   */
  monitored: number,
  suspended: boolean,
};

/**
 * État observable du superviseur (exposé par le service d'arrière-plan).
 */
export type SupervisorHeartbeat = {
  /**
   * Dernier tour de poll réussi (`None` = aucun depuis le démarrage).
   */
  last_poll: number | null,
  /**
   * Serveurs sous connexion monitor.
   */
  monitored: number,
  suspended: boolean,
};

// --- Comptes MineStrator (accounts.rs) ---

/**
 * Compte MineStrator enregistré sur l'appareil (aucun secret : la clé reste dans le stockage
 * sécurisé).
 */
export type Account = {
  /**
   * Identifiant utilisateur MineStrator.
   */
  id: number,
  pseudo: string,
  /**
   * Date d'ajout sur l'appareil (secondes Unix).
   */
  added_at: number,
  /**
   * Compte actif (celui de l'app, du MCP et des agents du Copilote).
   */
  active: boolean,
};

/**
 * Payload de l'event `account://switched` : le compte actif a changé (bascule, ajout depuis
 * l'écran de connexion, retrait). L'UI ferme les onglets serveurs de l'ancien compte et
 * recharge ses listes ; `current = null` ramène à l'écran de connexion.
 */
export type AccountSwitch = {
  previous: number | null,
  current: Account | null,
};

// --- Automatisations : redémarrage automatique, tâches planifiées (automation.rs) ---

/**
 * Automatisations configurées (persistées dans `automations.json`, transmises au service).
 */
export type AutomationConfig = {
  restart_rules: Array<RestartRule>,
  tasks: Array<ScheduledTask>,
};

/**
 * Redémarrage automatique après crash d'un serveur.
 */
export type RestartRule = {
  /**
   * Compte MineStrator propriétaire de la règle.
   */
  account_id: number,
  server_id: number,
  enabled: boolean,
  max_per_hour: number,
};

/**
 * Tâche planifiée d'un serveur.
 */
export type ScheduledTask = {
  /**
   * Identifiant attribué par le cœur (vide à la création).
   */
  id: string,
  /**
   * Compte MineStrator propriétaire (attribué par le cœur).
   */
  account_id: number,
  server_id: number,
  /**
   * Libellé libre (facultatif).
   */
  name: string,
  enabled: boolean,
  action: TaskAction,
  schedule: Schedule,
  /**
   * Dernière modification (secondes Unix, attribuée par le cœur) : aucune occurrence antérieure
   * n'est exécutée, et c'est l'origine des intervalles.
   */
  updated_at: number,
};

/**
 * Action d'une tâche.
 */
export type TaskAction = { "type": "restart", warnings: Array<number>, message: string, } | { "type": "snapshot", name: string, } | { "type": "commands", commands: Array<string>, };

/**
 * Récurrence d'une tâche, en heure locale de la machine qui l'exécute.
 */
export type Schedule = { "kind": "daily", hour: number, minute: number, } | { "kind": "weekly", days: Array<number>, hour: number, minute: number, } | { "kind": "interval", every_minutes: number, };

/**
 * Tâche + sa prochaine exécution (vue UI).
 */
export type TaskInfo = {
  task: ScheduledTask,
  /**
   * Prochaine occurrence (secondes Unix), `null` si la tâche est désactivée.
   */
  next_run: number | null,
};

/**
 * Automatisations d'un serveur, pour le compte courant (vue UI).
 */
export type ServerAutomations = {
  server_id: number,
  account_id: number,
  restart: RestartRule,
  tasks: Array<TaskInfo>,
  /**
   * Qui exécute en ce moment : `app` | `service`.
   */
  executor: 'app' | 'service',
  /**
   * Retard maximal toléré d'une occurrence (secondes).
   */
  tolerance_s: number,
  /**
   * Délais avant chaque tentative de redémarrage automatique (secondes).
   */
  backoff_s: Array<number>,
  /**
   * Commande de diffusion du jeu (`null` : pas d'annonce possible).
   */
  broadcast: string | null,
};

/**
 * Une exécution consignée (tâche planifiée ou redémarrage automatique).
 */
export type AutomationRun = {
  /**
   * Identifiant dans la base de la machine qui l'a exécutée (app ou service : pas unique entre
   * les deux).
   */
  id: number,
  account_id: number,
  server_id: number,
  /**
   * Tâche planifiée (`null` : redémarrage automatique).
   */
  task_id: string | null,
  /**
   * Libellé de la tâche au moment de l'exécution.
   */
  task_name: string | null,
  /**
   * `restart` | `snapshot` | `commands` | `auto_restart`
   */
  kind: 'restart' | 'snapshot' | 'commands' | 'auto_restart',
  /**
   * Occurrence planifiée (tâches).
   */
  scheduled_ts: number | null,
  started_ts: number,
  finished_ts: number | null,
  /**
   * `ok` | `failed` | `skipped` | `missed` | `gave_up`
   */
  status: 'ok' | 'failed' | 'skipped' | 'missed' | 'gave_up',
  /**
   * Résultat lisible (français).
   */
  message: string,
  /**
   * Détail anonymisé (erreur, logs de crash).
   */
  details: string | null,
};

// --- Suivi de disponibilité (uptime.rs) ---

/**
 * Rapport de disponibilité d'un serveur.
 */
export type UptimeReport = {
  server_id: number,
  generated_at: number,
  /**
   * 24 h, 7 j, 30 j (dans cet ordre).
   */
  windows: Array<UptimeWindow>,
  /**
   * Coupures des 30 derniers jours, des plus récentes aux plus anciennes.
   */
  outages: Array<Outage>,
  /**
   * Début de l'historique disponible (première observation), `null` si aucune.
   */
  since: number | null,
};

/**
 * Disponibilité sur une fenêtre glissante.
 */
export type UptimeWindow = {
  /**
   * Durée de la fenêtre (secondes) : 86 400, 604 800 ou 2 592 000.
   */
  seconds: number,
  /**
   * Temps effectivement surveillé dans la fenêtre.
   */
  monitored_s: number,
  /**
   * Dont en service.
   */
  up_s: number,
  /**
   * Part de `monitored_s` connue seulement par les statistiques de l'hébergeur (toujours en
   * service : il ne prouve jamais une coupure).
   */
  host_s: number,
  /**
   * `up_s / monitored_s` en % ; `null` si rien n'a été surveillé.
   */
  percent: number | null,
};

/**
 * Une coupure (période hors service surveillée).
 */
export type Outage = {
  start: number,
  /**
   * `null` : coupure en cours.
   */
  end: number | null,
  duration_s: number,
  /**
   * `crash` | `stop` | `unknown`
   */
  cause: 'crash' | 'stop' | 'unknown',
  /**
   * `false` : la surveillance s'est interrompue pendant la coupure — sa fin réelle est inconnue
   * (`end` est alors la dernière observation).
   */
  end_known: boolean,
};

// --- Alertes externes : webhooks (webhook.rs) ---

/**
 * Destinations configurées (persistées dans `webhooks.json`).
 */
export type WebhookConfig = {
  destinations: Array<WebhookDestination>,
};

/**
 * Destination d'alertes (partie **non secrète** : l'URL est dans le stockage sécurisé).
 */
export type WebhookDestination = {
  /**
   * Attribué par le cœur (vide à la création).
   */
  id: string,
  name: string,
  kind: WebhookKind,
  enabled: boolean,
  events: Array<WebhookEvent>,
};

/**
 * Format d'envoi.
 */
export type WebhookKind = "discord" | "json";

/**
 * Événement transmis aux destinations.
 */
export type WebhookEvent = "crash" | "auto_restart" | "threshold" | "mybox" | "task_success" | "task_failure";

/**
 * Destination vue par l'UI : jamais l'URL, seulement sa présence.
 */
export type WebhookInfo = {
  destination: WebhookDestination,
  has_url: boolean,
};

// --- Secrets des agents (agent_secrets.rs) ---

/**
 * Secret des agents, tel que listé (jamais sa valeur).
 */
export type AgentSecret = {
  /**
   * Nom, tel qu'écrit dans la référence `{{secret:NOM}}` (`[a-z0-9_-]`, 64 caractères au plus).
   */
  name: string,
  /**
   * À quoi sert le secret (texte libre, non secret, montré aux agents).
   */
  description: string,
  /**
   * Date de création (secondes Unix).
   */
  created_at: number,
  /**
   * Valeur tirée au hasard (« Générer », `generate_secret`) plutôt que saisie.
   */
  generated: boolean,
};

/**
 * Caractères d'un secret généré.
 */
export type SecretCharset = "alphanumeric" | "hex" | "url_safe" | "symbols";

// --- Vue Réseau : réseaux proxy (network/view.rs) ---

/**
 * Gravité d'un problème de réseau.
 */
export type NetworkSeverity = "critical" | "important" | "info";

/**
 * Plateforme d'un serveur, du point de vue d'un réseau proxy.
 */
export type NetworkPlatform = "velocity" | "bungeecord" | "paper" | "spigot" | "fabric" | "forge" | "vanilla" | "unknown";

/**
 * Logiciel d'un proxy.
 */
export type NetworkProxyKind = "velocity" | "bungeecord";

/**
 * Transfert des joueurs (IP, UUID, skin) du proxy vers les serveurs de jeu.
 */
export type ForwardingMode = "none" | "legacy" | "bungeeguard" | "modern" | "other";

/**
 * Secret d'un serveur de jeu comparé à celui du proxy — jamais sa valeur.
 */
export type SecretCheck = "same" | "different" | "missing" | "unknown";

/**
 * Comment une entrée a été rapprochée d'un serveur du compte.
 */
export type NetworkMatch = "primary" | "secondary" | "dns" | "resolved" | "port_only";

/**
 * Fichier de réglages de Paper trouvé.
 */
export type NetworkPaperConfig = "global" | "legacy" | "missing";

/**
 * BungeeGuard sur un proxy BungeeCord.
 */
export type NetworkGuard = "absent" | "token" | "token_unreadable";

/**
 * FabricProxy-Lite sur un serveur Fabric.
 */
export type NetworkFabricProxy = "absent" | "mod_only" | "configured";

/**
 * Réseaux proxy d'une MyBox (ou du compte) : proxys → entrées → serveurs du compte.
 */
export type NetworkTopology = {
  /**
   * Serveurs Minecraft Java de tout le compte (choix de l'assistant, noms, états).
   */
  servers: Array<NetworkServer>,
  /**
   * Proxys de la MyBox demandée (de tout le compte sans MyBox), triés par nom.
   */
  proxies: Array<NetworkProxy>,
  counts: NetworkCounts,
  /**
   * Serveurs non lus (suspendus, proxys illisibles…), et pourquoi.
   */
  not_read: Array<string>,
  notes: Array<string>,
};

/**
 * Problèmes par gravité.
 */
export type NetworkCounts = {
  critical: number,
  important: number,
  info: number,
};

/**
 * Serveur Minecraft Java du compte.
 */
export type NetworkServer = {
  id: number,
  name: string,
  mybox_id: number | null,
  /**
   * État d'infrastructure (`active`, `hibernation`, `suspended`…).
   */
  status: string,
  /**
   * Fichiers accessibles (actif ou en veille).
   */
  readable: boolean,
  /**
   * Plateforme repérée (hébergeur, jar de démarrage) ; `null` : inconnue.
   */
  platform: NetworkPlatform | null,
  version: string | null,
  /**
   * `ip:port` publique.
   */
  address: string | null,
  /**
   * Proxy repéré dans la MyBox lue.
   */
  proxy: boolean,
};

/**
 * Un proxy et ses entrées.
 */
export type NetworkProxy = {
  server_id: number,
  name: string,
  kind: NetworkProxyKind,
  /**
   * `/velocity.toml` ou `/config.yml`.
   */
  config_path: string,
  online_mode: boolean | null,
  forwarding: ForwardingMode,
  /**
   * Valeur écrite de `player-info-forwarding-mode` (Velocity).
   */
  forwarding_raw: string | null,
  /**
   * Velocity : secret de transfert défini (jamais sa valeur) ; `null` pour BungeeCord.
   */
  secret_defined: boolean | null,
  /**
   * D'où vient le secret ou pourquoi il manque (chemin, sans valeur).
   */
  secret_note: string,
  /**
   * BungeeCord seulement.
   */
  bungeeguard: NetworkGuard | null,
  /**
   * Liste de repli, dans l'ordre.
   */
  try_order: Array<string>,
  forced_hosts: Array<NetworkForcedHost>,
  entries: Array<NetworkEntry>,
  /**
   * Triés par gravité.
   */
  problems: Array<NetworkProblem>,
};

/**
 * Hôte forcé (routage par domaine).
 */
export type NetworkForcedHost = {
  host: string,
  servers: Array<string>,
};

/**
 * Une entrée du proxy.
 */
export type NetworkEntry = {
  name: string,
  /**
   * Adresse telle qu'écrite.
   */
  address: string,
  /**
   * Dans la liste de repli.
   */
  in_try: boolean,
  /**
   * Adresse locale (`127.0.0.1`, `localhost`) : ne mène à aucun autre serveur.
   */
  local: boolean,
  /**
   * Serveur du compte reconnu ; `null` : aucun.
   */
  target: NetworkTarget | null,
  backend: NetworkBackend | null,
  /**
   * Réglages non lus, et pourquoi.
   */
  unread: string | null,
};

/**
 * Serveur du compte visé par une entrée.
 */
export type NetworkTarget = {
  server_id: number,
  name: string | null,
  matched_by: NetworkMatch,
  /**
   * Le port de l'entrée est celui du serveur.
   */
  port_ok: boolean,
  /**
   * `ip:port` principale du serveur.
   */
  address: string | null,
  /**
   * Joignable directement depuis Internet (IP publique).
   */
  reachable: boolean,
  /**
   * L'entrée vise le proxy lui-même.
   */
  self_proxy: boolean,
};

/**
 * Réglages d'un serveur de jeu qui comptent derrière le proxy (secrets comparés seulement).
 */
export type NetworkBackend = {
  platform: NetworkPlatform,
  version: string | null,
  online_mode: boolean | null,
  paper_config: NetworkPaperConfig,
  /**
   * `proxies.velocity.enabled`.
   */
  velocity_forwarding: boolean | null,
  /**
   * Secret de transfert comparé (transfert moderne seulement).
   */
  velocity_secret: SecretCheck | null,
  /**
   * `spigot.yml` présent.
   */
  spigot_file: boolean,
  /**
   * `settings.bungeecord` de `spigot.yml`.
   */
  bungeecord: boolean | null,
  bungeeguard: boolean,
  /**
   * Fabric seulement.
   */
  fabric_proxy: NetworkFabricProxy | null,
  fabric_secret: SecretCheck | null,
  /**
   * Lui-même un proxy (chaîne de proxys, non vérifiée).
   */
  chained_proxy: boolean,
};

/**
 * Problème du réseau, avec sa correction.
 */
export type NetworkProblem = {
  severity: NetworkSeverity,
  /**
   * Serveur de jeu concerné ; `null` : le proxy.
   */
  server_id: number | null,
  message: string,
  /**
   * Conseil (vide s'il n'y en a pas).
   */
  advice: string,
  /**
   * La correction passe par l'assistant de liaison.
   */
  link_fix: boolean,
};

/**
 * Édition pinguée.
 */
export type NetworkPingEdition = "java" | "bedrock";

/**
 * Ping d'un serveur du compte à son adresse publique.
 */
export type NetworkPing = {
  server_id: number,
  name: string,
  edition: NetworkPingEdition,
  /**
   * `ip:port` principale du serveur (`null` : inconnue).
   */
  address: string | null,
  /**
   * Port qui a répondu (UDP en Bedrock).
   */
  port: number | null,
  online: boolean,
  /**
   * Pourquoi il ne répond pas.
   */
  error: string | null,
  version: string | null,
  protocol: number | null,
  /**
   * MOTD en texte brut (codes de couleur retirés).
   */
  motd: string,
  sub_motd: string | null,
  gamemode: string | null,
  players_online: number | null,
  players_max: number | null,
  latency_ms: number | null,
  /**
   * Réponse au ping legacy (avant 1.7).
   */
  legacy: boolean,
  /**
   * Mods annoncés (Forge).
   */
  modded: string | null,
};

/**
 * Base de données d'une MyBox (préréglage LuckPerms), **sans** mot de passe.
 */
export type NetworkDatabase = {
  id: number,
  label: string | null,
  host: string,
  port: number,
  database: string,
  username: string,
  /**
   * MySQL ou MariaDB (ou moteur inconnu, supposé tel) : utilisable par LuckPerms.
   */
  supported: boolean,
};

/**
 * État d'une étape d'un plan.
 */
export type NetworkStepStatus = "ok" | "change" | "manual" | "refused";

/**
 * Résultat d'une étape appliquée.
 */
export type NetworkOutcome = "done" | "failed" | "not_run";

/**
 * Une étape d'un plan : une écriture (un fichier, un réglage, une installation) ou une action
 * manuelle.
 */
export type NetworkStep = {
  /**
   * Identifiant stable (`<serveur>.paper`, `proxy.config`…).
   */
  id: string,
  server_id: number,
  server: string,
  status: NetworkStepStatus,
  summary: string,
  /**
   * Diff unifié, secrets masqués.
   */
  diff: string | null,
  outcome: NetworkOutcome | null,
  /**
   * Mention de la garde (annulation) ou erreur.
   */
  detail: string | null,
};

/**
 * Étapes par état.
 */
export type NetworkStepCounts = {
  change: number,
  ok: number,
  manual: number,
  refused: number,
};

/**
 * Où en est un plan.
 */
export type NetworkPlanStatus = "simulated" | "changed" | "needs_confirmation" | "refused" | "nothing_to_do" | "done" | "failed" | "cancelled";

/**
 * Snapshot prise avant l'application.
 */
export type NetworkSnapshot = {
  server_id: number,
  server: string,
  name: string,
  job_id: number,
};

/**
 * Cible d'une liste de repli ou d'un hôte forcé : identifiant d'un serveur de jeu de la
 * demande (nombre) ou nom d'une entrée déjà déclarée (texte).
 */
export type LinkTarget = number | string;

/**
 * Nom d'entrée imposé pour un serveur de jeu.
 */
export type LinkEntryName = {
  server_id: number,
  name: string,
};

/**
 * Hôte forcé demandé : domaine (sans port) → entrées.
 */
export type LinkForcedHost = {
  host: string,
  targets: Array<LinkTarget>,
};

/**
 * Mode de transfert.
 */
export type LinkMode = "modern" | "legacy";

/**
 * Demande de liaison d'un proxy à des serveurs de jeu (mêmes règles que l'outil `link_proxy`).
 */
export type LinkRequest = {
  proxy_server_id: number,
  /**
   * Ordre gardé : c'est la liste de repli par défaut.
   */
  backend_server_ids: Array<number>,
  /**
   * Noms imposés ; sinon le nom d'une entrée qui vise déjà le serveur, ou tiré de son nom.
   */
  names: Array<LinkEntryName>,
  /**
   * Liste de repli ; vide : l'actuelle si elle reste valable, sinon le premier serveur.
   */
  try_order: Array<LinkTarget>,
  forced_hosts: Array<LinkForcedHost>,
  /**
   * Retirer les entrées qui ne mènent à aucun serveur de jeu (exemples de la config).
   */
  prune: boolean,
  /**
   * `null` : modern pour Velocity, legacy pour BungeeCord.
   */
  mode: LinkMode | null,
  /**
   * Snapshot de chaque serveur modifié avant d'écrire (attendue).
   */
  snapshot: boolean,
};

/**
 * Changement d'une entrée.
 */
export type LinkEntryChange = "added" | "updated" | "unchanged";

/**
 * Changement d'un hôte forcé.
 */
export type LinkHostChange = "added" | "modified" | "unchanged" | "removed";

/**
 * Provenance du secret de transfert (jamais sa valeur).
 */
export type LinkSecret = "not_used" | "reused" | "generated";

/**
 * Entrée du proxy après le plan.
 */
export type LinkPlannedEntry = {
  name: string,
  address: string,
  server_id: number,
  server: string,
  change: LinkEntryChange,
};

/**
 * Hôte forcé après le plan.
 */
export type LinkPlannedHost = {
  host: string,
  servers: Array<string>,
  change: LinkHostChange,
};

/**
 * Entrée retirée (nettoyage).
 */
export type LinkPrunedEntry = {
  name: string,
  address: string,
};

/**
 * Plan de liaison (simulation) ou résultat de son application.
 */
export type NetworkLinkPlan = {
  proxy_id: number,
  proxy_name: string,
  proxy_kind: NetworkProxyKind,
  config_path: string,
  mode: LinkMode,
  secret: LinkSecret,
  /**
   * Fichier du secret réutilisé (chemin, sans valeur).
   */
  secret_source: string | null,
  entries: Array<LinkPlannedEntry>,
  try_order: Array<string>,
  try_changed: boolean,
  forced_hosts: Array<LinkPlannedHost>,
  pruned: Array<LinkPrunedEntry>,
  steps: Array<NetworkStep>,
  counts: NetworkStepCounts,
  warnings: Array<string>,
  /**
   * Empreinte du plan simulé : à rendre pour appliquer exactement ce plan.
   */
  fingerprint: string,
  status: NetworkPlanStatus,
  /**
   * Étapes écrites.
   */
  applied: number,
  snapshots: Array<NetworkSnapshot>,
};

/**
 * Préréglage de réseau.
 */
export type NetworkPresetKind = "luckperms" | "geyser";

/**
 * Demande de préréglage (mêmes règles que l'outil `network_preset`).
 */
export type NetworkPresetRequest = {
  preset: NetworkPresetKind,
  proxy_server_id: number,
  backend_server_ids: Array<number>,
  /**
   * LuckPerms : MyBox de la base (défaut : celle du proxy).
   */
  mybox_id: number | null,
  /**
   * LuckPerms : base (défaut : la seule de la MyBox).
   */
  database_id: number | null,
  /**
   * Geyser : port UDP du proxy (défaut : port actuel de Geyser ou port décrit « Bedrock »).
   */
  bedrock_port: number | null,
  snapshot: boolean,
};

/**
 * Serveur concerné par un préréglage.
 */
export type NetworkPresetServer = {
  server_id: number,
  name: string,
  proxy: boolean,
  platform: NetworkPlatform,
  /**
   * Nom de son entrée sur le proxy (contexte `server` de LuckPerms).
   */
  entry: string | null,
};

/**
 * Base retenue (LuckPerms), sans mot de passe.
 */
export type NetworkPresetDatabase = {
  mybox_id: number,
  id: number,
  label: string | null,
  host: string,
  port: number,
  database: string,
  username: string,
  /**
   * Mot de passe lisible par l'API (sinon aucune config n'est écrite).
   */
  password_readable: boolean,
};

/**
 * Port Bedrock retenu (Geyser).
 */
export type NetworkBedrockPort = {
  /**
   * `null` : aucun port utilisable, à ajouter (`manage_ports`).
   */
  port: number | null,
  /**
   * Pourquoi ce port.
   */
  source: string | null,
  /**
   * Appel de l'outil officiel `manage_ports` qui en ajoute un (JSON).
   */
  manage_ports: string | null,
  /**
   * IP MyShield : seuls quelques ports publics sont joignables.
   */
  myshield: boolean,
};

/**
 * Plan d'un préréglage (simulation) ou résultat de son application.
 */
export type NetworkPresetPlan = {
  preset: NetworkPresetKind,
  proxy_name: string,
  servers: Array<NetworkPresetServer>,
  steps: Array<NetworkStep>,
  counts: NetworkStepCounts,
  warnings: Array<string>,
  /**
   * Effets qui ne se défont pas entièrement : à confirmer explicitement.
   */
  to_confirm: Array<string>,
  database: NetworkPresetDatabase | null,
  bedrock: NetworkBedrockPort | null,
  /**
   * Empreinte du plan simulé : à rendre pour appliquer exactement ce plan.
   */
  fingerprint: string,
  status: NetworkPlanStatus,
  applied: number,
  snapshots: Array<NetworkSnapshot>,
};
