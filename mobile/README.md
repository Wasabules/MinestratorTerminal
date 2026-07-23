# Minestrator Terminal — Mobile (Android, iOS à venir)

App **Tauri 2 mobile** partageant le même `crates/minestrator-core` que le desktop.
Même nom, même identité : **Minestrator Terminal** (`com.geoffreylecoq.minestratorterminal`).

> État : **scaffold en place et vérifié** (frontend + crate Rust compilent). Reste à lancer
> `tauri android init` avec ton SDK Android pour générer le projet Gradle et builder l'APK.
> **iOS** : cross-platform dans le code, mais l'`init`/build iOS nécessite macOS + Xcode (plus tard).

## Pourquoi Tauri mobile

Le cœur (`minestrator-core`) fait **tout le réseau** en Rust. Indispensable ici : le handshake
WebSocket Wings exige l'en-tête `Origin: https://minestrator.com`, **impossible à forger depuis
un webview/navigateur** (cf. `ARCHITECTURE.md` §4). Rust tourne nativement sur Android → le WS
et le SFTP (russh) fonctionnent à l'identique du desktop, sans réécriture.

## Ce qui est déjà là

```
mobile/
  package.json            projet npm (frontend Svelte mobile, port dev 1430)
  src/
    app.css               tokens de la charte (DESIGN.md), dark-first + safe-areas
    routes/+page.svelte    shell : boot auth → onboarding / liste / serveur
    lib/
      ipc.ts               couche IPC typée (sous-ensemble de départ)
      events.ts            events console (conn_id)
      types.ts, i18n.ts    modèles + fr/en
      stores/auth.svelte.ts
      components/          Onboarding, ServersList, ServerView, BottomNav
        views/            OverviewView (jauges + power), ConsoleView (WS live), PlayersView (stub)
  src-tauri/
    src/lib.rs             entrée mobile (mobile_entry_point), pont CoreEvent → webview
    src/commands.rs        commandes de départ (auth, serveurs, console, power, joueurs)
    tauri.conf.json        productName "Minestrator Terminal", devUrl :1430
    capabilities/          core + notification
    icons/                 (copiées du desktop, variantes Android incluses)
```

Le crate `mobile/src-tauri` est membre du workspace racine (`Cargo.toml`).

**Vérifié** : `npm run check` (180 fichiers, 0 erreur) · `npm run build` (adapter-static) ·
`cargo check -p minestrator-terminal-mobile` (host, OK).

## Lancer (Android)

Prérequis : **Android SDK + NDK**, un `ANDROID_HOME`/`NDK_HOME` configurés, et un
émulateur ou un appareil en débogage USB. (Détails : <https://v2.tauri.app/start/prerequisites/#android>.)

```bash
cd mobile
npm install
npx tauri android init      # génère src-tauri/gen/android (projet Gradle) — une fois
npx tauri android dev       # build + déploie sur l'émulateur/appareil
# npx tauri android build   # APK/AAB de release
```

## Périmètre & suite

Slice de départ livré : **onboarding (clé API) → liste serveurs → Overview (jauges + power) →
Console live**. À étoffer ensuite :

- ✅ **Joueurs** : liste en ligne + actions kick/ban/op et par pseudo (`api.playerAction`).
- ✅ **Secrets Android** : backend **fichier** dans le dossier privé de l'app (chiffré au repos par
  Android FBE) — le core aiguille automatiquement (`keyring` sur desktop). Durcissement Keystore
  matériel = amélioration future.
- ✅ **Alertes app fermée** : daemon `crates/minestrator-daemon` (surveillance + push FCM). Reste à
  câbler la **réception** côté Android après `tauri android init` → voir [`docs/PUSH.md`](../docs/PUSH.md).
- **Éditeurs de config en formulaire** (`server.properties`, `ops.json`…), Copilote en chat.
- **Natif** : verrou biométrique, widget de statut, pull-to-refresh, swipe-actions.
- **Factorisation** : extraire `ipc.ts`/`events.ts`/i18n/tokens dans un package partagé avec le desktop.
