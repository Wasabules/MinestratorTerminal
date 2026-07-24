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

## `gen/android` est **versionné**

Le projet `src-tauri/gen/android` est **commité** (ses sorties de build, `keystore.properties` et
`*.jks` restent ignorés). Les personnalisations y vivent donc durablement :

- **Clavier** : `android:windowSoftInputMode="adjustResize"` sur `.MainActivity`
  (`app/src/main/AndroidManifest.xml`).
- **Signature de release** : `app/build.gradle.kts` lit `keystore.properties` (écrit par la CI).
- **Push FCM** (à venir) : `google-services.json` + `FirebaseMessagingService`, cf. [`../docs/PUSH.md`](../docs/PUSH.md).

> Ne PAS relancer `tauri android init` (il écraserait ces personnalisations). Le build les régénère
> déjà ce qu'il faut (`tauri.properties`, `tauri.build.gradle.kts`, assets…).

## Lancer (Android)

Prérequis : **Android SDK + NDK**, `ANDROID_HOME`/`NDK_HOME` configurés, un émulateur ou un appareil
en débogage USB. (Détails : <https://v2.tauri.app/start/prerequisites/#android>.)

```bash
cd mobile
npm install
npx tauri android dev       # build + déploie sur l'émulateur/appareil
npx tauri android build --apk --debug   # APK debug local
```

## Release APK (CI) — signature

Le workflow [`release.yml`](../.github/workflows/release.yml) construit un **APK signé** et l'attache
à la release GitHub (job `android`). Il faut **4 secrets** de dépôt (une seule fois) — génération :

```bash
# 1. Générer un keystore de release (garde-le PRÉCIEUSEMENT : sans lui, plus de MAJ en place)
keytool -genkey -v -keystore release.jks -keyalg RSA -keysize 2048 -validity 10000 -alias minestrator

# 2. L'encoder en base64 (pour le secret)
base64 -w0 release.jks > release.jks.b64      # Linux
#   PowerShell : [Convert]::ToBase64String([IO.File]::ReadAllBytes("release.jks")) > release.jks.b64
```

Puis dans **Settings → Secrets and variables → Actions**, créer :

| Secret | Valeur |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | contenu de `release.jks.b64` |
| `ANDROID_KEYSTORE_PASSWORD` | mot de passe du keystore |
| `ANDROID_KEY_ALIAS` | `minestrator` (l'alias choisi) |
| `ANDROID_KEY_PASSWORD` | mot de passe de la clé |

> **Version** : bumper aussi `mobile/src-tauri/tauri.conf.json` (`version`) avant de taguer — c'est
> lui qui donne le `versionName`/`versionCode` de l'APK (le `versionCode` doit croître pour permettre
> les mises à jour en place).

## Périmètre & suite

Slice de départ livré : **onboarding (clé API) → liste serveurs → Overview (jauges + power) →
Console live**. À étoffer ensuite :

- ✅ **Joueurs** : liste en ligne + actions kick/ban/op et par pseudo (`api.playerAction`).
- ✅ **Secrets Android** : backend **fichier** dans le dossier privé de l'app (chiffré au repos par
  Android FBE) — le core aiguille automatiquement (`keyring` sur desktop). Durcissement Keystore
  matériel = amélioration future.
- ✅ **Alertes app fermée (on-device)** : réglage *Surveillance en arrière-plan* → un **service
  Android au premier plan** (`MonitorService`) garde le process vivant, donc le superviseur Rust +
  `forward()` continuent de poster les alertes app fermée (Home / changement d'app). Aucun serveur
  ni Firebase. Limite : un kill total du process (mémoire, balayage sur OEM agressif) suspend la
  surveillance jusqu'à réouverture — pour une garantie « même tél éteint », voir le palier FCM.
- ✅ **Notifications** : permission `POST_NOTIFICATIONS` demandée à l'exécution (Android 13+).
- ⏳ **Palier FCM (garanti)** : daemon `crates/minestrator-daemon` (surveillance + push FCM), pour
  être prévenu même process tué / tél en veille. Reste à câbler la **réception** côté Android →
  voir [`docs/PUSH.md`](../docs/PUSH.md).
- **Éditeurs de config en formulaire** (`server.properties`, `ops.json`…), Copilote en chat.
- **Natif** : verrou biométrique, widget de statut, pull-to-refresh, swipe-actions.
- **Factorisation** : extraire `ipc.ts`/`events.ts`/i18n/tokens dans un package partagé avec le desktop.
