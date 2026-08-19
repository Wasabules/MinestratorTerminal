# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> Le code, les commentaires, la doc et les messages de commit de ce dépôt sont **en français**. Garde
> cette convention. Fins de ligne **LF** partout (`.gitattributes`).

## Commandes

Il n'y a **pas de projet npm à la racine** : `desktop/` et `mobile/` ont chacun le leur.
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
cargo test -p minestrator-core                  # 56 tests unitaires, tous dans le core
cargo test -p minestrator-core redact::         # un module de tests
cargo test -p minestrator-core nom_du_test      # un test précis
cargo clippy --workspace --all-targets          # lint
cargo build -p minestrator-mcp --release        # binaire MCP headless
cargo run -p minestrator-daemon --release       # daemon FCM (voir crates/minestrator-daemon/README.md)
```

Pas de framework de test côté front : la vérification est `npm run check`. Tous les tests Rust vivent
en modules `#[cfg(test)]` dans `crates/minestrator-core/src/*.rs` (aucun répertoire `tests/`).

## Architecture

Monorepo **Cargo workspace** : toute la logique vit dans un cœur UI-agnostique, les apps ne sont que
des couches d'adaptation. `ARCHITECTURE.md` est la référence détaillée (à jour, hors numéro de version).

```
crates/minestrator-core/   TOUTE la logique métier (façade `Core` + `CoreEvent`)
crates/minestrator-mcp/    serveur MCP headless (stdio)
crates/minestrator-daemon/ daemon Linux : superviseur + push FCM
desktop/src-tauri/         adaptation desktop : 88 commandes IPC, tray, pont d'events, updater
desktop/src/               SvelteKit + Svelte 5 (runes) + TS
mobile/src-tauri/          adaptation Android/iOS : 28 commandes (sous-ensemble), service de fond
mobile/src/                UI tactile, même core
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
`generate_handler!` de `desktop/src-tauri/src/lib.rs`** (oubli fréquent) → `desktop/src/lib/ipc.ts` +
`types.ts` → clés i18n dans **`i18n/fr.ts` ET `i18n/en.ts`** (mêmes arborescences de clés des deux
côtés ; le type `Dict` est dérivé de `fr.ts`, donc un oubli côté `en` casse le typecheck).

**Ajouter un outil MCP** : `crates/minestrator-core/src/mcp.rs` → schéma dans `tool_list()` +
`dispatch_inner()` + inscription dans `READ_TOOLS` **ou** `WRITE_TOOLS`. La garde est en
**deny-by-default** : un outil absent de `READ_TOOLS` est traité comme modifiant, un outil absent des
deux listes est refusé. Un test du module vérifie cette cohérence. Mets `MCP.md` à jour (catalogue).

**Ajouter un fournisseur LLM** : un variant dans `llm.rs` + son (dé)sérialiseur ; l'agent parle un
format normalisé (`ToolSpec`/`Msg`/`ToolCall`/`ToolResult`), `copilot.rs` ne bouge pas. Les agents CLI
locaux (Claude Code / OpenCode / Gemini CLI) sont isolés dans `cli_agent.rs` (flags, config MCP, parsing).

**Erreurs** : `core/src/error.rs` — l'enum `Error` sérialise en `{ kind, message }`. Ajouter un variant
implique de mettre à jour `kind()`, puis `humanizeError()` (`ipc.ts`) et la clé `errors.<kind>` en fr+en.

### Points transverses

- **Adaptation par jeu** : `core/src/games.rs` (`GameCapabilities`, dérivé du nom d'egg Pterodactyl) est
  la **source unique** qui pilote les vues exposées, l'autocomplétion et la coloration de la console, la
  marketplace et les outils du Copilote. Un nouveau jeu se déclare là, pas dans les composants.
- **Secrets** : `core/src/secrets.rs` aiguille automatiquement — `keyring` natif sur desktop, backend
  **fichier** (`secrets/file_store.rs`, dossier privé de l'app) sur Android/iOS et pour le daemon via
  `MINESTRATOR_SECRETS_FILE` + `MINESTRATOR_DATA_DIR`.
- **Confidentialité** : tout ce qui sort vers un LLM ou un service de paste passe par `redact.rs`
  (mots de passe, IPv4, e-mails, secrets `clé=valeur`). Ne contourne pas ce filtre.
- **Double point d'entrée desktop** : `desktop/src-tauri/src/main.rs` lance la boucle MCP stdio au lieu
  d'ouvrir la GUI si l'argument `--mcp` est présent — même `core::mcp::serve_stdio` que le binaire dédié.
- **Charte graphique** : `DESIGN.md` (tokens CSS, dark-first). Ne pas câbler en dur les couleurs
  renvoyées par `/user` (bannières promo temporaires).

## Release

Bumper la version dans **5 fichiers** (ils doivent rester identiques) : `Cargo.toml` (racine,
`[workspace.package]`), `desktop/package.json`, `desktop/src-tauri/tauri.conf.json`,
`mobile/package.json`, `mobile/src-tauri/tauri.conf.json` — puis `git tag vX.Y.Z && git push origin vX.Y.Z`.
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
| `MCP.md` | Serveur MCP : catalogue des 39 outils, gating, transport |
| `COPILOT.md` | Copilote IA : fournisseurs, agents CLI, boucle superviseur → diagnostic |
| `DESIGN.md` | Charte graphique / tokens |
| `docs/AUTO-UPDATE.md`, `docs/PUSH.md` | Updater signé ; notifications push mobile (FCM) |
| `mobile/README.md` | Spécificités Android (gen/android, signature, périmètre livré) |
| `ROADMAP.md` | Livré / à finir / backlog |
