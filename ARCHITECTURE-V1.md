# Minestrator Terminal — Architecture V1

> Client desktop léger pour piloter ses serveurs Minecraft via l'API MineStrator.
> Stack : **Tauri 2 (Rust)** + **SvelteKit** + **xterm.js**.
> Objectif V1 : *moniteur + console temps réel* — voir l'état d'un serveur, lire la console live, envoyer des commandes et des power actions.

---

## 0. Principes directeurs

1. **Le réseau vit dans Rust, pas dans le webview.** Tous les appels HTTP et le WebSocket sont gérés côté Rust. Le front (SvelteKit) ne voit jamais la clé API et ne parle qu'à des commandes Tauri. Bénéfices : clé jamais exposée au JS, pas de problème CORS, WebSocket robuste, perf.
2. **Console-first.** L'écran serveur est construit autour de la console live et des jauges temps réel.
3. **Léger et instantané.** Binaire Tauri (~5–15 Mo), démarrage quasi immédiat, RAM minimale.
4. **Dégrader proprement.** Serveur hiberné / suspendu / expiré, clé invalide, hors-ligne, rate limit, sous-utilisateur sans permission : chaque état a un rendu défini.

---

## 1. Faits d'API qui contraignent l'archi

| Fait | Conséquence architecture |
|---|---|
| Base URL `https://mine.sttr.io` | Configurable (const), pas en dur dans le front. |
| Auth `Authorization: Bearer <token>` | ⚠️ **Le panel affiche la clé déjà encodée en base64** (vérifié : `Bearer QWQ2…=` → 200 ; ré-encoder → 401). `authenticate()` essaie la valeur telle quelle, puis base64 en secours, et stocke la forme qui valide. |
| Enveloppe `{ api: { code, endpoint, data } }` | Une couche de désenveloppage centralisée renvoie `data` au front. |
| Erreurs `{ api: { error: "API_...", data } }` | Mapping code d'erreur → message FR + action (ex. `API_SERVER_SUSPENDED`). |
| `429 Too Many Requests` (limites non chiffrées) | Backoff exponentiel + file de requêtes côté Rust ; respecter `Retry-After` si présent. |
| **`live/light` = limites seulement** ; usage instantané via WS | Les jauges CPU/RAM ont besoin du flux `stats` du WebSocket. |
| WS `url` + `token` dans `GET /server/{id}` ; vides si hiberné | Récupérer la connexion à l'ouverture de l'écran serveur ; pas de WS si hibernation. |
| **WS exige `Origin: https://minestrator.com`** (✅ vérifié : sinon 403) | En-tête posé côté Rust ; impossible depuis un webview → WS obligatoirement en Rust. |
| Infra **Pterodactyl** (yolks), protocole **Wings ✅ validé** | `auth`→`auth success`→`status`/`stats`/`console output` ; `stats` ~1×/s. |
| `owner: 0/1` + permissions sous-utilisateur (`command`, `poweraction_*`, `view`…) | Griser les actions non autorisées. |

---

## 2. Vue d'ensemble en couches

```
┌──────────────────────────────────────────────────────────┐
│  SvelteKit (webview)                                       │
│  routes: /onboarding · / (workspace à onglets)            │
│  stores: auth · servers · activeServer · console          │
│  xterm.js  ·  jauges  ·  TanStack Query (cache REST)      │
└───────────────┬───────────────────────▲──────────────────┘
      invoke()  │ (commandes)            │ events (émis)
                ▼                        │
┌──────────────────────────────────────────────────────────┐
│  Cœur Rust (Tauri)                                         │
│  secrets  →  api (reqwest)  →  ws (Wings)  →  models       │
│  stockage clé OS · base64 auth · backoff 429 · reconnect  │
└───────────────┬──────────────────────────────────────────┘
                ▼
        API MineStrator  +  WebSocket node (Wings)
```

---

## 3. Cœur Rust — modules

```
src-tauri/src/
├── main.rs            # setup Tauri, plugins, register commands
├── secrets.rs         # stockage/lecture/effacement de la clé API
├── api/
│   ├── mod.rs         # client reqwest, base URL, header auth, désenveloppage
│   ├── error.rs       # ApiError (mapping des codes API_*), backoff 429
│   └── endpoints.rs   # fonctions typées: get_user, list_servers, get_server, live_light, power_action, send_command, console_logs
├── ws/
│   ├── mod.rs         # ConsoleManager: 1 tâche tokio par serveur connecté
│   ├── protocol.rs    # (dé)sérialisation des events Wings
│   └── reconnect.rs   # backoff + refresh token
├── commands.rs        # #[tauri::command] exposées au front
├── events.rs          # helpers d'émission d'events typés
└── models.rs          # structs serde (UserProfile, ServerListItem, ServerFull, LiveLight, Stats…)
```

### 3.1 Commandes Tauri (IPC front → Rust)

| Commande | Entrée | Sortie | Rôle |
|---|---|---|---|
| `validate_and_store_key` | `key: String` | `UserProfile` | Teste la clé via `GET /user`, la stocke si OK. |
| `has_stored_key` | — | `bool` | Au démarrage : onboarding ou app. |
| `clear_key` | — | `()` | Déconnexion / suppression clé. |
| `list_servers` | — | `{ myboxes, servers }` | `GET /user/{id}/servers`. |
| `get_server` | `id` | `ServerFull` | `GET /server/{id}` (inclut url+token WS). |
| `get_live_light` | `id` | `LiveLight` | Polling léger (joueurs, version, motd, limites). |
| `power_action` | `id, action` | `()` | `PUT /server/{id}/poweraction`. |
| `send_command` | `id, command` | `()` | `PUT /server/{id}/command`. |
| `get_console_logs` | `id` | `Vec<String>` | 100 dernières lignes (préchargement console). |
| `console_connect` | `id` | `()` | Ouvre la tâche WS pour ce serveur. |
| `console_disconnect` | `id` | `()` | Ferme la tâche WS. |

### 3.2 Events (Rust → front, via `emit`)

| Event | Payload | Usage front |
|---|---|---|
| `console://output` | `{ server_id, line }` | Append dans xterm. |
| `console://stats` | `{ server_id, cpu_abs, mem_bytes, disk_bytes, uptime, state }` | Jauges instantanées. |
| `console://status` | `{ server_id, state }` | Badge running/starting/stopping/offline. |
| `console://connection` | `{ server_id, phase }` | `connecting \| open \| reconnecting \| closed \| error`. |

---

## 4. Sécurité de la clé API

- Stockage via le **trousseau de l'OS** : `keyring` crate (Keychain macOS / Credential Manager Windows / libsecret Linux), ou `tauri-plugin-stronghold` (coffre chiffré) selon dispo Linux.
- **Jamais** dans `localStorage`, un fichier en clair, ni transmise au webview.
- Token résolu une fois à la connexion (`authenticate` : valeur telle quelle, sinon base64) et stocké prêt à l'emploi ; réutilisé sans transformation.
- Validation à la saisie : `GET /user` → `200` = OK (on affiche pseudo + solde), `401` = clé invalide.
- `clear_key` efface l'entrée du trousseau (déconnexion).

---

## 5. Client HTTP (`api`)

- `reqwest` (async, TLS natif), un `Client` réutilisé (pool de connexions).
- Injection auto du header `Authorization`, `Accept: application/json`, timeout (~10 s).
- **Désenveloppage** : toute réponse OK → on extrait `api.data`. Réponse erreur → `ApiError { http, code, api_code, message_fr }`.
- **Backoff 429/5xx** : retry exponentiel avec jitter, plafonné (ex. 3 tentatives), respect de `Retry-After`.
- Mapping FR des codes connus : `API_SERVER_SUSPENDED`, `API_SERVER_EXPIRED`, `API_MISSING_REQUIRED_FIELDS`, permission refusée (403), etc.

---

## 6. Console WebSocket — le cœur

**Protocole : Pterodactyl Wings — ✅ VALIDÉ en réel** (spike du 2026-07-04 contre `wss://5027.mystrator.com:52709/api/servers/{uuid}/ws`).

> 🔑 **Contrainte critique confirmée : `Origin: https://minestrator.com` est OBLIGATOIRE** sur le handshake d'upgrade.
> Sans lui (ou avec un autre Origin) → **HTTP 403** ; avec → **101 Switching Protocols**.
> Un webview navigateur ne peut pas forger l'`Origin` → **le WS DOIT vivre dans Rust** (tokio-tungstenite pose l'en-tête sans problème). C'est la justification n°1 de l'architecture.

Séquence validée :
1. `get_server(id)` → `websocket.url` + `websocket.token` (si vides → serveur hiberné, pas de WS).
2. Connexion `wss://…` avec **`Origin: https://minestrator.com`**.
3. Auth : envoi `{"event":"auth","args":[token]}`.
4. Réception d'events (noms et formats **observés en réel**) :
   - `auth success` (`args:[]`) → prêt ; on enchaîne `send logs` + `send stats`.
   - `status` `args:["running"|"starting"|"stopping"|"offline"]` → event `console://status`.
   - `stats` `args:["{json}"]` où le JSON = `{ cpu_absolute, memory_bytes, memory_limit_bytes, disk_bytes, network:{rx_bytes,tx_bytes}, uptime, state }` → event `console://stats` (streamé ~1×/s).
   - `console output` `args:["ligne (ANSI)"]` → event `console://output`.
   - `token expiring` / `token expired` → **refresh** : re-`get_server(id)`, ré-`auth` avec le nouveau token.
   - `daemon error` / `jwt error` → log + reconnexion.

Requêtes client observées : `{"event":"send logs","args":[null]}` (rejoue le backlog), `{"event":"send stats","args":[null]}` (force un push stats). `send command` / `set state` existent aussi côté Wings.

**Jauges — calcul confirmé :** `cpu_absolute` est un **pourcentage** (ex. `3.4` = 3,4 %). La limite `cpu.limit` de `live/light` est en centièmes de cœur (`300` = 3 cœurs = 300 %) → usage % = `cpu_absolute / cpu.limit`. Pour la RAM/disque, utiliser `memory_bytes / memory_limit_bytes` et `disk_bytes` du flux WS (plus fiable que les limites décimales de `live/light`).

**Décision de conception — envoi via REST, WS en lecture.**
Les commandes et power actions sont envoyées par les **endpoints REST publics** (`/command`, `/poweraction`), pas par le WS. Raisons : contrat d'API officiel et stable, gestion des permissions homogène, moins de surface de casse si le protocole Wings évolue. Le WS sert à **recevoir** output + stats + status. (Le token WS peut d'ailleurs être émis en lecture seule ; l'envoi REST est donc plus sûr. Option d'optimisation plus tard : bascule vers l'envoi WS si latence critique.)

**Reconnexion** : sur `close`/`error`, backoff exponentiel + jitter (ex. 1s → 30s max), émission de `console://connection: reconnecting`. Le préchargement des 100 lignes (`/console/logs`) évite le « trou » visuel après reconnexion.

**Hibernation** : `websocket.url` vide → écran « serveur en hibernation », bouton *Démarrer* (power action `start`) ; à la sortie d'hibernation, re-fetch et connexion.

---

## 7. Rendu console (front)

- Composant `Console.svelte` encapsulant **xterm.js** (rendu ANSI natif, scrollback, recherche `Ctrl+F`, addon `fit`).
- Au montage : `get_console_logs` → écrit les 100 lignes, puis `console_connect` → bascule sur le flux `console://output`.
- Champ de saisie → `send_command` (historique des commandes ↑/↓, autocomplétion basique plus tard).
- Barre d'actions : `start / restart / restart10 / stop / stop10 / kill` (avec confirmation sur `stop`/`kill`), grisées selon permissions.

---

## 8. Stats & état temps réel (front)

- **Jauges** CPU / RAM / disque = *usage* (event `console://stats`) sur *limite* (`live/light`). Si pas de WS (hiberné/arrêté) → jauges à 0 / « hors-ligne ».
- **Joueurs / version / motd / hostname** = `live/light`, polling léger (10–15 s) via TanStack Query, pausé quand la fenêtre est cachée.
- **Badge d'état** = `console://status` (fallback `live/light.status`).

---

## 9. Front SvelteKit — organisation (shell à onglets)

**Décision (2026-07-04) : shell « workspace à onglets », pas de routes par serveur.**
L'app authentifiée tient sur **une seule route** (`/`) qui rend une barre d'onglets + une
pile de panneaux. Un onglet « Home » permanent (liste des serveurs) coexiste avec N onglets
serveur ; chaque onglet serveur cible un couple **(serveur, vue)** — `overview`, `console`,
`sftp`, `files`, `players`, `settings`. **Plusieurs onglets peuvent viser le même serveur et
la même vue** (doublons autorisés), chacun avec son `id` d'instance.

Pourquoi pas des routes `/server/[id]` : (1) le routing SvelteKit démonte la page quittée →
une console/SFTP perdrait sa connexion en changeant d'onglet ; (2) une URL ne peut pas
représenter deux onglets identiques. **Les panneaux inactifs restent donc montés** (masqués
en `display`), ce qui préserve l'état vivant (WS, scrollback) — c'est le point d'architecture clé.

```
src/
├── routes/
│   ├── +layout.svelte              # thème, garde d'auth (onboarding ↔ /)
│   ├── onboarding/+page.svelte     # saisie + validation clé
│   └── +page.svelte                # shell : topbar + <Workspace/>
├── lib/
│   ├── ipc.ts                      # wrappers typés autour de invoke()
│   ├── types.ts                    # miroirs TS des models Rust
│   ├── status.ts                   # état serveur → libellé + couleur
│   ├── theme.ts                    # thème clair/sombre
│   ├── stores/auth.svelte.ts       # état d'auth (rune $state)
│   ├── tabs/tabs.svelte.ts         # gestionnaire d'onglets (open/close/focus/doublons)
│   └── components/
│       ├── Workspace.svelte        # barre d'onglets + pile de panneaux keep-alive
│       ├── TabBar.svelte           # rendu des onglets
│       ├── HomePanel.svelte        # liste MyBox/serveurs (jalon 2)
│       ├── ServerCard.svelte       # carte serveur + ouverture d'onglets
│       ├── ServerPanel.svelte      # dispatch (serveur, vue) → composant de vue
│       └── views/                  # ServerOverview · ConsoleView · SftpView · ComingSoon
```

- **State** : runes `$state` (auth, onglets, données de liste). TanStack Query optionnel plus
  tard pour le cache REST poli (live/light).
- **Garde d'auth** : au boot, `has_stored_key` → `/onboarding` ou `/`.
- **Keep-alive** : `Workspace` fait `{#each tabs}` avec panneau monté en permanence, seul
  l'actif est en `display:block` (démontré par le compteur « monté depuis Xs » de `ConsoleView`).

---

## 10. UX desktop

- Fenêtre unique, thème sombre par défaut (clair/sombre suivant l'OS).
- **Tray icon** : accès rapide + (plus tard) notifications « serveur hors-ligne ».
- Raccourcis : power actions, `Ctrl+K` (command palette — V2/V3), `Ctrl+F` (recherche console).

---

## 11. États limites à couvrir (checklist V1)

- [ ] Clé invalide / révoquée (401 en cours d'usage → retour onboarding).
- [ ] Hors-ligne réseau (bannière + retry).
- [ ] Serveur hiberné (pas de WS) / suspendu / expiré / désactivé.
- [ ] Sous-utilisateur : actions grisées selon permissions (`owner=0`).
- [ ] Rate limit 429 (backoff transparent + indicateur discret).
- [ ] Token WS expiré (refresh silencieux).

---

## 12. Jalons de construction V1 (ordre recommandé)

| # | Jalon | Contenu | « Done » |
|---|---|---|---|
| 1 | **Socle** | Scaffold Tauri 2 + SvelteKit, stockage clé, onboarding, `GET /user` | Je saisis ma clé, je vois mon pseudo + solde. |
| 2 | **Liste serveurs** | `list_servers`, cartes groupées par MyBox, badges d'état | Je vois mes serveurs et leur état. |
| 3 | **Dashboard REST** | `get_server` + `live/light`, jauges (limites), power bar via REST | Je peux start/stop/restart un serveur. |
| 4 | **Spike WS** 🔬 | Connexion réelle, validation du protocole Wings, refresh token | Je reçois output + stats en direct dans un log brut. |
| 5 | **Console** | xterm.js, préchargement `console/logs`, flux live, envoi commande | Console pleinement utilisable. |
| 6 | **Polish** | Erreurs, hibernation, permissions, reconnexion, tray | Tous les états limites gérés. |

---

## 13. Dépendances prévues

**Rust (`Cargo.toml`)** : `tauri` 2, `reqwest` (rustls), `tokio` (rt + macros), `tokio-tungstenite`, `serde`/`serde_json`, `base64`, `keyring` (ou `tauri-plugin-stronghold`), `thiserror`, `tracing`.

**Front (`package.json`)** : `@sveltejs/kit`, `@tauri-apps/api`, `@tanstack/svelte-query`, `xterm` + `xterm-addon-fit` + `xterm-addon-search`, `vite`.

---

## 14. Risques / à valider tôt

1. ~~**Protocole WS exact**~~ → ✅ **RÉSOLU** (spike 2026-07-04) : Wings confirmé, `Origin: https://minestrator.com` obligatoire, events `auth success`/`status`/`stats`/`console output`, `stats` ~1×/s. Le jalon 4 devient une simple transcription en Rust.
2. **Refresh du token WS** : durée de vie exacte du JWT et déclenchement `token expiring` non encore observés (session de spike trop courte) → à instrumenter au jalon 4.
3. **Rate limits réels** non documentés → prévoir backoff généreux et polling raisonnable.
4. **Permissions sous-utilisateur** : liste exacte des clés et leur présence dans les réponses (le compte de test est `owner=1`, `permissions:[]` → cas sous-utilisateur non couvert par le spike).
