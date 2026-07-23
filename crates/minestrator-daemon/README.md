# minestrator-daemon

Daemon Linux qui réutilise `minestrator-core` : il démarre le **superviseur** (crash, seuils
CPU/RAM/disque, expiration MyBox) et **pousse chaque alerte en notification FCM** aux appareils
enregistrés. C'est ce qui permet d'être prévenu **app fermée** — impossible depuis le mobile seul
(pas de tâche de fond persistante sur Android/iOS).

> Le bout « recevoir le push » côté app se câble après `tauri android init` + un projet Firebase.
> Guide complet : [`../../docs/PUSH.md`](../../docs/PUSH.md).

## Configuration (variables d'environnement)

| Variable | Requis | Rôle |
|---|---|---|
| `MINESTRATOR_API_KEY` | ✅ | Clé API MineStrator (validée au démarrage). |
| `MINESTRATOR_DATA_DIR` | — | Dossier données/secrets du daemon (défaut : temp). |
| `FCM_PROJECT_ID` | pour push | ID du projet Firebase. Absent → alertes seulement journalisées. |
| `FCM_ACCESS_TOKEN` | pour push | Access token OAuth2 (scope `firebase.messaging`). |
| `FCM_ACCESS_TOKEN_FILE` | alt. | Fichier contenant l'access token (rafraîchi par un helper). |
| `DEVICE_TOKENS_FILE` | pour push | JSON — tableau des tokens d'appareil FCM. Relu à chaque alerte. |

> **Access token** : le scaffold lit un token déjà obtenu. En production, le générer depuis un
> **compte de service** (JWT signé, ex. crate `gcp_auth`) et le rafraîchir toutes les ~55 min.
> Détails et exemple `systemd` dans `docs/PUSH.md`.

## Lancer

```bash
export MINESTRATOR_API_KEY="<ta clé>"
export MINESTRATOR_DATA_DIR="/var/lib/minestrator-daemon"
export FCM_PROJECT_ID="mon-projet"
export FCM_ACCESS_TOKEN="ya29...."           # ou FCM_ACCESS_TOKEN_FILE
export DEVICE_TOKENS_FILE="/var/lib/minestrator-daemon/devices.json"

cargo run -p minestrator-daemon --release
# ou le binaire : target/release/minestrator-daemon
```

Sans `FCM_*`, le daemon tourne quand même et **journalise** les alertes — utile pour tester la
surveillance avant de brancher Firebase.
