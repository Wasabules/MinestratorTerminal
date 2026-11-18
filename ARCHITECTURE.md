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
  ré-auth sur `token expiring`/`token expired`. Publie des `CoreEvent`.
- **`sftp`** — client SFTP natif (`russh`/`russh-sftp`) : `SftpManager` à **sessions poolées** par serveur
  (rouvertes à la volée si empoisonnées), list/read/write/mkdir/delete/rename/upload/download/walk/search.
  Hôte/port/utilisateur viennent de `GET /server/{id}` ; le **mot de passe n'y figure plus** (retiré
  par MineStrator fin 2026, toujours documenté) : saisi une fois par serveur, vérifié par une session
  avant d'être enregistré (`set_sftp_password`), puis essayé avant celui de l'API si elle en renvoie
  un. Sans mot de passe : erreur `sftp_password_required`, que l'UI traduit en saisie.
- **`models`** — structs `serde` : miroirs typés des réponses d'API et de leurs enveloppes brutes
  (`UserProfile`, `ServersOverview`/`MyBoxSummary`, `ServerDetails`, `LiveLight`, `Startup`, `MarketPage`,
  `Backup`, `Snapshot`, `InstalledItem`, `SftpEntry`…).
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
- **`nbt`** — décodage NBT (`fastnbt`) `.dat`/`level.dat`/playerdata/`.schem` → arbre typé `NbtNode` + SNBT.
- **`mca`** — validation/réparation des régions Anvil `.mca` (table de localisation, chunks corrompus).
- **`world`** — outils monde de plus haut niveau (`inspect_region`, coordonnées de région, réparation).

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
  surveillées (app fermée, PC éteint) ne comptent pas ; fusion avec l'historique du service.
- **`store`** — historique local de métriques en **SQLite embarqué** (`rusqlite` *bundled*). L'API ne
  fournit aucun historique : l'app l'accumule elle-même (avantage structurel d'un client persistant).
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
- **`official_mcp`** — client du **serveur MCP OFFICIEL** de MineStrator (`https://mcp.sttr.io/minestrator`,
  Streamable HTTP stateless, `Bearer <clé API>`) : une centaine d'outils de gestion (107 annoncés fin
  2026, cf. minestrator.com/connecter/votre/ia) délégués à l'hôte. Notre MCP local
  garde le SFTP fin + les outils exclusifs (`inspect_region`, Spark, docteur démarrage).

### 3.5 Sécurité & confidentialité
- **`secrets`** — stockage sécurisé natif : clé API, clés LLM par fournisseur (`llm-key-<slug>`),
  secrets par jeu, mots de passe SFTP par serveur (`sftp-password-<id>`), URL des destinations
  d'alertes (`webhook-url-<id>` : une URL Discord suffit à poster). Trousseau de l'OS
  (`keyring`) sur desktop ; **Keystore Android** (`android-native-keyring-store` : AES-GCM, clé non
  exportable) sur Android, avec migration puis suppression de l'ancien `secrets.json` en clair.
  Backend **fichier** réservé au daemon (et à iOS, non livré) : dossier à accès restreint, jamais le
  répertoire temporaire, `0600` sous Unix, **chiffré DPAPI** (compte du service) sous Windows.
- **`cli_agent::RunDir`** — dossier éphémère de la config d'un agent CLI (qui peut porter la clé API) :
  privé, effacé à la fin du processus, orphelins purgés. Les outils natifs des agents (shell, disque)
  sont retirés — cf. `COPILOT.md`.
- **`redact`** — `PrivacyConfig` + anonymisation (`redact`) des secrets/IP/e-mails avant envoi à un LLM.
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
| `GET /mybox/{id}` | `Core::mybox_details` : accueil (renouvellement auto, ressources libres, suppression), outil `mybox_info`, superviseur (pas d'alerte d'expiration si renouvellement auto ; alerte critique de suppression) | alerte d'expiration classique si l'appel échoue |

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
| **Auth** | `validate_and_store_key`, `has_stored_key`, `get_user`, `logout` |
| **Serveurs** | `list_servers`, `server_details`, `live_light`, `server_activity` (journal d'audit), `live_players_bulk` (joueurs de N serveurs en 1 appel), `list_eggs`, `metrics_history`, `get_startup`/… |
| **Console / power** | `console_logs`, `power_action`, `send_command`, `console_connect`, `console_disconnect` |
| **Joueurs** | `player_action` (kick / ban / unban / op_add·remove / whitelist_add·remove) |
| **SFTP & fichiers** | `sftp_list/read_text/write_text/mkdir/delete/rename`, transferts `sftp_upload/download/download_zip`, archives/NBT/MCA `sftp_archive_*`, `sftp_gz_text`, `sftp_read_data_uri`, `sftp_nbt_tree/snbt`, `sftp_region_chunk*`, `sftp_inspect_region`, `sftp_search`, `sftp_disconnect` |
| **Backups / snapshots** | `list_backups`, `restore_backup`, `list_snapshots`, `create_snapshot`, `restore_snapshot`, `delete_snapshot` |
| **Marketplace** | `market_minecraft_versions`, `market_list`, `market_versions`, `install_mod`, `installed_mods`, `installed_plugins` |
| **Copilote / chat** | `copilot_apply`, `copilot_diagnose_now`, `copilot_analyze`, `copilot_performance`, `chat_send`, `chat_reset`, `chat_warm`, `has/set/clear_copilot_key` |
| **Configs** | `get/set_supervisor_config`, `get/set_mcp_config`, `get/set_privacy_config`, `get/set_copilot_config` |
| **Automatisations / disponibilité** | `server_automations`, `set_auto_restart`, `save_scheduled_task`, `delete_scheduled_task`, `automation_runs`, `uptime_report` |
| **Alertes externes** | `list_webhooks`, `save_webhook` (URL → stockage sécurisé), `delete_webhook`, `test_webhook` |
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
| `sftp://progress` | `{ id = transferId, name, direction, done, total, status, error }` | gestionnaire de transferts |
| `mods://install-progress` | `ModInstallProgress` (`{ id, phase, mod_name, done, total, status, error }`) | suivi d'une installation de mods par lot |

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
  onglets uniques **Réglages** et **Copilote**. Un onglet serveur cible un couple `(serverId, view)`.
  **Doublons autorisés** (`openNew` vs `focusOrOpen`), plus `close`/`closeOthers`/`closeLeft`/`closeRight`
  et `moveTo` (réordonnancement par glisser). `ServerView` = `overview \| console \| sftp \| players \|
  mods \| assistant \| backups \| automations \| settings`.
- **`windows.ts`** — **fenêtres détachées** : `detachTab` ouvre une `WebviewWindow` (`/?detach=…`) ;
  chaque fenêtre est une instance indépendante avec son propre `TabManager`, l'auth étant partagée via le
  trousseau OS. `readDetachSpec` (dans `+page.svelte`) rouvre l'onglet demandé ; `isPointerOutsideWindow`
  détecte le glisser-hors-fenêtre depuis la `TabBar`.
- **Garde d'auth** — `routes/+layout.svelte` : au boot, `has_stored_key` puis `get_user` valident la clé ;
  un `$effect` redirige entre `/onboarding` et `/`. `logout` efface la clé et ramène à l'onboarding.
- **Vues** (`src/lib/components/views/`) — `ServerOverview` (jauges + graphes d'historique +
  `UptimeCard` : disponibilité 24 h / 7 j / 30 j et coupures), `ConsoleView`
  (xterm + envoi de commandes), `SftpView` (+ `FileEditor` CodeMirror, archives, NBT, `.mca`), `PlayersView`,
  `MarketplaceView` (mods/plugins), `AssistantView` (chat Copilote streamé), `BackupsView`
  (backups + snapshots), `AutomationsView` (redémarrage automatique, tâches planifiées, historique
  d'exécution — en précisant qu'elles tournent sur cet ordinateur, pas sur le panel web),
  `SettingsView` (dont `settings/ServiceSection` et `settings/WebhooksSection`), `CopilotView`,
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
**`fastnbt`** ; `thiserror` ; `tracing` + `tracing-subscriber`/`tracing-appender` (journaux). **`keyring` 4** par OS
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
