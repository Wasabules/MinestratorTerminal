# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Le code, les commentaires, la doc et les messages de commit de ce dépôt sont **en français**. Garde
> cette convention. Fins de ligne **LF** partout (`.gitattributes`).

## Commandes

Il n'y a **pas de projet npm à la racine** : `desktop/` et `mobile/` ont chacun le leur
(SvelteKit 3 : **Node ≥ 22.17**, config dans `vite.config.js` — plus de `svelte.config.js` —,
imports internes via l'alias `#lib/…` avec extension, ex. `#lib/ipc.ts`).
Les commandes Cargo se lancent depuis la **racine** (le workspace couvre tout le monorepo).

```bash
# --- Frontend / app (depuis desktop/ ou mobile/) ---
npm install
npm run tauri dev            # desktop : app en dev (front hot-reload, port 1420)
npm run check                # typecheck (svelte-kit sync + svelte-check) — SEUL "lint" du front
npm run build                # front seul → build/
npm run tauri build          # installeurs desktop

# --- Mobile Android (depuis mobile/, port dev 1430) ---
npx tauri android dev                    # déploie sur émulateur/appareil (ANDROID_HOME + NDK_HOME requis)
npx tauri android build --apk --debug    # APK debug local

# --- Rust (depuis la racine) ---
cargo test -p minestrator-core                  # tests unitaires (tous dans le core)
cargo test -p minestrator-core redact::         # un module de tests
cargo test -p minestrator-core nom_du_test      # un test précis
cargo clippy --workspace --all-targets -- -D warnings   # lint (la CI refuse tout avertissement)
cargo bindings                                  # régénère les types TS des fronts depuis Rust (ts-rs)
cargo bindings-check                            # échoue si les bindings versionnés ont dérivé (CI)
cargo build -p minestrator-mcp --release        # binaire MCP headless
cargo run -p minestrator-daemon --release       # service d'arrière-plan au premier plan (voir son README)
cargo test -p minestrator-daemon                # tests du service (CLI, unité systemd, plist, SDDL, IPC)
```

Pas de framework de test côté front : la vérification est `npm run check`. Les tests Rust vivent en
modules `#[cfg(test)]` dans `crates/minestrator-core/src/*.rs` et `crates/minestrator-daemon/src/`
(aucun répertoire `tests/`), plus le contrôle des bindings du crate mobile
(`mobile/src-tauri/src/bindings.rs`, feature `ts`).
`cargo bindings` / `cargo bindings-check` sont des alias (`.cargo/config.toml`) qui lancent les tests
`regenerate_bindings` / `bindings_are_*` avec la feature `ts` du core et du crate mobile.

**CI** (`.github/workflows/`) :
- `ci.yml` tourne sur push `main` et sur PR : clippy `-D warnings` et `cargo test --workspace` sur
  Linux, Windows et macOS, plus `npm run check` et `npm audit --omit=dev` pour desktop **et**
  mobile.
- `release.yml` l'appelle en garde-fou avant tout build.
- `audit.yml` lance cargo audit et npm audit chaque semaine et à chaque changement de lockfile.
- Dependabot ouvre des PR groupées.

## Architecture

Monorepo **Cargo workspace** : toute la logique vit dans un cœur UI-agnostique, les apps ne sont que
des couches d'adaptation. `ARCHITECTURE.md` est la référence détaillée. Les décomptes chiffrés
(commandes, outils) y vieillissent vite : recompte dans le code si besoin.

```
crates/minestrator-core/   TOUTE la logique métier (façade `Core` + `CoreEvent`)
crates/minestrator-mcp/    serveur MCP headless (stdio)
crates/minestrator-daemon/ service d'arrière-plan Windows/Linux/macOS : superviseur, IPC, push FCM
desktop/src-tauri/         adaptation desktop : commandes IPC (toutes dans `commands.rs`), tray, events, updater
desktop/src/               SvelteKit + Svelte 5 (runes) + TS
mobile/src-tauri/          adaptation Android/iOS : sous-ensemble des commandes, réparties entre
                           `commands.rs`, `files.rs`, `background.rs`, `update.rs`, `apk_installer.rs`
mobile/src/                UI tactile, même core — front distinct (ipc.ts, types.ts, i18n propres)
```

### Invariants à ne pas casser

1. **Tout le réseau vit en Rust, jamais dans le webview.** HTTP, WebSocket, SFTP, uploads paste.
   Raison dure : le handshake WS Wings exige l'en-tête `Origin: https://minestrator.com`, **impossible
   à forger depuis un navigateur**. C'est la justification n°1 de l'architecture. La clé API n'atteint
   jamais le JS.
2. **`minestrator-core` ne connaît aucune UI.** Rien de Tauri/Svelte n'y entre : le daemon et le MCP
   headless l'utilisent tels quels.
3. **Le front n'appelle jamais `invoke` en direct** — tout passe par la façade typée `src/lib/ipc.ts`.
4. **Envoi via REST, WS en lecture seule.** Commandes et power actions passent par les endpoints REST
   (`PUT /server/{id}/command`, `/poweraction`) ; le WebSocket ne sert qu'à *recevoir* output/stats/status.
5. **Les events console sont taggés par `conn_id`, pas par `server_id`** : plusieurs onglets ou fenêtres
   peuvent observer le même serveur indépendamment.
6. **Shell à onglets keep-alive** (une seule route `/`) : les panneaux inactifs restent montés (masqués
   en `display`) pour préserver WS, scrollback et session SFTP. N'introduis pas de route par serveur.

### Chaînes de fichiers à respecter

**Ajouter une fonctionnalité exposée à l'UI** (desktop) :
`crates/minestrator-core/src/<module>.rs` → méthode publique sur `Core` (`core/src/lib.rs`) →
`desktop/src-tauri/src/commands.rs` (`#[tauri::command]`) → **enregistrement dans le
`generate_handler!` de `desktop/src-tauri/src/lib.rs`** (oubli fréquent) → type IPC éventuel (chaîne
suivante) → `desktop/src/lib/ipc.ts` → clés i18n dans **`i18n/fr.ts` ET `i18n/en.ts`** (mêmes
arborescences de clés des deux côtés ; le type `Dict` est dérivé de `fr.ts`, donc un oubli côté `en`
casse le typecheck).

**Ajouter / modifier un type IPC** (réponse de commande, payload d'event, argument structuré) : les
types TS ne s'écrivent **jamais à la main**, ils sont générés par ts-rs. Sur la struct/enum Rust
sérialisée : `#[cfg_attr(feature = "ts", derive(ts_rs::TS))]` (attributs ts-rs eux aussi sous
`cfg_attr(feature = "ts", ts(...))`) → l'ajouter à `bindings::core()` dans
`crates/minestrator-core/src/bindings.rs` (type propre à l'adaptation mobile :
`mobile/src-tauri/src/bindings.rs`) → **`cargo bindings`** → commiter `desktop/src/lib/bindings/` et
`mobile/src/lib/bindings/` (générés, versionnés). `types.ts` (desktop et mobile) ré-exporte tout via
`export type *` : rien à y ajouter ; il ne garde que les unions purement front (`PowerAction`…).
ts-rs suit les attributs serde ; un `skip_serializing_if` **sans** `default` (champ omis du JSON) exige
`ts(optional)`. `Option<T>` sort en `T | null`, les entiers 64 bits en `number`. `cargo bindings-check`
(CI) échoue si les fichiers versionnés ne correspondent plus au Rust.

**Côté mobile**, la chaîne est parallèle et ne partage aucun fichier front avec le desktop :
`mobile/src-tauri/src/*.rs` → `generate_handler!` de `mobile/src-tauri/src/lib.rs` →
`mobile/src/lib/ipc.ts` + `types.ts` → `mobile/src/lib/i18n.ts` (un seul fichier, dictionnaires
`fr`/`en` **plats et non typés** : une clé oubliée dans `en` n'est pas détectée et s'affiche brute).

**Mode démo** : si la réponse d'une nouvelle commande contient des données identifiantes (noms de
serveur/MyBox, pseudos, IP, e-mails, références), enveloppe l'appel dans `demo(invoke(...), transform)`
dans `ipc.ts` avec les helpers de `fake.svelte.ts` — sinon le mode « données de démo » (captures
publiables) laisse fuiter les vraies valeurs. La substitution ne touche que l'affichage, jamais ce qui
est envoyé au serveur.

**Ajouter un outil MCP** : `crates/minestrator-core/src/mcp.rs` → schéma dans `tool_list()` +
`dispatch_inner()` + inscription dans `READ_TOOLS` **ou** `WRITE_TOOLS`. La garde est en
**deny-by-default** : un outil absent de `READ_TOOLS` est traité comme modifiant, un outil absent des
deux listes est refusé. Un test du module vérifie cette cohérence. Mets `MCP.md` à jour (catalogue).

**Ajouter un fournisseur LLM** : un variant dans `llm.rs` + son (dé)sérialiseur ; l'agent parle un
format normalisé (`ToolSpec`/`Msg`/`ToolCall`/`ToolResult`), `copilot.rs` ne bouge pas. Les agents CLI
locaux (Claude Code / OpenCode / Gemini CLI) sont isolés dans `cli_agent.rs` (flags, config MCP, parsing).

**Erreurs** : `core/src/error.rs` — l'enum `Error` sérialise en `{ kind, message }` via `ErrorPayload`
(type TS généré `AppError`, discriminant `ErrorKind` → `AppErrorKind`). Ajouter un variant implique de
le rattacher dans `error_kind()` (+ `ErrorKind` et `as_str()` si le discriminant est nouveau),
`cargo bindings`, puis `humanizeError()` (`ipc.ts`) et la clé `errors.<kind>` en fr+en.
Le mobile a son propre `humanizeError()` (`mobile/src/lib/ipc.ts`) qui lit des clés **`error.<kind>`**
(singulier) dans `i18n.ts`, avec repli sur le message brut si la clé manque.

### Points transverses

- **Adaptation par jeu** : `core/src/games.rs` (`GameCapabilities`, dérivé du nom d'egg Pterodactyl) est
  la **source unique** qui pilote les vues exposées, l'autocomplétion et la coloration de la console, la
  marketplace et les outils du Copilote. Un nouveau jeu se déclare là, pas dans les composants.
- **Secrets** : `core/src/secrets.rs` aiguille automatiquement — `keyring` natif sur desktop, backend
  **fichier** (`secrets/file_store.rs`, dossier privé de l'app) sur Android/iOS et pour le daemon via
  `MINESTRATOR_SECRETS_FILE` + `MINESTRATOR_DATA_DIR`. Dossier = `util::data_dir()` ; **jamais** le
  répertoire temporaire (partagé sous Unix), fichier en `0600`.
- **Service d'arrière-plan** (`docs/SERVICE.md`) : `core/src/service.rs` porte le protocole IPC, la
  décision « qui supervise » et le lien côté app (superviseur local mis en veille quand le service
  surveille le même compte). Le service ne s'embarque dans les installeurs qu'avec la surcouche
  `tauri build --config src-tauri/tauri.service.conf.json` (utilisée par `release.yml`). Windows :
  l'app s'installe **toujours pour l'utilisateur** (aucun droit admin) ; seul le service en
  demande, à l'activation, et exécute une copie protégée du binaire sous `Program Files`.
- **Confidentialité** : tout ce qui sort vers un LLM, un service de paste ou un export de journaux
  passe par `redact.rs` (mots de passe, IPv4, e-mails, secrets `clé=valeur`). Ne contourne pas ce filtre.
- **Agents CLI du Copilote** (`cli_agent.rs`) : ils lisent du contenu non fiable (chat des joueurs),
  donc leurs outils **natifs** (shell, disque local) sont retirés, avec un mécanisme propre à chaque
  CLI et des tests qui le vérifient. Ne réintroduis jamais `yolo`, ni de `--allowedTools` sans
  `--tools`. Le dossier de config de l'agent (qui contient la clé API) est un `RunDir` : il doit
  vivre aussi longtemps que le processus de l'agent, et pas plus.
- **Journaux** : `core::logs::init` (fichier tournant dans `<data>/logs`), initialisé par l'app. Sur
  mobile, l'initialisation se fait dans `setup`, après `MINESTRATOR_DATA_DIR`.
- **Double point d'entrée desktop** : `desktop/src-tauri/src/main.rs` lance la boucle MCP stdio au lieu
  d'ouvrir la GUI si l'argument `--mcp` est présent — même `core::mcp::serve_stdio` que le binaire dédié.
- **Charte graphique** : `DESIGN.md` (tokens CSS, dark-first). Ne pas câbler en dur les couleurs
  renvoyées par `/user` (bannières promo temporaires).

## Release

Bumper la version dans **5 fichiers** (ils doivent rester identiques) : `Cargo.toml` (racine,
`[workspace.package]`), `desktop/package.json`, `desktop/src-tauri/tauri.conf.json`,
`mobile/package.json`, `mobile/src-tauri/tauri.conf.json` — ajouter l'entrée `## vX.Y.Z` dans
**`CHANGELOG.md`** (la CI en extrait la section pour le corps de la release ; sans entrée, notes
vides) — puis `git tag vX.Y.Z && git push origin vX.Y.Z`. Le skill projet `release-changelog`
(`.claude/skills/`) déroule tout le processus et fixe le style des notes (effet visible pour
l'utilisateur, aucun nom de fonction/fichier).
`.github/workflows/release.yml` build les 3 OS + l'APK Android signé, signe les artefacts de mise à
jour, génère `latest.json` et crée une release **draft** à publier à la main (l'endpoint updater ne
résout que vers une release publiée). Détails : `docs/AUTO-UPDATE.md`.

⚠️ `mobile/src-tauri/gen/android` est **versionné** et personnalisé (`AndroidManifest.xml`,
signature dans `build.gradle.kts`, plugins Kotlin `ApkInstallerPlugin`/`BackgroundMonitorPlugin`,
`MonitorService`). **Ne jamais relancer `tauri android init`** : il écraserait ces personnalisations.
Les keystores (`*.jks`, `keystore.properties`) et `updater-private.key` ne doivent jamais être commités.

## Documentation

| Doc | Contenu |
|---|---|
| `ARCHITECTURE.md` | Couches, modules du core, catalogue IPC/events, faits d'API contraignants |
| `MCP.md` | Serveur MCP : catalogue des outils, gating, transport |
| `COPILOT.md` | Copilote IA : fournisseurs, agents CLI, boucle superviseur → diagnostic |
| `DESIGN.md` | Charte graphique / tokens |
| `docs/AUTO-UPDATE.md`, `docs/PUSH.md` | Updater signé ; notifications push mobile (FCM) |
| `docs/SERVICE.md` | Service d'arrière-plan : secrets machine, IPC, notifications, installeurs |
| `mobile/README.md` | Spécificités Android (gen/android, signature, périmètre livré) |
| `ROADMAP.md` | Livré / à finir / backlog |
| `CHANGELOG.md` | Notes de version (source du corps des releases GitHub) |
