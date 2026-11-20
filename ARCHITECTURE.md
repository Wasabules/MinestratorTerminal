# Minestrator Terminal — Architecture

> Client desktop léger pour piloter, surveiller et diagnostiquer ses serveurs Minecraft via l'API MineStrator.
> Stack : **workspace Rust 3 crates** + **Tauri 2** + **SvelteKit / Svelte 5 (runes)** + **xterm.js** + **CodeMirror**.
> Version **0.2.1** — licence **GPL-3.0-or-later** (public).
>
> Ce document décrit l'architecture **réelle d'aujourd'hui**. La spec fondatrice
> ([`docs/history/ARCHITECTURE-V1.md`](./docs/history/ARCHITECTURE-V1.md)) décrivait un simple
> *moniteur + console* ; le produit couvre désormais SFTP complet, marketplace,
> superviseur de fond, Copilote IA multi-fournisseur et serveur MCP. Les faits d'API validés en V1
> restent exacts et sont repris en §4.

---

## 0. Principes directeurs (invariants)

1. **Tout le réseau vit dans Rust, jamais dans le webview.** Appels HTTP, WebSocket console et SFTP
   sont gérés côté Rust. Le front ne voit **jamais** la clé API et ne parle qu'à des commandes Tauri.
   Bénéfices : clé jamais exposée au JS, pas de CORS, WebSocket robuste (l'en-tête `Origin` imposé par
   Wings est impossible à forger depuis un navigateur — cf. §4), perf.
2. **Console-first.** L'expérience serveur est bâtie autour de la console live et des jauges temps réel.
3. **Léger et instantané.** Binaire Tauri natif, démarrage quasi immédiat, RAM minimale, TLS `rustls`
   (aucune dépendance système : ni OpenSSL, ni SQLite système, ni zlib système).
4. **Dégrader proprement.** Serveur hiberné / suspendu / expiré, clé invalide, hors-ligne, rate-limit,
   sous-utilisateur sans permission : chaque état a un rendu défini et une erreur typée (`Error::kind`).
5. **Séparation stricte logique métier / UI.** Toute la logique vit dans `minestrator-core`, totalement
   UI-agnostique. Les frontends (desktop, serveur MCP, futur daemon/CLI) ne sont que de minces couches
   d'adaptation par-dessus le même `Core`.

---

## 1. Vue d'ensemble en couches

```
┌───────────────────────────────────────────────────────────────┐
│  SvelteKit (webview)  — Svelte 5 runes                         │
│  routes: /onboarding · / (workspace à onglets keep-alive)     │
│  stores rune: auth · tabs · colors · sftp-columns             │
│  xterm.js (console) · CodeMirror (éditeur fichiers) · i18n     │
│  couche ipc.ts typée  ·  abonnements events.ts  ·  updater.ts  │
└───────────────┬───────────────────────────▲──────────────────┘
     invoke()   │ (commandes)                │ events Tauri (tag conn_id)
                ▼                            │
┌───────────────────────────────────────────────────────────────┐
│  src-tauri  — couche d'adaptation desktop                     │
│  commands.rs (#[tauri::command]) · pont CoreEvent→webview      │
│  tray · notifications natives · updater · démarre superviseur  │
└───────────────┬───────────────────────────────────────────────┘
                ▼
┌───────────────────────────────────────────────────────────────┐
│  minestrator-core  — logique métier, UI-agnostique            │
│  api(reqwest) · console(WS Wings) · sftp(russh) · supervisor   │
│  copilot/llm/cli · mcp/official_mcp · store(SQLite) · secrets  │
└───────────────┬───────────────────────────────────────────────┘
                ▼
   API MineStrator (REST)  +  WebSocket Wings  +  SFTP  +  LLM/MCP externes
```

---

## 2. Workspace : 3 crates

`Cargo.toml` racine (`resolver = "2"`, version `0.2.1` partagée) :

```
members = [
  "desktop/src-tauri",           # app desktop (Windows/macOS/Linux)
  "mobile/src-tauri",            # app mobile (Android ; iOS à venir)
  "crates/minestrator-core",     # cœur métier partagé
  "crates/minestrator-mcp",      # serveur MCP headless
  "crates/minestrator-daemon",   # service d'arrière-plan Windows/Linux/macOS : supervision, IPC, push FCM
]
```

| Crate | Rôle | Nature |
|---|---|---|
| **`minestrator-core`** | Toute la logique métier, **indépendante de l'UI**. Expose une façade [`Core`] : chaque frontend en instancie une, appelle ses méthodes, s'abonne à ses `CoreEvent` via `Core::subscribe`. Le token API est géré en interne (trousseau OS) ; les appelants ne le manipulent jamais. Réutilisable → **futur daemon Linux, CLI/TUI**. | `lib` |
| **`minestrator-mcp`** | Binaire **serveur MCP headless** (stdio). Lit du JSON-RPC ligne à ligne sur stdin, délègue à `core::mcp`, écrit sur stdout. **stdout = protocole, logs sur stderr.** | `bin` |
| **`desktop/src-tauri`** | **App desktop** (Windows/macOS/Linux). Couche mince au-dessus du `Core` : commandes IPC, pont d'events → webview, notifications natives, tray, updater, démarrage du superviseur et du Copilote. Une app **`mobile/src-tauri`** (Android/iOS) rejoindra le workspace sur le même `Core`. | `lib`+`bin` |

### Diagramme des dépendances

```
                 ┌────────────────────────┐
                 │    minestrator-core    │   (façade Core, CoreEvent)
                 └───────────┬────────────┘
             depends │       │       │ depends
        ┌────────────┘       │       └─────────────┐
        ▼                    ▼                      ▼
┌───────────────┐   (le même binaire      ┌────────────────────┐
│   src-tauri   │    src-tauri --mcp       │   minestrator-mcp  │
│  (GUI + tray) │    sert AUSSI le MCP)    │  (serveur MCP pur) │
└───────────────┘                          └────────────────────┘
```

> **Double point d'entrée du desktop.** `desktop/src-tauri/src/main.rs` : si l'argument `--mcp` est présent,
> l'app lance `run_mcp()` (boucle stdio MCP) **au lieu** d'ouvrir la GUI. Le mode GUI et le binaire
> `minestrator-mcp` appellent tous deux la même `core::mcp::serve_stdio` — zéro duplication.

---

## 3. Modules du cœur (`crates/minestrator-core/src/`)

Regroupés par domaine (liste réelle des fichiers).

### 3.1 Réseau & modèle MineStrator
- **`api`** — client HTTP typé (`reqwest`/rustls). Token passé à chaque appel (client pur). Désenveloppage
  `api.data` centralisé, mapping des codes HTTP → `Error`, `authenticate` (valeur brute puis base64 en secours).
- **`console`** — `ConsoleManager` : une tâche tokio par `conn_id`, WebSocket Wings, reconnexion à backoff,
  ré-auth sur `token expiring`/`token expired`. Publie des `CoreEvent`. Les jetons de ré-auth viennent
  d'un cache groupé (`ApiClient::get_ws_credentials`) : `GET /user/{id}/servers/websocket` donne ceux de
  tous les serveurs en un appel, partagé 30 s par les connexions qui expirent ensemble (repli par serveur).
  Le superviseur ne relit les limites CPU/disque d'un serveur que toutes les 30 min.
- **`sftp`** — client SFTP natif (`russh`/`russh-sftp`) : `SftpManager` à **sessions poolées** par serveur
  (rouvertes à la volée si empoisonnées), list/read/write/mkdir/delete/rename/upload/download/walk/search.
  Identifiants complets, **mot de passe compris**, via `GET /server/{id}/sftp` (non documenté, utilisé
  par le panel web) : le bloc `sftp` de `GET /server/{id}` ne contient plus le mot de passe depuis fin
  2026, d'où un repli sans mot de passe sur ce dernier. En secours, un mot de passe saisi par
  serveur, vérifié par une session avant d'être enregistré (`set_sftp_password`), essayé après celui
  de l'API. Sans aucun : erreur `sftp_password_required`, que l'UI traduit en saisie.
  **Identité du serveur** : l'empreinte SHA-256 de la clé d'hôte est mémorisée au premier contact
  par `hôte:port` (`sftp_known_hosts.json`, `KnownHosts`) ; une empreinte différente bloque la
  connexion **avant** l'envoi du mot de passe (`sftp_host_key_changed`). L'UI montre les deux
  empreintes ; seule celle réellement présentée peut être acceptée (`trust_sftp_host_key`).
- **`models`** — structs `serde` : miroirs typés des réponses d'API et de leurs enveloppes brutes
  (`UserProfile`, `ServersOverview`/`MyBoxSummary`, `ServerDetails`, `LiveLight`, `Startup`, `MarketPage`,
  `Backup`, `Snapshot`, `InstalledItem`, `SftpEntry`…).
- **`updates`** — mises à jour des **mods, plugins et datapacks** Minecraft installés (endpoints
  non documentés du panel, §4.3) : version détectée (`/minecraft_version`), installés, vérification
  par l'hébergeur (pure comparaison), puis **garde-fou local** (`assess`) qui écarte les propositions
  visant une autre plateforme ou une version identique/plus ancienne. Application = installation de
  la cible par les endpoints d'installation, puis suppression de l'ancien fichier si son nom change,
  élément par élément après une nouvelle vérification ; bilan détaillé (`UpdateReport`), redémarrage
  nécessaire. Résultat gardé 15 min par serveur (`UpdatesCache`), oublié après une installation ou
  une mise à jour et au changement de compte. Desktop : section « Mises à jour » de la Marketplace,
  badges sur les installés, compteur de l'Aperçu ; mobile : onglet Mods et Aperçu ; MCP :
  `check_updates` (lecture), `update_mods` (modifiant).
- **`config`** — constantes (URLs, `Origin` Wings, comptes trousseau, version Anthropic, variables d'env MCP).
- **`error`** — `Error` unifié, sérialisé `{ kind, message }` (via `ErrorPayload`, discriminant typé
  `ErrorKind`) pour un traitement par cas côté frontend.
- **`bindings`** *(feature `ts`)* — génération des types TypeScript des frontends depuis les structs
  serde (ts-rs). Voir §5.3.
- **`events`** — `CoreEvent` (enum) + payloads `Serialize` diffusés sur un canal `broadcast`.
- **`util`** — petits helpers texte partagés.

### 3.2 Fichiers & monde Minecraft (au-dessus du SFTP)
- **`archive`** — lecture **en mémoire** de `.zip`/`.tar`/`.tar.gz`/`.gz` (lister, extraire une entrée,
  extraction complète anti zip-slip / anti zip-bomb). Pur, synchrone, testable sans réseau.
- **`server_files`** — opérations de fichiers **faites par le serveur** via l'API REST du panel
  (endpoints non documentés, cf. §4.5). Principe : le **SFTP reste le transport** (session poolée,
  flux binaire, reprise, pas de limite de débit de l'API) ; l'API ne sert qu'à ce que le SFTP ne sait
  pas faire sur place. **Compression** : job asynchrone suivi en *long polling* (`follow_job`, borné,
  annulable — seul le suivi s'arrête, l'API ne sait pas interrompre le job), archive conservée
  (`server_compress`) ou **temporaire** pour un téléchargement (`run_sftp_download_zip` : archive
  `minestrator-temp-<nom>-<horodatage>.zip` créée dans le dossier de la sélection, rapatriée par SFTP
  avec reprise, puis supprimée — y compris après une annulation, une fois le job fini ; seules les
  archives au préfixe temporaire sont supprimables). **Repli** sur l'archive construite par l'appareil
  (`build_zip`, SFTP récursif) si l'API refuse ou si le job échoue (`fallback_on` : tout sauf une clé
  refusée). **Extraction** (`extract_archive`) par le serveur (formats du panel : zip, tar.*, 7z, rar,
  mcworld…), repli SFTP (`sftp_extract_archive`, `.zip`/`.tar`/`.tar.gz`, plafonné). **Espace disque**
  (`disk_usage`, `disk_space`) : plus grosses entrées d'un dossier (tailles récursives calculées par le
  serveur), occupation comptée face à la limite du serveur et au disque partagé de la MyBox. Le reste
  du module est pur (chemins SFTP ↔ panel, corps de requête validés comme le panel, lecture tolérante,
  choix serveur/repli) et testé sans réseau.
- **`nbt`** — décodage NBT (`fastnbt`) `.dat`/`level.dat`/playerdata/`.schem` → arbre typé `NbtNode` + SNBT.
- **`mca`** — validation/réparation des régions Anvil `.mca` (table de localisation, chunks corrompus).
- **`world`** — outils monde de plus haut niveau (`inspect_region`, coordonnées de région, réparation).
  **`world_info`** (MCP, lecture) : résumé d'un monde lu dans `level.dat` et, depuis 26.1, dans
  `data/minecraft/{world_gen_settings,game_rules,weather,world_border,world_clocks}.dat` (Paper :
  sous `dimensions/minecraft/overworld/data/`) ; graine (`WorldGenSettings.seed` ou `RandomSeed`),
  version, mode, difficulté, apparition, jour/heure, règles, datapacks ; taille de chaque dimension
  (régions par SFTP aux trois emplacements possibles — `dimensions/minecraft/<dim>`, `DIM-1`/`DIM1`,
  `<monde>_nether`/`<monde>_the_end` —, dossiers par l'endpoint d'espace disque). Extraction pure, testée
  sur un `level.dat` 26.1 réel et sur les formats anciens.

Contenu, monde et audit (outils MCP en lecture `check_plugin_deps`, `world_info`, `audit_server`) :
- **`jar_meta`** — lecture **à la demande** des `.jar` : `ZipProbe` (pur) dit quelle tranche lire (fin
  d'archive, répertoire central zip/zip64, descripteurs seulement — jamais le jar entier, contrairement à
  `archive`) ; descripteurs `plugin.yml`, `paper-plugin.yml`, `bungee.yml`, `velocity-plugin.json`,
  `fabric.mod.json`, `quilt.mod.json`, `mods.toml`, `neoforge.mods.toml`, `mcmod.info` ; jars imbriqués ;
  contraintes de version (prédicats Fabric/Quilt, intervalles Maven de Forge). Plafonds : répertoire
  central 4 Mio, descripteur 512 Kio.
- **`plugin_deps`** — `check_plugin_deps` : liste `plugins/` et `mods/` (SFTP), lit les jars en
  parallèle (6 à la fois, 400 jars et 192 Mio lus au plus, session morte rejouée une fois), puis règles
  pures : dépendances obligatoires manquantes ou désactivées, optionnelles absentes, versions de
  Minecraft/API/dépendance, doublons, autre plateforme, mauvais dossier, mods client, Folia, jars
  illisibles. Plateforme d'après `/minecraft_version`, déduite du contenu si l'hébergeur ne la nomme pas.
- **`server_audit`** — `audit_server` : règles pures sur des données déjà lues (`server.properties`
  via l'API, `ops.json`, `spigot.yml`/`paper-global.yml` pour le proxy, `-Xmx` face à la RAM du
  conteneur, Java d'après l'image face à Minecraft, mises à jour sans effet de bord d'`updates`).
  Contrôles choisis selon l'édition (`games.rs` : Java, Bedrock, autre jeu non géré). Aucun secret en
  sortie (mot de passe RCON qualifié vide/faible/correct). Porte aussi `server_capabilities`.
- **`findings`** — constats triés par gravité (critique, important, info) avec recommandation ; rendu
  texte borné, commun à `plugin_deps` et `server_audit`.
- **`yaml_lite`** — sous-ensemble YAML tolérant (blocs, listes et mappings en ligne, guillemets, `|`/`>`)
  pour les descripteurs Bukkit et les réglages de serveur, sans dépendance.

### 3.3 Surveillance, historique & persistance
- **`supervisor`** — monitoring de fond : connexions **monitor** légères (stats/status + repérage
  WARN/ERROR), historique de métriques et **détection d'alertes** (crash, seuils CPU/RAM/disque, expiration).
  Réglable via `SupervisorConfig` persistée ; état vivant dans `SupervisorState`. Classe chaque
  transition d'état (`crash` / `stop`) et consigne les **observations d'état** (suivi de
  disponibilité). À un crash, capture les dernières lignes de console (`GET …/console/logs`,
  anonymisées par `redact`) et les joint à l'alerte (`Alert.details`), puis signale le crash aux
  automatisations. L'état connu d'un serveur est oublié dès que sa connexion monitor tombe : un
  arrêt volontaire survenu pendant la coupure ne passe jamais pour un crash.
- **`automation`** — automatisations locales exécutées par le **superviseur actif** (l'app, ou le
  service d'arrière-plan quand il supervise le même compte — jamais les deux) : **redémarrage
  automatique après crash** (par serveur, délais 30 s / 2 min / 5 min, plafond horaire, alerte
  critique au-delà) et **planificateur** (redémarrage avec annonces aux joueurs via
  `GameCapabilities::broadcast`, snapshot, commandes ; quotidien, jours de la semaine ou intervalle,
  heure locale via `chrono`, tolérance de 5 min sans rattrapage). Règles et tâches rattachées au
  compte (`account_id`). Config `automations.json`, journal des exécutions en SQLite. Ce sont des
  automatisations **de l'app** (ordinateur allumé), distinctes des tâches du panel web.
- **`webhook`** — alertes vers **Discord** (embeds, aucune mention interprétée) et **webhook JSON** :
  destinations non secrètes dans `webhooks.json`, **URL dans `secrets`** ; événements choisis par
  destination ; tout passe par `redact` ; regroupement (5 s) et débit limité (un envoi / 10 s par
  destination, file bornée). Erreurs journalisées sans URL.
- **`uptime`** — suivi de disponibilité (pur) : observations → périodes surveillées → % sur
  24 h / 7 j / 30 j et journal des coupures (cause `crash` / `stop` / inconnue). Les périodes non
  surveillées (app fermée, PC éteint) ne comptent pas ; fusion avec l'historique du service, puis
  **comblement par l'hébergeur** (`host_spans` / `fill_unmonitored`) : une minute *mesurée* par
  l'hébergeur prouve que le serveur tournait et compte « en service » là où rien ne surveillait ;
  une minute à zéro (serveur éteint **ou** non mesurée : l'API ne les distingue pas) reste non
  surveillée — l'hébergeur ne crée **jamais** de coupure et ne contredit jamais le local. La part
  qu'il fournit est exposée (`UptimeWindow.host_s`) et affichée.
- **`history`** — **historique croisé** : échantillons locaux (app + service, 10 s, 14 jours) et
  statistiques à la minute de l'hébergeur (`GET /server/{id}/stats/{début}/{fin}`, voir §4.4).
  Le local fait foi là où il existe ; les minutes mesurées de l'hébergeur comblent les trous et
  prolongent l'historique jusqu'à 30 jours ; unités ramenées à celles du local ; chaque point porte
  son origine (`HistorySource` : `local` | `host`) ; regroupement par tranches en gardant le maximum ;
  joueurs connectés (hébergeur seulement). Cache mémoire par blocs de 6 h : un bloc lu plus de
  10 min après sa fin ne change plus et n'est jamais relu, le bloc en cours l'est au plus une fois par
  minute, une requête par tranche de 3 jours au plus, pause d'une minute après une erreur. Les
  bornes sont en **heure de Paris** (calculée en Rust : règle européenne des changements d'heure) ;
  un écart d'horloge de la machine (en-tête `Date` des réponses, au-delà de 90 s) est corrigé pour
  que les deux séries restent alignées. Façade : `Core::server_history` (commande
  `metrics_history`, outil MCP `server_metrics`) et le comblement de `Core::uptime_report`.
- **`store`** — historique local de métriques en **SQLite embarqué** (`rusqlite` *bundled*). L'API
  ne garde qu'une mesure par minute (et seulement en moyennes horaires au-delà de 7 jours) : l'app
  accumule son propre historique, plus fin (10 s) et porteur de l'état Wings, que `history` complète.
  Même base : journal d'alertes (avec `details`, colonne ajoutée par migration), observations d'état
  (`uptime`) et journal des automatisations (`automation_runs`), gardés 32 jours.
- **`cache`** — cache mémoire TTL des lectures d'API stables/semi-stables (au niveau du `Core`).
- **`persist`** — config JSON persistée (load/get/set sous mutex) dans le dossier data de l'OS ;
  mutualise MCP, Copilote et confidentialité.
- **`logs`** — journaux **fichier** à rotation quotidienne (`<data>/logs/`, 7 jours) installés par
  l'app desktop et mobile (`logs::init`), et **export anonymisé** (`redact`) pour un rapport de bug
  (Réglages → Debug sur desktop, Réglages sur mobile). Sans eux, les logs d'une release Windows
  (sans console) ou Android (stdout non relié à logcat) sont perdus.
- **`perf`** — analyse de performance via **Spark** : orchestre health/tps/gc + profiler, télécharge et
  parse le rapport protobuf (`prost`), assemble un contexte pour le Copilote.
- **`doctor`** — docteur démarrage : rassemble commande de démarrage + fin de `latest.log` + crash-report
  et pré-scanne les pannes connues (EULA, port, OOM, version Java, mixin, monde corrompu…).
- **`server_logs`** (journaux et historique) — journaux **des serveurs** (≠ `logs`, ceux de l'app),
  outils MCP `search_logs` et `error_digest`. Lecture SFTP plafonnée du dossier des journaux
  (`games::log_dir`, paramétrable) : `latest.log` + archives `.log.gz` (`archive::gunzip_capped`),
  plafonds de fichiers, d'octets téléchargés, de taille par fichier et de résultats, toujours
  signalés. Parsing pur et testé sur des extraits Paper, Spigot, Fabric, Forge et Unreal :
  horodatages, datation des lignes (jour du nom de fichier, passage de minuit, remontée depuis la
  dernière écriture, décalage UTC du serveur déduit de `latest.log`), enregistrements multi-lignes,
  **signatures d'erreur** (cause première + frames utiles, normalisées), plugin ou mod probable,
  alias des pseudos (`redact::pseudonym`, comme `list_players`).
- **`incidents`** (journaux et historique) — journal local des incidents (table `incident_log`) :
  l'app de bureau y consigne alertes et diagnostics du Copilote (`Core::start_incident_journal`) ;
  l'outil MCP `list_incidents` le relit, y ajoute les alertes du service d'arrière-plan (IPC, s'il
  supervise le compte) et les crashs du suivi de disponibilité, et rattache chaque diagnostic à son
  incident (`assemble`, pur). Le MCP headless partage le dossier de données mais ne surveille rien :
  hors de l'app, `Core::probe_service_mode` consulte le service sans effet de bord, et l'absence de
  données est dite explicitement. `uptime_report` met en forme `Core::uptime_report` (`uptime::ai_report`).

### 3.4 Copilote IA & serveur MCP
- **`copilot`** — agent LLM : diagnostic automatique déclenché par les alertes du superviseur, analyse de
  sélection, analyse de perf, et **chat multi-tours** (onglet Assistant). Client MCP interne : n'a que les
  outils de **lecture**, *propose* les actions modifiantes selon le niveau d'`Autonomy`.
- **`llm`** — couche **multi-fournisseur** : `Provider::Anthropic` (API *Messages*) et
  `Provider::OpenaiCompatible` (GPT, Gemini, Mistral, Groq, xAI, DeepSeek, OpenRouter, Ollama/LM Studio/vLLM
  locaux). Format d'agent normalisé ; ajouter un fournisseur = un variant + son (dé)sérialiseur.
- **`cli` / `cli_agent` / `cli_session`** — agents **CLI locaux** (Claude Code, OpenCode, Gemini CLI) pilotés
  **sans clé API** (abonnement de la machine). `cli` = exécution one-shot robuste (stdin→stdout) ;
  `cli_agent` = adaptateurs par CLI (flags, config MCP, parsing) ; `cli_session` = process **persistant**
  (Claude Code en `stream-json`) réutilisé entre messages, avec repli one-shot.
- **`mcp`** — logique MCP **indépendante du transport** : traite un message JSON-RPC, expose la liste
  d'outils et le `dispatch`. Réglable via `McpConfig` (activer/désactiver, mode lecture seule). Réutilisée
  par `minestrator-mcp`, par `src-tauri --mcp`, et par le Copilote en interne.
  Protocole (voir `MCP.md` §2) : `mcp_transport` (stdio concurrent, une tâche par requête, versions
  `2024-11-05` → `2026-07-28` avec et sans poignée de main, annulation, élicitation), `mcp_ctx`
  (contexte d'appel des outils : session, progression, annulation, confirmation par l'humain),
  `mcp_catalog` (groupes d'outils `--toolsets`, annotations, instructions), `mcp_prompts`,
  `mcp_resources`.
- **`official_mcp`** — client du **serveur MCP OFFICIEL** de MineStrator (`https://mcp.sttr.io/minestrator`,
  Streamable HTTP stateless, `Bearer <clé API>`, `?toolsets=…&readonly=1`) : 107 outils en 10
  toolsets (relevé du 2026-11-18, 43 en lecture). Notre MCP local garde le SFTP fin + les outils
  exclusifs (`inspect_region`, Spark, docteur démarrage).
- **`official_relay`** — **politique** d'accès à ces outils, partagée par deux consommateurs.
  Copilote : deny-by-default (lecture = liste `readonly=1` ; en écriture, la seule liste blanche
  `COPILOT_OFFICIAL_WRITES` d'actions bornées à un serveur et réversibles), filtrée au listing et à
  l'exécution, portée limitée au serveur de la session ; ses agents CLI ne se connectent plus à
  l'officiel, notre serveur `--mcp` le relaie (variables `MINESTRATOR_MCP_OFFICIAL`,
  `MINESTRATOR_MCP_COPILOT_SERVER`). Passerelle du MCP local (`McpConfig::official_gateway`,
  désactivée par défaut) : tous les toolsets sauf `files`, gating lecture/écriture, `confirm_name`
  vérifié localement pour tout outil destructif. Table d'équivalences `EQUIVALENTS` (un outil par
  fonction), catalogue en cache 10 min.
- **`fleet`** — outils MCP **« Plusieurs serveurs »** : copie de serveur à serveur
  (`copy_between_servers` : fichier par fichier en flux SFTP, ou archive compressée par le serveur
  source, transférée, extraite par la cible dans un dossier de transit puis mise en place, avec
  nettoyage des deux côtés même en cas d'échec), comparaison (`diff_files` : diff unifié via
  `similar`, filtré par `redact` ligne par ligne ; listings de dossiers), recherche d'un joueur sur
  tout le compte (`find_player` : `live_players_bulk` + listes de `players`), commande groupée
  (`broadcast_command`, REST). Partie pure (validation, plan de copie et collisions, stratégie,
  comparaison, fusion des joueurs, ciblage) testée sans réseau ; les deux outils modifiants sont
  dans `INFRA_TOOLS` (jamais au Copilote).
- **`agent_guard`** — **garde des agents MCP** : journal anonymisé de chaque appel reçu par le
  protocole (`protocol_dispatch`, branché dans `mcp::call`), **changesets** (copie avant chaque outil
  modifiant passé par `mcp::dispatch`, Copilote compris, puis opérations inverses ; annulation par
  `rollback_changeset`, refusée si la cible a changé depuis) et **mode constructeur**
  (`McpConfig::write_scope` : écritures limitées aux serveurs créés par la session ou autorisés).
  Base `agent_guard.db` (WAL) partagée par l'app et les processus MCP, rétention et taille bornées ;
  capture et annulation derrière le trait `Remote`, testées sur un faux serveur.
- **`agent_loop`** — outils **« agir puis vérifier »** : tampon de console par serveur (connexion
  WebSocket dédiée `agent:<id>:<époque>`, 5 000 lignes numérotées, curseurs, rejeu de Wings marqué
  historique, fermeture après 10 min sans lecture) derrière `tail_console`, `run_command` et
  `wait_for` ; `deploy_and_verify` (jar local, envoi et mise de côté par `mcp::dispatch` pour que
  la garde les capture, redémarrage, verdict tiré de la console).
- **`config_edit`** — lecture et **édition structurée** des configs (YAML via `saphyr-parser`, TOML
  via `toml_edit`, JSON/JSONC, `.properties`) : seule la plage de la valeur est réécrite, le
  résultat est relu et comparé à l'arbre attendu avant écriture ; secrets masqués et valeurs
  masquées refusées. Outils `get_config` / `set_config`. Retrait de clés (`config_edit/remove.rs`,
  `set_config` avec `remove`) : chaque éditeur de format retire la clé (lignes en YAML, entrée de
  table en TOML, membre et virgule en JSON, lignes logiques en `.properties`), puis le fichier relu
  doit être l'ancien privé des seules clés retirées.
- **`manifest`** — **manifeste déclaratif** d'un serveur (v1) : export, plan (écart avec le serveur
  réel, étapes à identifiant stable, `manual` / `unsupported` expliqués), application idempotente
  après snapshot, et `clone_server` (`manifest/clone.rs` : création, copie, manifeste de la source).
  Les étapes `manual` de version, plateforme, Java et JAR désignent l'outil officiel qui les fait
  (`manifest/official.rs` : `change_version`, `manage_java`, `set_startup_config`, arguments et
  disponibilité par la passerelle, `official_relay::gateway_availability`).
- **`pregen`** — outil **`pregenerate_world`** : prégénération avec Chunky (détection dans le
  contenu installé, installation depuis Modrinth par `install_mod` passé par la garde, estimation
  du disque, commandes et réponses par la boucle d'`agent_loop`, analyse des lignes de progression,
  suivi avec avancement MCP). Dans `INFRA_TOOLS`.
- **`db_connect`** — outil **`connect_database`** : accès d'une base de la MyBox lus avec tolérance
  dans `GET /mybox/{id}` (repli : `get_mybox` officiel en lecture), écrits dans la config d'un
  plugin par `set_config` (garde des agents) sans que le mot de passe apparaisse ; réinitialisation
  facultative par l'outil officiel `manage_database`, confirmée par l'humain. Dans `INFRA_TOOLS`.
- **`network`** — **réseaux proxy** Velocity / BungeeCord : repérage des proxys, rapprochement de
  leurs entrées avec les serveurs du compte (`GET /server/{id}` : ip, port, DNS, ports secondaires),
  cohérence des serveurs de jeu (`network_topology`) ; liaison (`network/link.rs`, `link_proxy`) :
  plan pur avec diffs masqués, secret de transfert réutilisé ou généré (CSPRNG) et jamais rendu,
  écritures par `agent_guard::guarded_write`.
- **`local_sync`** — outil **`sync_dir`** : dossier de l'ordinateur synchronisé vers un dossier
  d'un serveur (parcours local sans suivre les liens, motifs glob `include`/`exclude`, `.git/`
  exclu, plan pur, sha256 à taille égale avec lecture distante plafonnée, suppressions bornées à
  `remote_dir` et aux motifs). Trait `ServerSide` (lectures SFTP, écritures nommées comme les
  outils MCP et passées par `guarded_write`) partagé avec `world_upload`, testé sur le faux serveur
  de la garde ; point d'accroche `TEXT_HOOK` pour les fichiers texte avant envoi. Dans
  `LOCAL_DISK_TOOLS`.
- **`world_upload`** — outil **`upload_world`** : monde local (dossier ou `.zip`) vérifié
  (`level.dat`, dossier racine en trop, entrées dangereuses), recompressé ou recopié sans
  recompression (`zip`), arrêt confirmé par l'humain, snapshot attendue, ancien monde renommé
  `<nom>.old-<date>`, envoi et extraction par le serveur, `level-name`. Dans `LOCAL_DISK_TOOLS` et
  `INFRA_TOOLS`.
- **`fleet_apply`** — outil **`apply_to_servers`** : même outil modifiant (liste blanche, jamais
  un outil d'`INFRA_TOOLS`) sur une liste de serveurs, `server_id` injecté, contrôles d'un appel
  direct puis par serveur (jeu, mode constructeur), appels par `mcp::dispatch`, rapport par
  serveur. Dans `LOCAL_DISK_TOOLS` et `INFRA_TOOLS`.
- **`platform`** — outil **`set_server_platform`** : plateforme, version, Java et JAR d'un serveur
  Minecraft sans le panel. Lectures (catalogue `search_game_content`, builds `get_content_files`
  en `readonly=1`, `GET /site/java/versions`), voie du catalogue (`change_version`) ou voie JAR
  (`platform/jar.rs` : JAR officiel PaperMC Fill v3 ou Purpur, vérifié, envoyé par `upload_file`
  via la garde, choisi par `set_startup_config`) ; confirmation du nom exact avant tout effet ;
  outils officiels appelés comme étapes de la garde (`official_write`). Réutilisé par
  `apply_manifest` (`apply_official`). Dans `INFRA_TOOLS`. `db_connect` porte aussi
  **`delete_database`** (`DELETE /mybox/{id}/database/{id}`, nom exact confirmé).
  routage par domaine (`forced_hosts`) et nettoyage des entrées qui ne mènent à rien (`prune`),
  écritures par `agent_guard::guarded_write` (boucle partagée `link::apply_steps` : snapshots
  attendues, une étape annulable par écriture). `network/install.rs` : plateforme du serveur
  vérifiée par `install_mod` (build de proxy sur un proxy, le catalogue MineStrator n'offrant que
  les builds Bukkit/Paper), API publique de Modrinth pour les builds de proxy, source `geysermc`
  (Geyser, Floodgate), téléchargements vérifiés par empreinte et déposés par SFTP.
  `network/preset.rs` (`network_preset`) : préréglages `luckperms` (une base pour tout le réseau,
  par `db_connect`) et `geyser` (clé de Floodgate copiée de serveur à serveur sans être lue, port
  Bedrock du proxy). `network/ping.rs` (`ping_server`, option `ping` de `network_topology`) :
  Server List Ping Java (repli legacy 0xFE) et ping RakNet Bedrock, vers l'adresse d'un serveur du
  compte seulement.
- **`blueprint`** — **réseau entier depuis un fichier** : un proxy et ses serveurs de jeu décrits
  en un document JSON (format v1, validé strictement : `Network::parse`), monté sur une MyBox sans
  le panel. `plan_network` (lecture) : rapprochement (`server_id`, sinon nom exact dans la MyBox),
  place libre de la MyBox, puis simulation des outils élémentaires serveur par serveur
  (`set_server_platform`, `plan_manifest`, `sync_dir`, `upload_world`) et pour le réseau
  (`link_proxy`, `network_preset`), statut `ok` / `create` / `change` / `manual` / `blocked` par
  élément et ordre d'exécution. `apply_network` (`blueprint/apply.rs`) : `dry_run` par défaut ;
  avant tout effet, mode constructeur vérifié pour chaque serveur rapproché (par nom compris),
  récapitulatif confirmé par l'humain, nom exact retapé pour chaque `change_version`, snapshots ;
  puis phases dans l'ordre (création, plateforme avec attente de la fin de `change_version`,
  contenu, mondes, premier démarrage pour générer les fichiers, manifestes, dossiers, liaison,
  préréglages, démarrage, prégénération, vérification), chaque écriture par l'outil élémentaire
  et la garde des agents (`plan::sub_dispatch` : enveloppe de `mcp::dispatch` sans repasser par
  son grand `match`, trop lourd en debug pour deux niveaux), arrêt au premier échec, idempotent
  (marqueur `/.minestrator-blueprint.json` pour le monde et la prégénération). Les confirmations
  simples d'un sous-outil sont pré-accordées (`mcp_ctx::preapproved`) seulement si elles figuraient
  dans le récapitulatif. `blueprint/export.rs` : `export_network` (blueprint d'un réseau existant)
  et `network_template` (modèles répartis sur la place libre). Accès au compte derrière le trait
  `Net`, testé sur un faux compte. Dans `INFRA_TOOLS` (`apply_network`), `LOCAL_DISK_TOOLS`
  (`plan_network`, `apply_network`) et `BLUEPRINT_TOOLS` (jamais au Copilote).

### 3.5 Sécurité & confidentialité
- **`accounts`** — plusieurs comptes MineStrator sur un appareil et changement de compte (app
  seulement ; le service reste à clé unique). Clé du compte **actif** sous `api-key` (ce que lisent
  le MCP headless, les agents CLI, le superviseur et l'activation du service), clés des comptes
  **inactifs** sous `api-key-<idUtilisateur>`, liste non secrète (id, pseudo, date d'ajout, compte
  actif) dans `accounts.json`, constituée d'elle-même au premier lancement (`Core::session_user`).
  L'échange de clés n'a **aucune fenêtre de perte** : copie vérifiée de la clé active à l'écart,
  installation de la cible (rétablissement en cas d'échec), ménage en dernier ; une interruption
  laisse au pire un doublon, résorbé à la résolution suivante du profil (la clé active fait foi).
  Une bascule ferme consoles et sessions SFTP, vide les caches (token, id utilisateur, TTL) et les
  conversations de l'assistant, redécide qui supervise (`service_tick`), réveille le superviseur et
  émet `CoreEvent::AccountSwitched`. L'index `sftp_passwords.json` rattache chaque serveur à son
  compte (l'ancien format, liste d'ids, est rattaché au compte actif) : retirer un compte n'efface
  que ses mots de passe. Testé sur le backend fichier d'un dossier temporaire, pannes injectées.
- **`panel_schedules`** — tâches planifiées **du panel MineStrator** (`/server/{id}/schedule…`,
  non documentées, modèle Pterodactyl) : exécutées par l'hébergeur même PC éteint, à distinguer des
  automatisations locales (`automation`). Lecture, création, modification, actions (`command` /
  `power`, délai 0-900 s) et ordre ; toute saisie est validée avant envoi (cron borné, commande
  sur une ligne). Desktop : section de la vue Automatisations ; mobile : vue « Tâches » (lecture et
  activation) ; MCP : `list_schedules` et les outils modifiants (`create_schedule`,
  `update_schedule`, `delete_schedule` gardé par le nom exact, `add_/update_/delete_schedule_task`,
  `reorder_schedule_tasks`), dans `INFRA_TOOLS` : jamais donnés au Copilote.
- **`players`** — joueurs **déjà venus** d'un serveur Minecraft Java (capacité `player_data` de
  `games.rs`), lus par l'hébergeur dans `playerdata/` : `GET /server/{id}/players/offline`
  (non documenté ; pages de 200 au plus, **toujours triées par dernière sauvegarde**, `play_time`
  en ticks, `mtime` en secondes Unix), `…/players/{uuid}/profile` et `…/players/{uuid}/inventory`.
  Pour trier par temps de jeu, toute la liste est lue (pages en parallèle, plafond 5 000 joueurs,
  doublons éliminés), gardée **une minute** par serveur (`RosterCache`, purgé par `player_action`
  et au changement de compte) ; recherche, filtres, tri, pagination et statistiques (actifs 24 h /
  7 j / 30 j, temps de jeu cumulé, classement) sont de la logique pure testée. L'inventaire est
  décodé en emplacements lisibles (barre d'action, sac, armure, main secondaire ; nom
  personnalisé, enchantements, usure, icône du CDN de MineStrator), anciens formats compris.
  L'UUID est validé avant d'entrer dans un chemin d'URL. L'édition d'inventaire du panel n'est
  **pas** reprise (écriture d'un fichier joueur, non testable sans risque). Desktop : vue Joueurs
  (statistiques, liste, fiche en panneau latéral avec la grille d'inventaire) ; mobile : liste et
  fiche en lecture ; MCP : `list_players` (pseudos remplacés par un alias stable,
  `redact::pseudonym`, si l'anonymisation IA est active).
- **`mybox`** — gestion des MyBox (analyse : [`docs/api/MYBOX.md`](docs/api/MYBOX.md)). Capacité
  (`MyBoxCapacity` : CPU en cœurs et RAM en Go totaux / alloués aux serveurs **actifs** / restants,
  disque **partagé**, emplacements `pods`, FlexCore, `blockers`), catalogue de création (jeux,
  champs requis, ressources recommandées, localisations ; cache 1 h), écritures construites et
  **validées sans réseau** (`WriteRequest` : création, redimensionnement, pause/reprise, ordre,
  renommages, snapshot avant suppression, suppression gardée par le nom exact) puis envoyées par
  `ApiClient::send_raw` ; chaque écriture vide le cache TTL. `plan_placement` : plan de placement
  **pur** (stratégies `pack`/`spread`, pistes de mise en pause). Desktop : onglet unique « MyBox »
  (`MyBoxView`, dialogues dans `src/lib/mybox/`) ouvert depuis l'accueil ou la palette ; mobile :
  lecture des ressources et renommage ; MCP : outils « architecte » (`mybox_capacity`,
  `server_catalog`, `plan_placement`, `mybox_smartscan`) et `INFRA_TOOLS` modifiants, **jamais**
  exposés au Copilote : absents de son catalogue, de la liste blanche de ses agents CLI et
  d'« Appliquer » — et leurs équivalents officiels (`create_server_on_mybox`,
  `update_server_resources`, `enable_disable_server`, `delete_server_from_mybox`, renommage…) hors
  de sa liste blanche, refusés au listing comme à l'exécution (`official_relay`).
- **`secrets`** — stockage sécurisé natif : clés API (compte actif `api-key`, comptes inactifs
  `api-key-<id>`), clés LLM par fournisseur (`llm-key-<slug>`),
  secrets par jeu, mots de passe SFTP par serveur (`sftp-password-<id>`), URL des destinations
  d'alertes (`webhook-url-<id>` : une URL Discord suffit à poster). Trousseau de l'OS
  (`keyring`) sur desktop ; **Keystore Android** (`android-native-keyring-store` : AES-GCM, clé non
  exportable) sur Android, avec migration puis suppression de l'ancien `secrets.json` en clair.
  Backend **fichier** réservé au daemon (et à iOS, non livré) : dossier à accès restreint, jamais le
  répertoire temporaire, `0600` sous Unix, **chiffré DPAPI** (compte du service) sous Windows.
- **`agent_secrets`** — **secrets des agents** : valeurs (token de bot, clé d'API, mot de passe
  RCON…) qu'un agent MCP écrit dans les configs par la référence `{{secret:NOM}}` sans jamais les
  voir. Valeur sous `agent-secret-<nom>` dans `secrets`, index non secret (nom, description, date,
  généré ou saisi) dans `agent_secrets.json`, relu à chaque accès (partagé par l'app et les
  processus MCP). Résolution juste avant l'écriture (`set_config`, `write_file`, `edit_file`,
  `set_property`, configs, réglages et fichiers d'un manifeste), erreur avant tout effet sur un nom
  inconnu, **refus pour le Copilote** (`agent_guard::copilot_call`). Toute valeur résolue, créée ou
  chargée est remplacée par sa référence dans les sorties (`redact::register_secret` /
  `mask_known_secrets`, appliqués par `Core::redact_ai` quel que soit le réglage et par
  `mcp::dispatch` sur toute sortie d'outil). Point d'accroche : `Core::resolve_secret_refs`. Outils
  MCP `list_secrets` et `generate_secret` (CSPRNG, `INFRA_TOOLS`) ; ajout d'une valeur et
  suppression dans l'app seulement. Testé sur le backend fichier d'un dossier temporaire.
- **`cli_agent::RunDir`** — dossier éphémère de la config d'un agent CLI (qui peut porter la clé API) :
  privé, effacé à la fin du processus, orphelins purgés. Les outils natifs des agents (shell, disque)
  sont retirés — cf. `COPILOT.md`.
- **`redact`** — `PrivacyConfig` + anonymisation (`redact`) des secrets/IP/e-mails avant envoi à un LLM ;
  valeurs connues des secrets des agents remplacées par `{{secret:NOM}}` (formes brute et échappées).
- **`paste`** — export d'un texte (console, fichier) vers un service public (mclo.gs, instance MineStrator,
  pastes.dev). Contenu **toujours** anonymisé + dé-ANSI avant envoi (indépendant du réglage IA).

---

## 4. Faits d'API / WebSocket qui contraignent l'architecture

Repris de V1 et **re-vérifiés dans le code actuel** (`config.rs`, `api.rs`, `console.rs`).

| Fait | Où / conséquence |
|---|---|
| Base URL `https://mine.sttr.io` | `config::API_BASE_URL`. |
| Auth `Authorization: Bearer <token>` | `api::bearer`. Le panel affiche la clé **déjà encodée base64** : `authenticate()` essaie la valeur brute, puis sa version base64 en secours, et **stocke la forme qui valide** (200 sur `GET /user`). |
| Enveloppe `{ api: { code, endpoint, data } }` | `read_envelope` désenveloppe et renvoie `api.data` (erreur explicite si absent). |
| Erreurs `{ api: { error: "API_…" } }` | mappées en `Error::Api { code }`. |
| Statuts HTTP | `401 → Unauthorized`, `403 → Forbidden`, **`429 → RateLimited` immédiat**. |
| WS `websocket.url` + `token` dans `GET /server/{id}` ; **vides si hiberné** | `run_once` récupère l'URL/token à chaque (re)connexion ; url/token absents → `Outcome::Hibernated` (pas de WS). |
| **`Origin: https://minestrator.com` OBLIGATOIRE** sur le handshake WS (sinon 403) | `config::WS_ORIGIN`, posé par `tokio-tungstenite`. **Impossible depuis un webview → le WS DOIT vivre en Rust.** C'est la justification n°1 de l'architecture. |
| Infra **Pterodactyl / Wings** | events `auth success` → `send logs`+`send stats` ; `status` ; `stats` (~1×/s) ; `console output` ; `install output` (déploiement de l'egg) ; `token expiring`/`token expired`. |
| Jauges | `stats` porte `cpu_absolute` (%), `memory_bytes`/`memory_limit_bytes`, `disk_bytes`, `uptime`, `state`. |

### 4.1 Endpoints vérifiés au banc d'essai (2026-08-19)

Relevés par reverse engineering de l'app mobile officielle (`C:\Users\Geoffrey\minestrator-re`,
`API_SPEC.md`), puis **testés un à un** contre l'API avec une clé du panel. La vérification a compté :
la moitié des endpoints annoncés par le RE n'existent pas pour cette authentification.

| Endpoint | Verdict | Usage dans le core |
|---|---|---|
| `GET /server/{id}/websocket` | OK — `{websocket:{url,token}}`, ~1 Kio | Rafraîchissement de token (`console.rs`), à la place du détail complet du serveur |
| `POST /servers/live/light/bulk` (`{server_ids}`) | OK mais **joueurs seulement** — ni CPU, ni disque, ni statut. Une entrée peut valoir `null` | `Core::live_players_bulk`, pour les compteurs de la liste |
| `GET /server/{id}/activity?limit&offset` | OK — **journal d'audit** paginé : `action_name`, `action_category`, `action_result`, `actor_name` (l'auteur !), `actor_type`, `context` (ex. la commande tapée), `ip`, `is_staff`, `created_at` ; `pagination.total` (2 173 entrées sur un serveur de test) | `Core::server_activity`, section « Activité récente » de l'Aperçu (desktop + mobile) |
| `GET /site/eggs` | OK — 37 eggs (`id`, `name`, `icon`, `resources`, `is_official`) | `Core::list_eggs` (cache 1 h) + table de mapping de `games.rs` |
| `https://cdn.minestrator.com/eggs/server_icon/{icon}` | OK — webp ~1 Kio (`…/server_header/` ≈ 200 Kio) | `config::EGG_ICON_BASE`, composé dans `ServerListItem` |
| `GET /user/servers` | ✗ renvoie le **profil**, pas les serveurs | on garde `/user/{user_id}/servers` |
| `GET /user/servers/websocket` | ✗ 404 | — |
| `GET /user/tickets`, `/user/notifications/unreaded` | ✗ inaccessibles (auth mobile `api_authkey` requise) | — ; les notifications utilisateur sont **déjà** dans `GET /user` (`user.notifications[]`) |
| En-tête `X-App-Token` | ✗ non requis | on ne l'envoie pas : c'est le secret client de l'app officielle |

### 4.2 Endpoints adoptés depuis l'OpenAPI officielle (non vérifiés au banc)

Source : `https://api.minestrator.com/openapi-public-fr.yaml`. Contrairement au §4.1, ces endpoints
n'ont **pas** été testés avec une clé réelle : leurs modèles (`models.rs`) sont **tolérants** (entiers
acceptés en flottant/chaîne, drapeaux `0/1` ou booléens, objet vide sérialisé en `[]` par PHP), et
les usages critiques gardent un **repli** sur le chemin éprouvé.

| Endpoint | Usage dans le core | Repli |
|---|---|---|
| `GET /server/{id}/live` | `Core::server_live` : outil MCP `server_status`, `wait_offline` (installation de mods), Aperçu desktop (FlexCore, réseau, uptime) | `live/light` + échantillon WebSocket (`sample_stats`) |
| `GET /server/{id}/properties` | `Core::server_properties` : vue **Paramètres** desktop, outil MCP `read_properties` (secrets masqués) | — (lecture seule, erreur affichée) |
| `PATCH /server/{id}/properties` (`{property, value}`) | `Core::set_server_property` : une clé à la fois, effet au prochain démarrage ; outil `set_property` | — |
| `GET /mybox/{id}` | `Core::mybox_details` : accueil (renouvellement auto, ressources libres, suppression), outil `mybox_info`, superviseur (pas d'alerte d'expiration si renouvellement auto ; alerte critique de suppression). Lu aussi, avec tolérance, par `Core::mybox_capacity` (module `mybox`) | alerte d'expiration classique si l'appel échoue |
| `POST/PATCH /mybox/{id}/server`, `…/server/disable·enable`, `DELETE /mybox/{id}/server/{id}`, `PATCH /server/{id}/name`, `GET /server/catalog/locations` | module `mybox` : création, redimensionnement, pause/reprise, suppression, renommage, localisations. Plus, non documentés (panel) : `GET /mybox/{id}/disk`, `/site/eggs/experimentals`, `/user/{id}/pimpmybox`, `/mybox/{id}/smartscan`, `PATCH /mybox/{id}/name`, `PATCH /mybox/{id}/servers/order`, `POST …/snapshot-before-delete`, `GET /user/{id}/backup_job/{id}` — voir `docs/api/MYBOX.md` | écritures jamais testées contre l'API réelle : validées localement, corps conformes au panel |

> **Piège de l'API** : un chemin `/user/<inconnu>` ne renvoie **pas** 404 mais le profil complet,
> avec `description: "User - Informations - Get user datas"`. Un endpoint « qui répond 200 » n'est
> donc pas un endpoint qui existe — vérifier la présence des clés attendues dans `data`.
>
> `GET /server/{id}/websocket` répond **403 `API_SERVER_SUSPENDED`** pour un serveur suspendu :
> il ne distingue pas suspension et hibernation, d'où le maintien de `get_server` à l'ouverture
> d'une connexion console (voir le commentaire dans `console.rs::run_once`).

> Le `history[]` de `GET /server/{id}` (date/ip/is_staff/action) est **abandonné** au profit de
> `/activity`, qui nomme l'auteur et porte le contexte. Une seule source pour le journal.
>
> ⚠️ `context` peut contenir ce que l'utilisateur a tapé en console : le passer par
> [`redact`](../crates/minestrator-core/src/redact.rs) avant tout envoi vers un LLM ou un paste.

### 4.3 Mises à jour des mods, plugins et datapacks (endpoints du panel, relevé du 2026-11-18)

Non documentés : formats lus dans le code public du panel web, vérifications appelées pour de vrai
(lecture et comparaison seulement), installations jamais déclenchées depuis l'app pendant le relevé.

| Endpoint | Rôle | Usage dans le core |
|---|---|---|
| `GET /server/{id}/minecraft_version` | `{version, source, confidence}` ; `source` = plateforme (`paper`…) ou `explicit_jar` | version envoyée à la vérification, plateforme de repli du garde-fou |
| `GET /server/{id}/{mods,plugins,datapacks}` | installés : `filename`, `sha1`, `version`, `enabled`, `loader` (mods) ; datapacks : `level_name` + `name` | `ApiClient::content_list` (lecture tolérante) |
| `POST /server/{id}/{type}/updates` | **comparaison sans effet de bord** — le panel l'appelle seul à l'ouverture de ses onglets. Corps `{items, mc_version}` (plugins `{filename, sha1}`, mods `+loader`, datapacks `{name}`) → `{updates: {clé: {has_update, source, target}}, updates_available}`. Liste vide → `API_MISSING_REQUIRED_FIELDS`, sans version → `API_INVALID_MC_VERSION` | `updates::check` |
| `POST /server/{id}/modrinth` `{slug, version_id}` | installation d'un plugin (même appel que le catalogue) | mise à jour d'un plugin |
| `POST /server/{id}/modrinth/mod` `{slug, version_id, loader}`, `POST /server/{id}/curseforge/mod` `{modId, fileId, loader}` | installation d'un mod (panel actuel) | mise à jour d'un mod |
| `POST /server/{id}/{modrinth,curseforge}/datapack` | installation d'un datapack (`{slug, version_id}` / `{modId, fileId}`) | mise à jour d'un datapack |
| `DELETE /server/{id}/{mod,plugin}/{fichier}`, `POST /server/{id}/files/delete` `{files: ["<monde>/datapacks/<nom>"]}` | suppression de l'ancien fichier, seulement si la nouvelle version porte un autre nom | après installation ; un échec donne `old_kept` (deux versions coexistent) |

> **Faux positifs de l'hébergeur** : le corps des plugins ne porte aucun loader, et la vérification
> propose parfois la variante d'une **autre plateforme** à version égale. Relevé réel sur un serveur
> Paper : `LuckPerms-Bukkit-5.5.71.jar` → `LuckPerms-Velocity-5.5.71.jar`,
> `packetevents-spigot-2.14.0.jar` → `packetevents-velocity-2.14.0.jar`. Le panel les afficherait et
> les appliquerait ; `updates::assess` les écarte (plateforme nommée dans le fichier installé, à
> défaut le loader du mod ou la plateforme du serveur ; numéros comparés après retrait de la version
> de Minecraft). Un numéro illisible laisse passer la proposition.
>
> **Installation de mods** : `install_mod` (catalogue) poste toujours `POST /server/{id}/modrinth`
> sans loader, vérifié en juillet 2026, quand `/modrinth/mod` répondait 404. Le panel de novembre
> 2026 utilise `/modrinth/mod` avec `loader` (et `/curseforge/mod`) pour installer **et** mettre à
> jour un mod : les mises à jour suivent le panel ; l'alignement d'`install_mod` reste à vérifier.

**Champs déjà servis par l'API et longtemps ignorés au parsing** (aucun appel supplémentaire) :
`/user/{id}/servers` porte `id_egg`, `last_backup`, `poweraction_message_restart`/`_stop`, `note`,
`tend`, `hashsupport` par serveur, et `is_trial`/`has_bonus`/`tend` par MyBox.

> **Deux champs mentent sur leur nom** (constaté en faisant tourner l'app, 2026-08-19) :
> `note` n'est pas une note libre mais une **clé i18n du panel** décrivant l'indisponibilité
> (`PANEL.SERVERS_LIST.SERVER_IS_DISABLED_MYBOX`) — à traduire, jamais à afficher brut ; et
> `last_backup` n'est pas une date mais l'**URL signée de téléchargement** du dernier backup
> (`https://flash.minestrator.com/get/<jeton>`) — jeton porteur, à ouvrir sans jamais l'afficher.
> D'où les noms `unavailable_reason` et `last_backup_url` côté `ServerListItem`. `/server/{id}` porte
en plus `history[]` (journal d'actions : date, IP, `is_staff`, `action`) et `ports[]` — pistes pour
le « journal d'audit » du ROADMAP.

### 4.4 Statistiques de l'hébergeur (non documenté, relevé le 2026-11-18)

`GET /server/{id}/stats/{début}/{fin}`, lu par `history.rs` (contrat surveillé par
`examples/api_contract.rs`). Relevé avec une clé API, en lectures seules :

| Fait | Conséquence |
|---|---|
| Bornes `AAAA-MM-JJTHH:MM:SS` **en heure de Paris**, changements d'heure compris (grille sans l'heure sautée de mars, heure d'octobre répétée avec les mêmes valeurs) ; date seule, `Z` ou Unix → 404 ; fin avant début → `API_INCORRECT_DATE_FORMAT` | conversion faite en Rust (`paris_label`, `paris_instants`), indépendante du fuseau de la machine |
| Moins de 7 jours : grille **à la minute** (`AAAA-MM-JJ HH:MM:SS`, entiers) ; à partir de 7 jours : **moyennes horaires** des seules minutes mesurées (`AAAA-MM-JJ HH:MM`, flottants), lentes (3 à 10 s pour 30 à 90 jours, 504 vers 365 jours) | l'app ne demande que des minutes, par tranches de 3 jours au plus, et regroupe elle-même |
| La grille est **complétée de zéros** (serveur éteint, minute pas encore écrite, bord de plage non aligné) | un zéro n'est jamais une mesure : ni point de graphe, ni coupure |
| `cpu` en % d'un cœur (comme `cpu_absolute` de Wings), arrondi à l'entier ; `ram` et `disk` en **Mo décimaux** (10⁶ octets — `/live` les donne en Mio) ; `players` | normalisés en octets pour être comparables au local |
| `retention_days: 365`, `data_available_from` ; une borne plus ancienne est ramenée | l'app se limite à 30 jours |
### 4.5 Endpoints fichiers et disque du panel (non documentés)

Format relevé dans le code public du panel web (2026-11-18) ; lectures vérifiées avec une clé API,
écritures **non testées** contre l'API réelle (vrais serveurs de joueurs). Chemins du panel :
relatifs à la racine du serveur, sans `/` initial (`world/region`), même racine que le SFTP.

| Endpoint | Format | Usage dans le core |
|---|---|---|
| `POST /server/{id}/files/compress/jobs` | `{ path: dossier, files: [chemins depuis la racine], name: "x.zip", cwd: dossier, overwrite: false }` → job `{ id, status, progress: { queued, file_count, total_size, elapsed_s }, archive_path, partial, skipped_count, error_message, error_code }` ; refus `API_ZIP_EXISTS`, `API_ZIP_BUSY` | `server_files::compress_request`, `ApiClient::compress_start` |
| `GET …/compress/jobs/{id}?wait=20` | même job ; le serveur ne répond qu'au changement d'état ou au bout de 20 s. `status` : `pending`, `running`, `completed`, `failed` | `follow_job` (90 appels ≈ 30 min, 10 échecs consécutifs tolérés) |
| `GET …/compress/jobs?limit=5` | `{ jobs: [...], count }` (vérifié) | `Core::compress_jobs` (contrat surveillé) |
| `POST /server/{id}/files/extract` | `{ file: "dossier/a.zip", destination: "/dossier" }` → `{ destination, skipped }` (`skipped` : nombre ou objet de compteurs) ; synchrone | `Core::extract_archive` (destination validée comme le panel : pas de `..`, noms réservés, 300 caractères) |
| `GET /server/{id}/disk/usage?limit=&path=` | `{ analyzed_path: "/world", total_size, items_count, items: [{ path: "world/region", name, type: folder\|file, size }] }` trié par taille décroissante, tailles récursives (vérifié ; `path` sans `/` initial, racine si absent ; 20 entrées par défaut) | `Core::disk_usage` |
| `GET /server/{id}/disk/total` | `{ total_bytes, raw_bytes, base_game_bytes, overhead_applied }` : `total = brut − franchise de 500 Mo` quand elle s'applique (vérifié) | `Core::disk_space` ; limite = `live/light` `disk.limit` (Mio, franchise comprise) |
| `GET /mybox/{id}/disk` | `{ disk_servers, disk_servers_unit: "MB" }` : occupation de tous les serveurs (vérifié) | `Core::disk_space`, face au disque de l'offre (`disk_total`, Go) |

**Séquence WebSocket (`console.rs`).** `get_server` → connexion `wss://…` avec `Origin` → `{"event":"auth","args":[token]}` → sur `auth success`, `send logs` (sauf mode monitor) + `send stats` → relais des events en `CoreEvent`. Sur `token expiring`/`token expired` : re-`get_server` et ré-`auth` avec le token frais. `Ping`→`Pong`. Sur close/erreur : **reconnexion à backoff** (1 s → ×2 → **30 s max**), remis à 1 s si la session a tenu ≥ 30 s (jugée saine). Une **connexion monitor** (superviseur) ne demande pas les logs et ne relaie pas `console output` : elle ne pousse que stats/status et repère les lignes WARN/ERROR pour le Copilote.

**Envoi via REST, WS en lecture seule.** Commandes et power actions passent par les endpoints REST
(`PUT /server/{id}/command`, `/poweraction`), jamais par le WS : contrat stable, permissions homogènes.
Le WS ne sert qu'à **recevoir** output + stats + status.

> **Écart assumé avec V1 :** le *backoff HTTP 429 + file de requêtes* imaginé en V1 **n'est pas implémenté**.
> Un `429` remonte immédiatement en `Error::RateLimited` (message FR côté UI). Seule la **reconnexion WS**
> a un backoff.

---

## 5. IPC (front ↔ Rust)

### 5.1 Commandes (`desktop/src-tauri/src/commands.rs`, enregistrées dans `lib.rs`)

Les commandes `#[tauri::command]` (une centaine), appelées **uniquement** via la couche typée `src/lib/ipc.ts`
(aucun composant n'appelle `invoke` en direct). Groupées par domaine :

| Domaine | Commandes (extraits) |
|---|---|
| **Auth & comptes** | `validate_and_store_key` (connexion : la clé devient celle du compte actif), `has_stored_key`, `get_user` (profil + mise en cohérence de la liste des comptes), `logout` (retire le compte actif, renvoie celui qui prend la relève), `accounts_list`, `account_add`, `account_switch`, `account_remove` |
| **Serveurs** | `list_servers`, `server_details`, `live_light`, `server_activity` (journal d'audit), `live_players_bulk` (joueurs de N serveurs en 1 appel), `list_eggs`, `metrics_history` (historique croisé local + hébergeur, `MetricsHistory`), `get_startup`/… |
| **MyBox** | `mybox_details`, `mybox_capacities`, `mybox_capacity`, `server_catalog`, `mybox_create_server`, `mybox_resize_server`, `mybox_set_server_paused`, `mybox_reorder_servers`, `rename_server`, `rename_mybox`, `mybox_snapshot_before_delete`, `backup_job`, `mybox_delete_server` (nom exact exigé), `mybox_plan_placement` |
| **Console / power** | `console_logs`, `power_action`, `send_command`, `console_connect`, `console_disconnect` |
| **Joueurs** | `player_action` (kick / ban / unban / op_add·remove / whitelist_add·remove), `known_players` (joueurs déjà venus : recherche, filtre, tri, page, statistiques), `player_profile`, `player_inventory` (lecture seule) |
| **SFTP & fichiers** | `sftp_list/read_text/write_text/mkdir/delete/rename`, transferts `sftp_upload/download/download_zip` (archive construite par le serveur, repli local), `cancel_transfer`, archives/NBT/MCA `sftp_archive_*`, `sftp_gz_text`, `sftp_read_data_uri`, `sftp_nbt_tree/snbt`, `sftp_region_chunk*`, `sftp_inspect_region`, `sftp_search`, `sftp_disconnect` |
| **Fichiers côté serveur** | `server_compress` (archive conservée, tâche de fond), `server_extract`, `disk_usage`, `disk_space` ; mobile en plus : `server_compress_temp` + `remove_temp_archive` (téléchargement d'un dossier) |
| **Backups / snapshots** | `list_backups`, `restore_backup`, `list_snapshots`, `create_snapshot`, `restore_snapshot`, `delete_snapshot` |
| **Marketplace** | `market_minecraft_versions`, `market_list`, `market_versions`, `install_mod`, `installed_mods`, `installed_plugins`, `installed_datapacks` |
| **Mises à jour de contenu** | `content_updates` (vérification gardée 15 min, `force`), `apply_content_updates` (sélection `{kind, key}` → `UpdateReport`) |
| **Copilote / chat** | `copilot_apply`, `copilot_diagnose_now`, `copilot_analyze`, `copilot_performance`, `chat_send`, `chat_reset`, `chat_warm`, `has/set/clear_copilot_key` |
| **Configs** | `get/set_supervisor_config`, `get/set_mcp_config`, `get/set_privacy_config`, `get/set_copilot_config` |
| **Automatisations / disponibilité** | `server_automations`, `set_auto_restart`, `save_scheduled_task`, `delete_scheduled_task`, `automation_runs`, `uptime_report` |
| **Alertes externes** | `list_webhooks`, `save_webhook` (URL → stockage sécurisé), `delete_webhook`, `test_webhook` |
| **Garde des agents MCP** | `agent_actions` (journal filtré), `agent_changesets`, `rollback_changeset` (après confirmation dans l'UI) ; portée des écritures dans `McpConfig` |
| **Secrets des agents** | `agent_secrets` (noms, descriptions, dates : `AgentSecret`), `add_agent_secret` (valeur → stockage sécurisé, jamais relue), `generate_agent_secret` (longueur, `SecretCharset`), `delete_agent_secret` (après confirmation dans l'UI) |
| **Divers / paste / MCP** | `paste_upload`, `detect_clis`, `app_exe_path` (compose la config MCP de Claude) |

> **Updater / version** : côté **front** via les plugins Tauri (`updater.ts` : `check` / `downloadAndInstall`
> + `relaunch`, `UpdateBanner.svelte`), pas via des commandes métier. Côté Rust, `src-tauri` enregistre
> `tauri-plugin-updater` + `tauri-plugin-process` ; la signature des releases GitHub est vérifiée avec la
> `pubkey` de `tauri.conf.json`.

### 5.2 Events (Rust → webview)

Le cœur publie des `CoreEvent` sur un canal `broadcast`. Le pont de `desktop/src-tauri/src/lib.rs` (`forward`) les
relaie en events Tauri, et transforme les **alertes** + **diagnostics** en **notifications natives**. Les
events console sont **taggés par `conn_id`** (pas par `server_id`) : plusieurs onglets/fenêtres peuvent
observer le même serveur indépendamment.

| Event Tauri | Payload (tag) | Usage |
|---|---|---|
| `console://output` | `{ conn_id, line }` | append xterm |
| `console://stats` | `{ conn_id, cpu_absolute, memory_bytes, memory_limit_bytes, disk_bytes, uptime, state }` | jauges |
| `console://status` | `{ conn_id, state }` | badge d'état |
| `console://connection` | `{ conn_id, phase }` | `connecting \| open \| reconnecting \| closed \| hibernated \| suspended` |
| `alert://new` | `{ server_id, kind, severity, message, ts, details? }` (`kind` : `crash`, `cpu`, `ram`, `disk`, `expiry`, `auto_restart`, `task`) | bandeau + **notif native** ; `details` = logs du crash / erreur d'une tâche, dépliables dans le centre d'alertes |
| `copilot://started` | `{ id, server_id, trigger }` | indicateur « analyse en cours » |
| `copilot://progress` | `{ id, phase }` | log de progression |
| `copilot://diagnosis` | `Diagnosis` (cause, fix, actions proposées) | rapport + **notif native** |
| `chat://delta` | `{ id = session_id, text }` | streaming des réponses assistant |
| `sftp://progress` | `{ id = transferId, name, direction, done, total, status, error, phase, files, skipped }` (`direction` : `up`, `down`, `server` ; `phase` : `queued`, `compressing`, `extracting`, `downloading`, `local_zip`, `cleanup`, en fin `still_running`) | gestionnaire de transferts (y compris compression et extraction faites par le serveur) |
| `mods://install-progress` | `ModInstallProgress` (`{ id, phase, mod_name, done, total, status, error }`) | suivi d'une installation de mods par lot |
| `account://switched` | `AccountSwitch` (`{ previous, current }`, `current = null` : plus aucun compte) | toutes les fenêtres ferment les onglets serveurs et se rechargent sur le nouveau compte |

*(`CoreEvent::ConsoleLog` est interne — déclencheur Copilote — et n'est pas relayé au webview.)*

### 5.3 Types partagés : générés depuis Rust (ts-rs)

Les types qui traversent l'IPC (réponses de commandes, payloads d'events ci-dessus, arguments
structurés) ne sont **pas** écrits à la main en TypeScript : ils sont générés depuis les structs serde
par [`ts-rs`](https://docs.rs/ts-rs) 12, puis **versionnés**. Une dérive (champ serde renommé, `Option`
ajoutée…) casse la CI au lieu de casser l'app à l'exécution.

| Fichier généré | Source | Ré-exporté par |
|---|---|---|
| `desktop/src/lib/bindings/core.ts` | `minestrator_core::bindings::core()` | `desktop/src/lib/types.ts` (`export type *`) |
| `mobile/src/lib/bindings/core.ts` | idem (contenu identique) | `mobile/src/lib/types.ts` |
| `mobile/src/lib/bindings/mobile.ts` | `mobile/src-tauri/src/bindings.rs` (`UpdateInfo`, `SftpSearchResult`) | `mobile/src/lib/types.ts` |

- **Dérivation optionnelle** : `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]`. La feature `ts` (core et
  crate mobile) n'est activée que par les alias de génération : ts-rs n'entre dans aucun binaire livré, et
  le core ne gagne aucune dépendance d'UI.
- **Forme JSON exacte** : ts-rs lit les attributs serde (`rename`, `rename_all`, `tag`, `default`,
  `flatten`…). `Option<T>` → `T | null` (champ présent, à `null`). Un `skip_serializing_if` **sans**
  `default` (champ réellement omis) exige `ts(optional)` → `?:` (`NbtNode`). `serde_json::Value` →
  `JsonValue`. `Error`, à sérialisation manuelle, passe par `ErrorPayload` (`AppError`) et `ErrorKind`
  (`AppErrorKind`).
- **Entiers 64 bits en `number`** (`Config::with_large_int("number")`) : c'est ce que `JSON.parse`
  produit réellement, pas un `bigint`.
- `types.ts` ne garde que les unions purement front (`PowerAction`, `PlayerAction`, `MarketKind`,
  `MarketSource` : de simples chaînes côté Rust).
- **`cargo bindings`** régénère ; **`cargo bindings-check`** (CI) échoue si un fichier versionné ne
  correspond plus au Rust. Les alias vivent dans `.cargo/config.toml`.

---

## 6. Frontend (SvelteKit + Svelte 5)

**Adapter statique**, Svelte 5 en **runes** (`$state`/`$derived`/`$effect`), TypeScript.

**Shell « workspace à onglets », pas de routes par serveur.** L'app authentifiée tient sur **une seule
route** (`/`) qui rend une barre d'onglets + une pile de panneaux **keep-alive** : les panneaux inactifs
restent montés (masqués en `display`), ce qui préserve l'état vivant (WS console, scrollback, connexion
SFTP) quand on change d'onglet. Une route dédiée les démonterait.

- **`tabs/tabs.svelte.ts`** — `TabManager` (store rune). Onglet **Home permanent** + N onglets, plus des
  onglets uniques **Réglages**, **Copilote**, **Notifications** et **MyBox** (`openMyboxes`). Un onglet serveur cible un couple `(serverId, view)`.
  **Doublons autorisés** (`openNew` vs `focusOrOpen`), plus `close`/`closeOthers`/`closeLeft`/`closeRight`
  et `moveTo` (réordonnancement par glisser). `ServerView` = `overview \| console \| sftp \| players \|
  mods \| assistant \| backups \| automations \| settings`.
- **`windows.ts`** — **fenêtres détachées** : `detachTab` ouvre une `WebviewWindow` (`/?detach=…`) ;
  chaque fenêtre est une instance indépendante avec son propre `TabManager`, l'auth étant partagée via le
  trousseau OS. `readDetachSpec` (dans `+page.svelte`) rouvre l'onglet demandé ; `isPointerOutsideWindow`
  détecte le glisser-hors-fenêtre depuis la `TabBar`.
- **Palette de commandes** (Ctrl+K / ⌘K, `components/CommandPalette.svelte`) — `<dialog>` modal qui
  pilote le `TabManager` (aucune route) ; recherche floue pure dans `palette/fuzzy.ts`. Elle n'a aucune
  entrée en dur : une fonctionnalité s'y ajoute via `palette/registry.svelte.ts` —
  `registerPaletteSource` (section de la racine), `registerServerEntries` (entrées par serveur, dans
  sa sous-page et en recherche) ou `registerPaletteMode` (préfixe, ex. `>` commande console) ; sources
  intégrées et exemple dans `palette/sources.svelte.ts`. Alimentation : mêmes garde-fous que les
  boutons via `servers/power.ts`. Récents = identifiants seuls (localStorage).
- **Garde d'auth** — `routes/+layout.svelte` : au boot, `has_stored_key` puis `get_user` valident la clé ;
  un `$effect` redirige entre `/onboarding` et `/`. `logout` retire le compte actif : l'app bascule
  sur un autre compte enregistré, ou revient à l'onboarding (qui propose aussi les comptes restants).
- **Comptes** — `stores/accounts.svelte.ts` (`applyAccountSwitch`, idempotent : appelé par l'action
  et par l'event `account://switched`) ferme les onglets serveurs et recharge le profil ;
  `+page.svelte` remonte le workspace par `{#key}` sur l'id du compte, donc toutes les listes se
  rechargent. Sélecteur dans `UserMenu`, `AddAccountDialog`, Réglages → Comptes
  (`settings/AccountsSection.svelte`, section mémorisée par `settings/nav.svelte.ts`, partagé avec la palette).
- **Vues** (`src/lib/components/views/`) — `ServerOverview` (jauges + graphes d'historique
  1 h → 30 j, `MetricsChart` en trait plein pour le local et pointillé pour l'hébergeur,
  `PlayersChart` des joueurs connectés + `UptimeCard` : disponibilité 24 h / 7 j / 30 j, part de
  l'hébergeur et coupures), `ConsoleView`
  (xterm + envoi de commandes), `SftpView` (+ `FileEditor` CodeMirror, archives, NBT, `.mca`,
  compression/extraction par le serveur, `DiskUsagePanel` « Espace disque », ouvrable aussi depuis
  l'Aperçu via `sftp/diskRequest.svelte.ts`), `PlayersView`,
  `MarketplaceView` (mods/plugins, mises à jour avec confirmation et rappel du redémarrage ; état
  partagé avec le compteur de l'Aperçu par `mods/updates.svelte.ts`), `AssistantView` (chat Copilote streamé), `BackupsView`
  (backups + snapshots), `AutomationsView` (redémarrage automatique, tâches planifiées, historique
  d'exécution — en précisant qu'elles tournent sur cet ordinateur, pas sur le panel web),
  `SettingsView` (dont `settings/ServiceSection`, `settings/WebhooksSection` et, sous MCP,
  `settings/AgentSecretsSection`), `CopilotView`,
  `ComingSoon` (placeholder).
- **Couches transverses** — `ipc.ts` (façade `api` typée + `humanizeError`/i18n), `events.ts`
  (abonnements typés), `types.ts` (ré-export des types générés de `bindings/`, cf. §5.3), stores rune (`stores/auth.svelte.ts`, `servers/colors.svelte`, `sftp/columns.svelte`),
  `theme.ts` (clair/sombre), `i18n` (fr/en), `updater.ts` + `UpdateBanner.svelte`.

**UX desktop.** Fenêtre en **tray** : fermer la fenêtre la **masque** (le superviseur continue en fond) ;
menu Afficher/Quitter + clic gauche = réafficher. Notifications natives pour alertes et diagnostics.

---

## 7. Dépendances clés

**`minestrator-core`** — `serde`/`serde_json` ; `reqwest` (`rustls-tls`, sans OpenSSL) + `base64` ;
`tokio` (rt/sync/macros/time/fs/io-util/**io-std**/**process**) ; `tokio-tungstenite`
(`rustls-tls-webpki-roots`) + `futures-util` ; **`russh` 0.65** (backend `ring`, clés d'hôte RSA ;
pas `aws-lc-rs`, qui exige CMake/NASM) **/ `russh-sftp` 3** ; **`rusqlite` 0.43 *bundled*** +
`directories` ; **`chrono`** (`clock` : heure locale du planificateur d'automatisations, déjà
présent via `russh-sftp`) ; **`prost` 0.13** (rapport Spark) ; `flate2`/`tar`/`zip` 9 (deflate **pur Rust**) ;
**`fastnbt`** ; `similar` (diff de `diff_files`, Apache-2.0) ; `toml` 1 (lecture des `mods.toml`, déjà présent via Tauri) ; `thiserror` ; `tracing` + `tracing-subscriber`/`tracing-appender` (journaux). **`keyring` 4** par OS
(`windows-native` / `apple-native` / `zbus-secret-service`), **`android-native-keyring-store`** +
`keyring-core` sur Android (Keystore). **`ts-rs` 12** *optionnel* (feature `ts`,
génération des bindings TypeScript uniquement).

**`src-tauri`** — `tauri` 2 (feature `tray-icon`) ; plugins `opener`, `dialog`, `notification`,
`updater`, `process` ; `minestrator-core` ; `tokio` (sync, rt-multi-thread, io-std, io-util) ;
`serde_json` ; `tracing`/`tracing-subscriber`.

**`minestrator-mcp`** — `minestrator-core` ; `tokio` (rt-multi-thread, macros) ; `tracing`.

**Front (`package.json`)** — `svelte` 5, `@sveltejs/kit` 2 (`adapter-static`), `vite` 6, `typescript` ;
`@tauri-apps/api` 2 + plugins `dialog`/`opener`/`process`/`updater` ; **`@xterm/xterm` 5.5 +
`@xterm/addon-fit`** (console) ; **CodeMirror 6** (`@codemirror/{state,view,commands,language,search}`,
`legacy-modes`, `theme-one-dark` + langages css/html/java/javascript/json/markdown/xml/yaml) pour
l'éditeur de fichiers.
