# Minestrator Terminal — Mobile (Android / iOS)

App **Tauri 2 mobile** partageant le même `crates/minestrator-core` que le desktop.
Même nom, même identité : **Minestrator Terminal** (`com.geoffreylecoq.minestratorterminal`).

> État : **à scaffolder**. Ce dossier ne contient encore que ce plan.

## Pourquoi Tauri mobile

Le cœur (`minestrator-core`) fait **tout le réseau** en Rust. C'est indispensable ici : le
handshake WebSocket Wings exige l'en-tête `Origin: https://minestrator.com`, **impossible à
forger depuis un webview/navigateur** (cf. `ARCHITECTURE.md` §4). Rust tourne nativement sur
Android/iOS → le WS et le SFTP (russh) fonctionnent à l'identique du desktop, sans réécriture.

## Structure cible

```
mobile/
  package.json            projet npm propre (frontend Svelte mobile)
  src/                    UI tactile : shell bottom-tabs (pas d'onglets keep-alive)
                          réutilise ipc.ts / events.ts / i18n / tokens du desktop
  src-tauri/              app Tauri mobile (thin layer sur le core)
    tauri.conf.json       productName "Minestrator Terminal", même identifier
    gen/android/          généré par `tauri android init` (projet Gradle)
    gen/apple/            généré par `tauri ios init`     (projet Xcode)
```

Le crate `mobile/src-tauri` s'ajoutera au workspace racine (`Cargo.toml` → `members`).

## Scaffold (étapes)

```bash
# 1. Frontend mobile (depuis mobile/)
#    npm create + SvelteKit adapter-static, puis brancher @tauri-apps/api + cli

# 2. Init Tauri mobile (nécessite les toolchains)
cd mobile
npm install
npx tauri android init      # Android SDK/NDK requis
npx tauri ios init          # macOS + Xcode requis (iOS)

# 3. Lancer
npx tauri android dev
npx tauri ios dev
```

## Adaptations spécifiques mobile (vs desktop)

- **Secrets** : `keyring` ne couvre pas iOS/Android → backend Keychain/Keystore derrière le
  trait `secrets` du core (une seule impl à ajouter).
- **Pas de superviseur de fond** (l'OS tue les tâches persistantes) → alertes via un
  **daemon Linux + push FCM/APNs** (le daemon réutilise `minestrator-core`).
- **Périmètre tactile** : console, power actions, joueurs, backups, éditeurs de config en
  formulaire, Copilote en chat. On laisse au desktop l'inspecteur NBT / la carte `.mca` / le SFTP lourd.
- **Plus natif** : verrou biométrique, widget de statut, pull-to-refresh, swipe-actions.
