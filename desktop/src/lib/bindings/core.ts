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
 * Progression d'un transfert SFTP (upload / download / zip), pour le gestionnaire de transferts.
 * `id` = identifiant du transfert (généré côté front pour corréler). `done`/`total` en octets.
 */
export type SftpProgress = {
  id: string,
  /**
   * Nom lisible (fichier, ou nom du .zip pour un téléchargement groupé).
   */
  name: string,
  /**
   * `up` (téléversement) | `down` (téléchargement).
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
