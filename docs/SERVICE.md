# Service d'arrière-plan (supervision machine)

Objectif : **superviser les serveurs même quand l'app est fermée ou qu'aucune session n'est
ouverte**, grâce à un service système sous Windows, Linux et macOS. Deux modes coexistent :

| | Mode utilisateur (défaut, historique) | Mode administrateur (machine) |
|---|---|---|
| Installation de l'app | pour l'utilisateur courant, sans droits admin | **idem** : l'app reste installée pour l'utilisateur |
| Service système | aucun | installé à l'activation, droits admin (UAC, pkexec, osascript) |
| Qui supervise | le superviseur **de l'app**, tant qu'elle tourne (fenêtre ou tray) | le **service système**, en permanence |
| Alertes | toast natif de l'app | consignées par le service, affichées par le tray de l'app, push FCM |
| Clé API | trousseau de l'utilisateur | stockage machine protégé (fichier à accès restreint) |

Le mode utilisateur ne change pas : sans service, l'app se comporte exactement comme avant.

---

## 0. Choix : faire évoluer `minestrator-daemon`

Le daemon Linux existant (`crates/minestrator-daemon`) devient **le** binaire de service des trois
OS, plutôt qu'un second binaire :

- il réutilise déjà le `Core` (superviseur, alertes, API) et le push FCM : rien à dupliquer ;
- un seul binaire = un seul protocole, un seul jeu de tests, une seule chaîne de build ;
- l'usage historique « VPS / conteneur » est conservé tel quel (`minestrator-daemon` sans
  argument = `run`, configuré par l'environnement) ;
- les spécificités d'OS restent cantonnées à de petits modules : `windows-service` (SCM),
  génération d'unité systemd, génération de plist launchd.

Sous-commandes :

```
minestrator-daemon [run] [--owner ID]...    premier plan (VPS, conteneur, dev)
minestrator-daemon service [--owner ID]...  lancé par systemd / launchd / SCM
minestrator-daemon install [--owner ID]... [--no-start]   (admin)
minestrator-daemon enable --owner ID        installe si besoin + autorise ID + démarre (admin)
minestrator-daemon uninstall [--purge]      (admin)
minestrator-daemon start | stop | restart   (admin)
minestrator-daemon status [--json]          (sans privilège)
```

`ID` = uid (Linux/macOS) ou SID (Windows) d'un compte autorisé sur le canal IPC.

```
┌──────────────────────── session utilisateur ───────────────────────┐
│  App Tauri (tray)                                                   │
│   Core ── lien service (10 s) ── superviseur local EN VEILLE        │
│     │  ▲ alertes relayées → toast + centre d'alertes + Copilote     │
└─────┼──┼────────────────────────────────────────────────────────────┘
      │  │ IPC local : socket Unix / canal nommé (JSON, 1 ligne = 1 message)
┌─────▼──┴──────────────────── système ──────────────────────────────┐
│  minestrator-daemon service (systemd / launchd / SCM)               │
│   Core ── superviseur ── SQLite (métriques + journal d'alertes)     │
│     └── push FCM (facultatif)                                       │
└─────────────────────────────────────────────────────────────────────┘
```

Code : `crates/minestrator-core/src/service.rs` (protocole, client, décision, élévation, lien
côté app), `crates/minestrator-daemon/src/` (serveur IPC, installation par OS, service Windows).

---

## 1. Secrets : stockage machine

Un service système n'a pas accès au trousseau d'un utilisateur (ni Credential Manager, ni
Keychain de session, ni Secret Service/D-Bus). Le service force donc le **backend fichier** du
core (`MINESTRATOR_SECRETS_FILE=1`) dans son **dossier de données machine**
(`MINESTRATOR_DATA_DIR`), jamais dans le répertoire temporaire :

| OS | Dossier (données + `secrets.json`) | Protection |
|---|---|---|
| Linux | `/var/lib/minestrator-daemon` → `/var/lib/private/minestrator-daemon` | `DynamicUser=yes` : utilisateur système dédié et sans privilège ; `StateDirectoryMode=0700` ; `/var/lib/private` est en 0700 root ; fichier `0600` (backend fichier durci) |
| macOS | `/Library/Application Support/MinestratorTerminal/service` | dossier `0700` root, daemon root, `Umask 077` |
| Windows | `%ProgramData%\MinestratorTerminal\service` | **DACL protégée** (héritage coupé) : `SYSTEM`, `Administrateurs`, `LocalService` (compte du service) seulement ; contenu **chiffré DPAPI** avec la clé de `LocalService` |

**DPAPI sous Windows : celle du compte du service, pas celle de la machine.** `CryptProtectData`
avec `CRYPTPROTECT_LOCAL_MACHINE` serait déchiffrable par **n'importe quel processus de la
machine** : simple obscurcissement. Le fichier est donc chiffré avec la DPAPI **du compte qui
exécute le service** (`LocalService`, dont le profil est chargé par le SCM) et une entropie propre
à l'app : copié ailleurs ou lu par un autre compte, il est inexploitable, même si la DACL venait à
être élargie. Un fichier en clair laissé par une version antérieure est relu puis réécrit chiffré.
Seul le service écrit ce fichier (la clé arrive par IPC) : l'installation élevée n'y touche pas,
sinon il serait chiffré pour le mauvais compte. Une évolution possible reste un compte de service
virtuel (`NT SERVICE\MinestratorDaemon`) pour ne plus partager la clé DPAPI ni la DACL avec les
autres services `LocalService`.

**macOS et Linux** : un service système n'a pas d'équivalent sûr au trousseau de session. Le
trousseau système macOS et les identifiants systemd reposent eux aussi sur l'accès réservé à
root ; le fichier `0600` d'un dossier `0700` appartenant au compte du service offre la même
garantie, sans dépendance supplémentaire.

Sans cette DACL, un sous-dossier de `%ProgramData%` hérite d'un accès en lecture pour
`Utilisateurs` : `secrets.json` serait lisible par tous. Elle est appliquée par l'installation
élevée (`install`/`enable`) et revérifiée au démarrage du service.

### Transmission de la clé à l'activation

1. L'app lance **élevé** `minestrator-daemon enable --owner <uid|SID>` : installe le service si
   besoin, ajoute le compte à la liste des propriétaires, (re)démarre le service.
2. Une fois le canal joignable, l'app envoie `set_key` **par IPC** avec la clé de son trousseau.
3. Le service valide la clé auprès de l'API (`GET /user`), la range dans son `secrets.json`
   machine et réveille son superviseur.

La clé ne passe **jamais** par une ligne de commande (visible dans la liste des processus) ni par
un fichier temporaire. `clear_key` (bouton « Désactiver ») la fait oublier ; `uninstall` efface
toujours `secrets.json`.

**Plusieurs comptes dans l'app.** La clé transmise est celle du compte **actif** au moment de
l'activation. Le service reste à **clé unique** (`Core::authenticate_and_store` remplace, jamais
il ne garde d'autre clé) et **garde ce compte** quand l'app change de compte ou retire ce compte
de sa liste : il a sa propre copie. Pour lui faire surveiller un autre compte, on bascule l'app sur
ce compte puis « Surveiller le compte actif » (nouveau `set_key`).

### Attentes vis-à-vis de `secrets/file_store.rs`

Le service s'appuie sur le comportement durci annoncé pour le backend fichier :

- dossier = `MINESTRATOR_DATA_DIR` (toujours défini par le service) ; erreur explicite sinon,
  **jamais** de repli vers le répertoire temporaire ;
- `secrets.json` écrit atomiquement, sans suivre de lien symbolique, en `0600` sous Unix
  (droits trop ouverts corrigés à la lecture), chiffré DPAPI sous Windows.

Points souhaitables, non bloquants : sous Windows, que le fichier temporaire d'écriture atomique
soit créé **dans le même dossier** (il hérite alors de la DACL protégée) — c'est déjà le cas
aujourd'hui (`secrets.json.tmp`).

---

## 2. IPC app ↔ service

| OS | Point de contact | Contrôle d'accès |
|---|---|---|
| Linux | `/run/minestrator-daemon/daemon.sock` (`RuntimeDirectory=`) | socket `0666` mais **filtrage par uid du pair** (`SO_PEERCRED`) : root, le compte du service, les propriétaires |
| macOS | `/var/run/minestrator-daemon.sock` | idem (`getpeereid`) |
| Windows | `\\.\pipe\minestrator-daemon` | **DACL du canal** : `SYSTEM`, `Administrateurs`, le compte du service, les propriétaires ; clients distants refusés |

`MINESTRATOR_DAEMON_SOCKET` remplace le point de contact (dev, tests, daemon au premier plan).

**Protocole** (`core::service`) : une requête JSON par ligne (`{"op": …}`), une réponse JSON par
ligne (`{"type": …, "data": …}`). Requêtes : `ping`, `status`, `history`, `alerts`,
`get_config`, `set_config`, `set_key`, `clear_key`, `restart`. Requête ≤ 64 Kio, réponse
≤ 64 Mio, connexion fermée après 30 s d'inactivité, 16 connexions simultanées au plus. Une
version de protocole (`PROTOCOL_VERSION`) protège contre une app et un service désaccordés.
Le `Debug` des requêtes masque la clé.

**Propriétaires.** La liste des comptes autorisés est écrite **dans la définition du service**
(arguments `--owner` de l'unité systemd, du plist, ou de la ligne de commande SCM), donc gérée
par root/admin : le service ne peut pas s'autoriser un compte lui-même. Une réinstallation
(mise à jour) relit et conserve les propriétaires existants. Un compte non autorisé reçoit
`forbidden` ; « Activer » l'ajoute (élévation).

**Anti-usurpation.**
- Unix : le dossier du socket n'est inscriptible que par le service (`/run/minestrator-daemon`,
  `/var/run`) ; impossible d'y poser un faux socket.
- Windows : n'importe quel compte peut créer un canal nommé libre. Le service crée la première
  instance avec `FILE_FLAG_FIRST_PIPE_INSTANCE`, et **l'app vérifie à chaque connexion que le
  PID du serveur (`GetNamedPipeServerProcessId`) est celui du service enregistré auprès du SCM**.
  Un processus qui squatterait le nom (service arrêté) ne reçoit donc ni la clé ni la main sur
  la supervision (`untrusted`).

### Qui supervise ? (`core::service::decide`, testé)

Toutes les 10 s, l'app interroge `status` et décide :

| Situation | Superviseur |
|---|---|
| Service injoignable, accès refusé, protocole différent | **app** |
| Service sans clé, superviseur désactivé, ou aucun poll depuis 5 min (bloqué) | **app** |
| Service sur **un autre compte** MineStrator, ou compte pas encore résolu | **app** |
| Service sur **le même compte**, poll récent | **service** — superviseur local **en veille** |
| Pas de session dans l'app, service actif | service |

Dans le doute, l'app garde son superviseur : mieux vaut une alerte en double qu'une panne non
signalée. Le premier tour est fait **avant** de démarrer le superviseur local, pour qu'il n'ouvre
pas ses connexions monitor pour rien. La mise en veille ferme les connexions monitor ; la sortie
de veille les rouvre aussitôt.

**Changement de compte dans l'app** (`core/src/accounts.rs`) : la décision est refaite aussitôt,
avec le nouveau compte, **avant** de réveiller le superviseur local. Si le service surveille un
autre compte que le nouveau compte actif, le superviseur local sort de veille et supervise le
compte actif (le service continue de son côté) ; si l'app revient sur le compte du service, le
superviseur local se remet en veille. Les alertes du service ne sont relayées que tant qu'il
surveille le compte actif.

Quand le service supervise :
- **historique** : `metrics_history` fusionne l'historique local et celui du service ;
- **réglages** du superviseur : enregistrés localement et transmis au service (`set_config`) ;
- **alertes** : voir §3.

**Pilotage** : l'état côté OS (installé / arrêté / démarré) est lu sans privilège (SCM,
`systemctl is-active`, `launchctl print`) ; « Redémarrer » passe par IPC (un propriétaire suffit :
le service quitte et son gestionnaire le relance) ; installer, désinstaller, démarrer ou arrêter
exigent une élévation.

---

## 3. Notifications

Un service Windows tourne en **session 0** : il ne peut afficher ni toast ni fenêtre dans la
session de l'utilisateur. Sous Linux/macOS, un daemon système n'a pas non plus de bus de
notification de session. Stratégie :

1. **Le service consigne** chaque alerte dans sa base SQLite (table `alerts`, même rétention de
   14 jours que les métriques) et la journalise.
2. **Le tray de l'app relaie** : le lien côté app lit les alertes postérieures à son curseur
   (persisté dans `service_link.json`) et les republie sur le bus d'events du `Core` — même
   chemin qu'une alerte locale : **toast natif, centre d'alertes, diagnostic du Copilote**.
   Seules les alertes de moins de 10 min sont notifiées ; plus anciennes, elles n'arrivent que
   dans le centre d'alertes.
3. **Arriéré** : à l'ouverture, l'app récupère les alertes des dernières 24 h (`service_alerts`)
   et les range dans le centre d'alertes, sans toast (dédoublonnées).
4. **Push FCM** (mobile) : inchangé, le service pousse chaque alerte si `FCM_*` est configuré
   (voir `docs/PUSH.md`) — c'est la seule notification « app fermée » au sens strict.

Écartés : `WTSSendMessage` (boîte modale bloquante dans la session, intrusive) et un processus
d'aide lancé dans la session utilisateur par le service (`CreateProcessAsUser`, complexité et
surface d'attaque). Recommandation : lancer l'app au démarrage de session (réduite dans le tray)
pour que les alertes s'affichent dès l'ouverture de session — non implémenté.

**Limites connues en mode service** :
- le Copilote ne se déclenche que sur les **alertes** (crash, seuils) relayées ; pas sur les
  lignes WARN/ERROR de console ni sur la surcharge prolongée (ces signaux viennent des connexions
  monitor, qui vivent désormais dans le service) ;
- la pastille d'état des onglets n'est plus alimentée par les connexions monitor : elle ne
  bouge que pour les serveurs dont une vue live (aperçu, console) est ouverte.

---

## 4. Installeurs

Le service s'active au build par une **surcouche de configuration**, sans toucher au
`tauri.conf.json` par défaut :

```bash
cd desktop
npm run tauri build -- --config src-tauri/tauri.service.conf.json
```

`desktop/src-tauri/tauri.service.conf.json` (clés vérifiées dans le schéma officiel
`https://schema.tauri.app/config/2`, CLI 2.11) :
- `build.beforeBuildCommand` : `npm run build` puis `scripts/build-daemon-sidecar.mjs`, qui
  compile `minestrator-daemon` pour `TAURI_ENV_TARGET_TRIPLE` (binaire universel macOS par
  `lipo`) et le dépose dans `src-tauri/binaries/minestrator-daemon-<triple>[.exe]` (ignoré par
  git) ;
- `bundle.externalBin: ["binaries/minestrator-daemon"]` : Tauri livre le binaire **à côté de
  l'exécutable de l'app** (c'est là que `core::service::locate_daemon_binary` le cherche) ;
- `bundle.windows.nsis.installMode: "currentUser"` (aucun droit admin pour l'app), `languages`
  (anglais, français) + `installerHooks` ;
- `bundle.linux.deb.{postInstallScript, preRemoveScript, postRemoveScript}` et
  `bundle.linux.rpm.{postInstallScript, preRemoveScript}`.

Pourquoi une surcouche : `externalBin` exige que le binaire existe au moment du build (le
`tauri dev`, `cargo clippy` et la CI casseraient sinon). `.github/workflows/release.yml` la passe
au build desktop des trois OS ; le script de sidecar compile le daemon pour la bonne cible.

### Windows — NSIS (`installMode: "currentUser"`, `windows/hooks.nsh`)

**Aucun droit administrateur pour l'application** : elle s'installe, se met à jour et se
désinstalle pour l'utilisateur courant. Le « mode administrateur » est le **service** : en fin
d'installation, une question oui/non propose la supervision en arrière-plan.

Pourquoi pas `installMode: "both"` : Tauri le déclare avec `MULTIUSER_EXECUTIONLEVEL Highest`, donc
la demande UAC apparaît **au lancement**, avant même le choix « pour moi seulement ». Le contourner
exigerait de forker le modèle NSIS de Tauri, et le désinstalleur (même niveau d'exécution que
l'installeur) ne saurait plus s'élever pour une installation machine.

| Moment | Action |
|---|---|
| `NSIS_HOOK_POSTINSTALL` (ni mise à jour, ni `/P`, ni `/S`, service absent) | question oui/non ; « oui » écrit `HKCU\Software\MinestratorTerminal\ServiceSetupRequested = 1` |
| première connexion dans l'app | `core::service::take_pending_setup` consomme la valeur → `service_enable` (UAC, copie protégée, transmission de la clé) ; résultat dans le centre d'alertes |
| `NSIS_HOOK_PREUNINSTALL`, mise à jour (`$UpdateMode = 1`) | rien : le service n'exécute pas les fichiers de l'app |
| `NSIS_HOOK_PREUNINSTALL`, désinstallation, service installé | `ExecShellWait runas … uninstall` (UAC) ; `--purge` si « supprimer les données » est coché ; ignoré en mode silencieux |

**Copie protégée du binaire.** Un service système ne doit jamais exécuter un binaire modifiable
par l'utilisateur (`%LOCALAPPDATA%`). `minestrator-daemon install`/`enable`, lancé élevé, recopie
donc le binaire dans `%ProgramFiles%\MinestratorTerminal\Service\` (écriture réservée aux
administrateurs) et enregistre **cette copie** ; une app installée sous `Program Files` (MSI) est
enregistrée telle quelle. `uninstall` retire la copie. Conséquence : une mise à jour de l'app ne
met pas le service à jour — `ServiceOverview.outdated` le signale et « Mettre à jour le service »
(Réglages, `service_update`) relance `install` élevé (propriétaires conservés).

Service : `MinestratorDaemon`, compte `NT AUTHORITY\LocalService`, démarrage automatique différé,
redémarrage sur échec (5 s, 15 s, 60 s), y compris sur arrêt volontaire avec code d'erreur (c'est
ainsi que « Redémarrer » fonctionne). Journaux :
`%ProgramData%\MinestratorTerminal\service\daemon.log` (5 Mio max, tronqué au démarrage).

**MSI (toujours machine)** — non câblé, faisable de deux façons :
1. **Sans rien changer** : le binaire est installé sous `Program Files` avec l'app ; le bouton
   « Activer » des réglages installe le service par élévation UAC (`enable --owner <SID>`). La
   désinstallation du MSI laisse toutefois le service en place : il faut « Désinstaller le
   service » dans l'app avant, ou ajouter (2).
2. **Fragment WiX** (`bundle.windows.wix.fragmentPaths` + `componentRefs`/`featureRefs`) avec
   deux actions personnalisées différées sans emprunt d'identité (`Execute="deferred"`,
   `Impersonate="no"`) : `"[INSTALLDIR]minestrator-daemon.exe" install` après `InstallFiles`
   et `… uninstall` avant `RemoveFiles` sur `REMOVE="ALL"`. `ServiceInstall`/`ServiceControl`
   natifs ne conviennent pas : ils doivent appartenir au composant qui contient le binaire, que
   Tauri génère lui-même.

### Linux — `.deb` / `.rpm` (systemd)

| Script | Action |
|---|---|
| deb `postinst configure` / rpm `%post` | `minestrator-daemon install` (si systemd) : écrit `/etc/systemd/system/minestrator-daemon.service`, `enable`, `restart` |
| deb `prerm remove` / rpm `%preun` (`$1 = 0`) | `uninstall` : `disable --now`, suppression de l'unité et de la clé machine |
| deb `prerm upgrade` | `stop` (le postinst de la nouvelle version redémarre) |
| rpm `%preun` mise à jour (`$1 ≥ 1`) | rien : le `%post` de la nouvelle version a déjà tourné (ordre RPM) |
| deb `postrm purge` | `rm -rf /var/lib/minestrator-daemon /var/lib/private/minestrator-daemon` |

RPM n'a pas de « purge » : l'historique reste (effacer avec `uninstall --purge` avant
`dnf remove`). L'unité utilise `DynamicUser=yes`, `ProtectSystem=strict`, `ProtectHome=yes`,
`NoNewPrivileges=yes`, `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6`… Journaux : journald
(`journalctl -u minestrator-daemon`).

**AppImage** : pas de script d'installation. « Activer » lance `pkexec minestrator-daemon enable
--owner <uid>`. root ne pouvant pas lire le montage FUSE de l'AppImage, l'app copie d'abord le
binaire dans un dossier privé (`$XDG_RUNTIME_DIR`), et la commande élevée le recopie dans
`/usr/local/lib/minestrator-daemon/` (chemin stable, réservé à root) avant d'écrire l'unité.
Sans `pkexec` (pas d'agent polkit), les réglages affichent la commande `sudo` à copier.

### macOS — DMG (LaunchDaemon)

Le DMG n'offre pas de choix d'installation : l'app est en mode utilisateur jusqu'à « Activer »,
qui lance `osascript … with administrator privileges` sur `minestrator-daemon enable --owner
<uid>`. La commande copie le binaire dans `/Library/PrivilegedHelperTools/
com.geoffreylecoq.minestratorterminal.daemon` (pas dans le `.app`, que l'utilisateur peut
déplacer), écrit `/Library/LaunchDaemons/com.geoffreylecoq.minestratorterminal.daemon.plist`
(`RunAtLoad`, `KeepAlive`) et le charge (`launchctl bootstrap system`). Journaux :
`/Library/Logs/MinestratorTerminal/daemon.log`. Le binaire copié garde la signature du bundle
(à vérifier avec la notarisation).

---

## 5. UI — Réglages › Service d'arrière-plan

`desktop/src/lib/components/settings/ServiceSection.svelte` (i18n : espace de noms `service.*`,
fr + en). Affiche l'état du service, qui supervise, le compte surveillé (en précisant s'il s'agit
du compte actif de l'app ou d'un autre — `same_account`), l'activité (serveurs surveillés, dernier
relevé), la version, le dossier de données, le push FCM. Réglages → Comptes signale aussi le compte
surveillé par le service. Quand le service surveille un autre compte, « Activer » devient
« Surveiller le compte actif ». Actions selon les droits :

| Action | Droits | Effet |
|---|---|---|
| Activer | élévation si le service n'est pas installé ou si le compte n'est pas autorisé | `enable --owner`, puis `set_key` + `set_config` par IPC |
| Désactiver | propriétaire | `clear_key` : le service reste installé mais ne supervise plus ; l'app reprend la main |
| Redémarrer | propriétaire | `restart` par IPC |
| Désinstaller le service (± purge) | élévation | `uninstall [--purge]` |

En mode utilisateur (Windows, app dans le profil), la section explique le fonctionnement actuel
(superviseur dans l'app + tray) et comment passer en mode machine. Le mobile n'est pas concerné.

---

## 6. Désinstallation propre

| Chemin | Service | Clé machine | Historique + journaux |
|---|---|---|---|
| App › « Désinstaller le service » | supprimé | effacée | conservés (effacés si « purge » coché) |
| Désinstalleur NSIS (si le service est installé, UAC) | supprimé | effacée | effacés si « supprimer les données » coché |
| deb `remove` / `purge` | supprimé | effacée | conservés / effacés |
| rpm `remove` | supprimé | effacée | conservés |
| `minestrator-daemon uninstall [--purge]` | supprimé | effacée | conservés / effacés |

`uninstall` supprime aussi la copie du binaire (`%ProgramFiles%\MinestratorTerminal\Service`,
`/usr/local/lib/minestrator-daemon`, `/Library/PrivilegedHelperTools/…`).

---

## Vérification

Testé automatiquement (sans privilège) :
- protocole IPC (sérialisation, trames bornées, masquage de la clé), décision « qui supervise »,
  fusion d'historique, validation des uid/SID, citations shell/AppleScript/Windows
  (`minestrator-core`, module `service`) ;
- journal d'alertes SQLite et curseur (`store`) ;
- génération **et relecture** de l'unité systemd et du plist launchd, ligne de commande SCM,
  ligne de commande du daemon, chaînes SDDL (`minestrator-daemon`) ;
- **bout en bout** : vrai serveur IPC (canal nommé avec DACL sous Windows, socket Unix ailleurs)
  + client du core ; refus `forbidden` d'un compte absent de la DACL (Windows).

Ces tests passent sous Windows et sous Linux (conteneur Debian, `cargo clippy` et `cargo test`
de `minestrator-core` et `minestrator-daemon`, socket Unix réel). Rien n'a été compilé pour
macOS.

Vérifié à la main : un `tauri build --debug --bundles nsis --config
src-tauri/tauri.service.conf.json` (CLI 2.11.4) accepte la configuration fusionnée, compile les
hooks avec `makensis`, et l'installeur généré déclare `INSTALLMODE "both"`, inclut
`windows/hooks.nsh` et livre le binaire en `$INSTDIR\minestrator-daemon.exe`. L'installeur n'a
pas été exécuté.

Non testé (exige des droits administrateur, une installation réelle ou un autre OS) :
installation/désinstallation effectives du service (SCM, systemd, launchd), hooks NSIS à
l'exécution, scripts deb/rpm, élévations (UAC, pkexec, osascript), DACL du dossier
`%ProgramData%` appliquée par `LocalService`, vérification du PID serveur face au vrai SCM,
redémarrage par les actions de récupération Windows, `DynamicUser` et durcissement systemd,
copie du binaire depuis une AppImage, notarisation macOS du binaire copié.
