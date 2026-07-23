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
members = ["desktop/src-tauri", "crates/minestrator-core", "crates/minestrator-mcp"]
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
- **`models`** — structs `serde` : miroirs typés des réponses d'API et de leurs enveloppes brutes
  (`UserProfile`, `ServersOverview`/`MyBoxSummary`, `ServerDetails`, `LiveLight`, `Startup`, `MarketPage`,
  `Backup`, `Snapshot`, `InstalledItem`, `SftpEntry`…).
- **`config`** — constantes (URLs, `Origin` Wings, comptes trousseau, version Anthropic, variables d'env MCP).
- **`error`** — `Error` unifié, sérialisé `{ kind, message }` pour un traitement par cas côté frontend.
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
  Réglable via `SupervisorConfig` persistée ; état vivant dans `SupervisorState`.
- **`store`** — historique local de métriques en **SQLite embarqué** (`rusqlite` *bundled*). L'API ne
  fournit aucun historique : l'app l'accumule elle-même (avantage structurel d'un client persistant).
- **`cache`** — cache mémoire TTL des lectures d'API stables/semi-stables (au niveau du `Core`).
- **`persist`** — config JSON persistée (load/get/set sous mutex) dans le dossier data de l'OS ;
  mutualise MCP, Copilote et confidentialité.
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
  Streamable HTTP stateless, `Bearer <clé API>`) : ~60 outils de gestion délégués à l'hôte. Notre MCP local
  garde le SFTP fin + les outils exclusifs (`inspect_region`, Spark, docteur démarrage).

### 3.5 Sécurité & confidentialité
- **`secrets`** — trousseau natif de l'OS (`keyring`) : clé API + clés LLM par fournisseur
  (`llm-key-<slug>`). Fonctionne aussi en headless.
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
| Infra **Pterodactyl / Wings** | events `auth success` → `send logs`+`send stats` ; `status` ; `stats` (~1×/s) ; `console output` ; `token expiring`/`token expired`. |
| Jauges | `stats` porte `cpu_absolute` (%), `memory_bytes`/`memory_limit_bytes`, `disk_bytes`, `uptime`, `state`. |

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

~70 commandes `#[tauri::command]`, appelées **uniquement** via la couche typée `src/lib/ipc.ts`
(aucun composant n'appelle `invoke` en direct). Groupées par domaine :

| Domaine | Commandes (extraits) |
|---|---|
| **Auth** | `validate_and_store_key`, `has_stored_key`, `get_user`, `logout` |
| **Serveurs** | `list_servers`, `server_details`, `live_light`, `metrics_history`, `get_startup`/… |
| **Console / power** | `console_logs`, `power_action`, `send_command`, `console_connect`, `console_disconnect` |
| **Joueurs** | `player_action` (kick / ban / unban / op_add·remove / whitelist_add·remove) |
| **SFTP & fichiers** | `sftp_list/read_text/write_text/mkdir/delete/rename`, transferts `sftp_upload/download/download_zip`, archives/NBT/MCA `sftp_archive_*`, `sftp_gz_text`, `sftp_read_data_uri`, `sftp_nbt_tree/snbt`, `sftp_region_chunk*`, `sftp_inspect_region`, `sftp_search`, `sftp_disconnect` |
| **Backups / snapshots** | `list_backups`, `restore_backup`, `list_snapshots`, `create_snapshot`, `restore_snapshot`, `delete_snapshot` |
| **Marketplace** | `market_minecraft_versions`, `market_list`, `market_versions`, `install_mod`, `installed_mods`, `installed_plugins` |
| **Copilote / chat** | `copilot_apply`, `copilot_diagnose_now`, `copilot_analyze`, `copilot_performance`, `chat_send`, `chat_reset`, `chat_warm`, `has/set/clear_copilot_key` |
| **Configs** | `get/set_supervisor_config`, `get/set_mcp_config`, `get/set_privacy_config`, `get/set_copilot_config` |
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
| `console://connection` | `{ conn_id, phase }` | `connecting \| open \| reconnecting \| closed \| hibernated` |
| `alert://new` | `{ server_id, kind, severity, message, ts }` | bandeau + **notif native** |
| `copilot://started` | `{ id, server_id, trigger }` | indicateur « analyse en cours » |
| `copilot://progress` | `{ id, phase }` | log de progression |
| `copilot://diagnosis` | `Diagnosis` (cause, fix, actions proposées) | rapport + **notif native** |
| `chat://delta` | `{ id = session_id, text }` | streaming des réponses assistant |
| `sftp://progress` | `{ id = transferId, name, direction, done, total, status, error }` | gestionnaire de transferts |

*(`CoreEvent::ConsoleLog` est interne — déclencheur Copilote — et n'est pas relayé au webview.)*

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
  mods \| assistant \| backups \| settings` (`settings` par-serveur non encore *ready*).
- **`windows.ts`** — **fenêtres détachées** : `detachTab` ouvre une `WebviewWindow` (`/?detach=…`) ;
  chaque fenêtre est une instance indépendante avec son propre `TabManager`, l'auth étant partagée via le
  trousseau OS. `readDetachSpec` (dans `+page.svelte`) rouvre l'onglet demandé ; `isPointerOutsideWindow`
  détecte le glisser-hors-fenêtre depuis la `TabBar`.
- **Garde d'auth** — `routes/+layout.svelte` : au boot, `has_stored_key` puis `get_user` valident la clé ;
  un `$effect` redirige entre `/onboarding` et `/`. `logout` efface la clé et ramène à l'onboarding.
- **Vues** (`src/lib/components/views/`) — `ServerOverview` (jauges + graphes d'historique), `ConsoleView`
  (xterm + envoi de commandes), `SftpView` (+ `FileEditor` CodeMirror, archives, NBT, `.mca`), `PlayersView`,
  `MarketplaceView` (mods/plugins), `AssistantView` (chat Copilote streamé), `BackupsView`
  (backups + snapshots), `SettingsView`, `CopilotView`, `ComingSoon` (placeholder).
- **Couches transverses** — `ipc.ts` (façade `api` typée + `humanizeError`/i18n), `events.ts`
  (abonnements typés), stores rune (`stores/auth.svelte.ts`, `servers/colors.svelte`, `sftp/columns.svelte`),
  `theme.ts` (clair/sombre), `i18n` (fr/en), `updater.ts` + `UpdateBanner.svelte`.

**UX desktop.** Fenêtre en **tray** : fermer la fenêtre la **masque** (le superviseur continue en fond) ;
menu Afficher/Quitter + clic gauche = réafficher. Notifications natives pour alertes et diagnostics.

---

## 7. Dépendances clés

**`minestrator-core`** — `serde`/`serde_json` ; `reqwest` (`rustls-tls`, sans OpenSSL) + `base64` ;
`tokio` (rt/sync/macros/time/fs/io-util/**io-std**/**process**) ; `tokio-tungstenite`
(`rustls-tls-webpki-roots`) + `futures-util` ; **`russh` 0.45 / `russh-sftp` 2.0** (+ `async-trait`) ;
**`rusqlite` *bundled*** + `directories` ; **`prost` 0.13** (rapport Spark) ; `flate2`/`tar`/`zip`
(deflate **pur Rust**) ; **`fastnbt`** ; `thiserror` ; `tracing`. **`keyring` 3** par OS
(`windows-native` / `apple-native` / `sync-secret-service`).

**`src-tauri`** — `tauri` 2 (feature `tray-icon`) ; plugins `opener`, `dialog`, `notification`,
`updater`, `process` ; `minestrator-core` ; `tokio` (sync, rt-multi-thread, io-std, io-util) ;
`serde_json` ; `tracing`/`tracing-subscriber`.

**`minestrator-mcp`** — `minestrator-core` ; `tokio` (rt-multi-thread, macros) ; `tracing`.

**Front (`package.json`)** — `svelte` 5, `@sveltejs/kit` 2 (`adapter-static`), `vite` 6, `typescript` ;
`@tauri-apps/api` 2 + plugins `dialog`/`opener`/`process`/`updater` ; **`@xterm/xterm` 5.5 +
`@xterm/addon-fit`** (console) ; **CodeMirror 6** (`@codemirror/{state,view,commands,language,search}`,
`legacy-modes`, `theme-one-dark` + langages css/html/java/javascript/json/markdown/xml/yaml) pour
l'éditeur de fichiers.
