# Notifications push (mobile) — architecture & mise en place

Objectif : être prévenu sur le téléphone **quand un serveur crashe / sature / expire, même app
fermée**. Deux paliers complémentaires :

- **Palier 1 — on-device (livré).** Le réglage *Surveillance en arrière-plan* démarre un **service
  Android au premier plan** (`MonitorService`) dont le seul rôle est de **garder le process de l'app
  vivant**. Le superviseur Rust (démarré dans `setup()`) et `forward()` continuent alors de sonder
  et de poster les alertes — même app en arrière-plan (Home, changement d'app). **Aucun serveur ni
  Firebase.** Limite : si le système **tue complètement le process** (mémoire basse, balayage des
  récents sur certains OEM, veille prolongée), la surveillance s'arrête jusqu'à réouverture. Pour la
  fiabilité, exclure l'app de l'optimisation batterie (Réglages Android → Batterie → sans restriction).
- **Palier 2 — FCM (ci-dessous, non câblé).** Pour une garantie **même process tué / tél en veille**,
  un **daemon** hébergé surveille en continu (via `minestrator-core`) et envoie un **push FCM**.

```
┌───────────────┐   surveille    ┌──────────────────────┐   FCM v1    ┌──────────────┐
│ API MineStrator│◀─────────────│  minestrator-daemon   │───push────▶│  FCM (Google) │
└───────────────┘  (superviseur) │  (Linux, réutilise le │            └──────┬───────┘
                                  │   même core)          │                   │ push
                                  └──────────────────────┘                   ▼
                                       ▲ device tokens              ┌──────────────────┐
                                       └────────────────────────────│  App Android      │
                                          (enregistrement)          │  (Tauri + FCM SDK)│
                                                                     └──────────────────┘
```

Trois morceaux : **(1)** le daemon (fait — `crates/minestrator-daemon`), **(2)** un projet Firebase,
**(3)** la réception du push dans l'app Android (après `tauri android init`).

## 1. Le daemon — déjà là

Il réutilise le core, démarre le superviseur, et pousse `CoreEvent::Alert` en FCM. Config et
lancement : [`crates/minestrator-daemon/README.md`](../crates/minestrator-daemon/README.md).
À héberger là où il tourne 24/7 (petit VPS, Raspberry Pi, la même machine que le desktop…).

### Access token FCM (production)

FCM v1 exige un **access token OAuth2** (scope `https://www.googleapis.com/auth/firebase.messaging`)
obtenu depuis un **compte de service** Firebase. Le scaffold lit un token déjà obtenu
(`FCM_ACCESS_TOKEN`/`FCM_ACCESS_TOKEN_FILE`). Pour l'automatiser, deux options :

- **Côté Rust** : intégrer `gcp_auth` dans le daemon (JWT signé → token, auto-rafraîchi). Point
  d'extension prévu : `fcm::access_token()`.
- **Côté ops** : un petit script/cron qui écrit le token dans `FCM_ACCESS_TOKEN_FILE` toutes les
  ~55 min (`gcloud auth application-default print-access-token`).

## 2. Projet Firebase

1. Console Firebase → nouveau projet → ajouter une app **Android** avec l'identifiant
   `com.geoffreylecoq.minestratorterminal`.
2. Télécharger **`google-services.json`** (servira à l'étape 3).
3. Créer un **compte de service** (Paramètres → Comptes de service) → clé JSON pour le daemon.
4. Noter le **project id** (→ `FCM_PROJECT_ID`).

## 3. Réception du push dans l'app Android

À faire **après** `cd mobile && npx tauri android init` (qui génère `src-tauri/gen/android`) :

1. **`google-services.json`** → `mobile/src-tauri/gen/android/app/`.
2. **Gradle** : plugin `com.google.gms.google-services` + dépendance
   `com.google.firebase:firebase-messaging` (dans les `build.gradle` générés).
3. **Service FCM** : un `FirebaseMessagingService` (Kotlin) qui récupère le **device token**
   (`onNewToken`) et reçoit les messages (`onMessageReceived`) → affiche une notification locale
   (on réutilise déjà `tauri-plugin-notification`).
4. **Enregistrement du token** : remonter le device token au daemon. Options :
   - via l'API MineStrator si elle expose un stockage de tokens, **ou**
   - un petit endpoint HTTP sur le daemon qui écrit dans `DEVICE_TOKENS_FILE`, **ou**
   - manuellement au début (coller le token dans `devices.json`) pour tester.

> Un plugin Tauri de push (communautaire ou maison) peut encapsuler 2–3. Tant qu'il n'est pas en
> place, on teste avec un device token collé à la main dans `DEVICE_TOKENS_FILE`.

## Ordre de test conseillé

1. Lancer le daemon **sans** `FCM_*` → vérifier qu'il logue les alertes (surveillance OK).
2. Créer le projet Firebase + compte de service ; fournir `FCM_PROJECT_ID` + un `FCM_ACCESS_TOKEN`.
3. Récupérer un device token (étape 3) → le mettre dans `DEVICE_TOKENS_FILE`.
4. Provoquer une alerte (arrêt inattendu d'un serveur de test) → push reçu, app fermée. ✅
