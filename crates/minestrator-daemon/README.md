# minestrator-daemon

Service d'arrière-plan de Minestrator Terminal, **un seul binaire pour Windows, Linux et
macOS**. Il réutilise `minestrator-core` : il démarre le **superviseur** (crash, seuils
CPU/RAM/disque, expiration MyBox), **consigne** chaque alerte dans sa base SQLite, la **sert à
l'app** par IPC (socket Unix / canal nommé Windows) pour que le tray l'affiche, et la **pousse
en FCM** si configuré. C'est ce qui permet d'être prévenu app fermée.

> Conception complète (secrets, IPC, notifications, installeurs, désinstallation) :
> [`../../docs/SERVICE.md`](../../docs/SERVICE.md). Push mobile : [`../../docs/PUSH.md`](../../docs/PUSH.md).

## Modes

| Commande | Usage | Droits |
|---|---|---|
| `minestrator-daemon [run] [--owner ID]...` | premier plan : VPS, conteneur, dev | aucun |
| `minestrator-daemon service [--owner ID]...` | lancé par systemd / launchd / le SCM | — |
| `minestrator-daemon install [--owner ID]... [--no-start]` | installe ou met à jour le service | admin |
| `minestrator-daemon enable --owner ID` | installe si besoin, autorise `ID`, démarre | admin |
| `minestrator-daemon uninstall [--purge]` | supprime le service et la clé machine (`--purge` : aussi l'historique) | admin |
| `minestrator-daemon start` \| `stop` \| `restart` | pilote le service installé | admin |
| `minestrator-daemon status [--json]` | état côté OS + état rapporté par IPC | aucun |

`ID` : uid (Linux/macOS) ou SID (Windows) d'un compte autorisé sur le canal IPC. En service,
c'est l'app qui l'ajoute (bouton « Activer » des réglages, avec élévation), puis transmet la
clé API par IPC : rien à configurer à la main.

Codes de sortie des commandes d'administration : `0` succès, `1` échec, `2` usage, `3` binaire
dans un emplacement utilisateur (Windows, installation « pour moi seulement »), `4` droits
administrateur requis.

## Dossier de données

La clé API (`secrets.json`), l'historique (`metrics.db`, alertes) et les réglages vivent dans :

1. `MINESTRATOR_DATA_DIR` s'il est défini (l'unité systemd et le plist le posent) ;
2. sinon, en service ou en root, l'emplacement machine protégé :
   - Linux : `/var/lib/minestrator-daemon` (`StateDirectory=`, utilisateur dynamique, 0700) ;
   - macOS : `/Library/Application Support/MinestratorTerminal/service` (0700, root) ;
   - Windows : `%ProgramData%\MinestratorTerminal\service` (DACL : SYSTEM, Administrateurs,
     LocalService) ;
3. sinon (premier plan sans privilège), le dossier de données de l'utilisateur
   (`…/MinestratorTerminal/daemon`).

**Jamais le répertoire temporaire** (partagé entre utilisateurs) : c'était le défaut des
versions précédentes. Le dossier est créé en 0700 sous Unix.

## Configuration (variables d'environnement)

| Variable | Requis | Rôle |
|---|---|---|
| `MINESTRATOR_API_KEY` | — | Clé API validée au démarrage (usage VPS). En service, la clé arrive par IPC. |
| `MINESTRATOR_DATA_DIR` | — | Dossier données/secrets (voir ci-dessus). |
| `MINESTRATOR_DAEMON_SOCKET` | — | Point de contact IPC (défaut : `/run/minestrator-daemon/daemon.sock`, `/var/run/minestrator-daemon.sock`, `\\.\pipe\minestrator-daemon`). |
| `FCM_PROJECT_ID` | pour push | ID du projet Firebase. Absent → alertes consignées et journalisées, sans push. |
| `FCM_ACCESS_TOKEN` | pour push | Access token OAuth2 (scope `firebase.messaging`). |
| `FCM_ACCESS_TOKEN_FILE` | alt. | Fichier contenant l'access token (rafraîchi par un helper). |
| `DEVICE_TOKENS_FILE` | pour push | JSON — tableau des tokens d'appareil FCM. Relu à chaque alerte. |
| `RUST_LOG` | — | Filtre des journaux (défaut `minestrator_daemon=info,minestrator_core=info,warn`). |

En service, ces variables se posent par `systemctl edit minestrator-daemon` (`Environment=`),
dans le plist (`EnvironmentVariables`) ou dans la valeur `Environment` de la clé de registre du
service Windows.

> **Access token FCM** : le daemon lit un token déjà obtenu. En production, le générer depuis un
> **compte de service** (JWT signé, ex. crate `gcp_auth`) et le rafraîchir toutes les ~55 min.
> Détails dans `docs/PUSH.md`.

## Lancer au premier plan (VPS)

```bash
export MINESTRATOR_API_KEY="<ta clé>"
export MINESTRATOR_DATA_DIR="/var/lib/minestrator-daemon"
export FCM_PROJECT_ID="mon-projet"
export FCM_ACCESS_TOKEN="ya29...."           # ou FCM_ACCESS_TOKEN_FILE
export DEVICE_TOKENS_FILE="/var/lib/minestrator-daemon/devices.json"

cargo run -p minestrator-daemon --release
# ou le binaire : target/release/minestrator-daemon
```

Sans `FCM_*`, le daemon tourne quand même, **consigne et journalise** les alertes — utile pour
tester la surveillance avant de brancher Firebase. Sans canal IPC disponible (socket
non inscriptible), il continue sans : l'IPC ne sert qu'à l'app de bureau.

## Installer comme service (sans l'app)

```bash
cargo build -p minestrator-daemon --release
sudo target/release/minestrator-daemon enable --owner "$(id -u)"   # Linux / macOS
target/release/minestrator-daemon status
```

Sous Windows (terminal administrateur), le binaire doit se trouver sous `Program Files` ; pour
un essai depuis `target\`, ajouter `--allow-unsafe-path` (jamais en production : un service
système ne doit pas exécuter un binaire modifiable par un utilisateur).

## Journaux

- Linux : `journalctl -u minestrator-daemon` ;
- macOS : `/Library/Logs/MinestratorTerminal/daemon.log` ;
- Windows : `%ProgramData%\MinestratorTerminal\service\daemon.log` (5 Mio max, tronqué au
  démarrage) ;
- premier plan : stderr.
