# Référence de l'API MineStrator (documentée et non documentée)

> Généré par `scripts/api-watch/reference.py` — ne pas éditer à la main. Relevé du **2026-11-18**.

La [documentation officielle](https://api.minestrator.com/openapi-public-fr.yaml) ne couvre qu'une
partie de l'API. Celle-ci recense **tous** les endpoints appelés par le panel web (JavaScript
public du panel, plus les appels observés en le parcourant en lecture seule), indique lesquels sont
documentés, et lesquels ont été vérifiés avec une **clé API**.

## En bref

- **314 chemins** recensés, dont **283 absents de la documentation officielle**.
- **79 lectures** testées avec une clé API répondent normalement ; seules les données de
  paiement et les passkeys sont réservées à la session web (`403 API_FORBIDDEN`). Une clé API a donc,
  en pratique, le même périmètre que le panel. Les écritures n'ont **pas** été testées (aucune action
  n'a été déclenchée), mais rien n'indique un traitement différent.
- Le **mot de passe SFTP** ne figure plus dans `GET /server/{id_server}` pour une clé API, mais
  `GET /server/{id_server}/sftp` le renvoie toujours.
- Une **API fichiers REST** complète existe (`/server/{id_server}/files/…` : lister, lire, écrire,
  créer, déplacer, renommer, supprimer, compresser, extraire, rechercher) : une alternative au SFTP.
- Autres endpoints utiles : état groupé des serveurs (`POST /servers/live/light/bulk`), tâches
  planifiées du panel (`/server/{id_server}/schedule…`), statistiques sur une période, joueurs hors
  ligne et inventaires, sous-utilisateurs et bases de données d'une MyBox.

## Conventions

- Base : `https://mine.sttr.io`. Authentification : `Authorization: Bearer <clé API>` (créée dans le
  panel : Compte → Clés API). Réponses dans l'enveloppe `{ api: { code, endpoint, data, error } }`.
- **Doc.** : ✓ si l'endpoint figure dans la documentation officielle.
- **Clé API** : ✓ méthode testée avec succès avec une clé API (lecture) ; ✗ refusée ; « non testé » sinon.
- `?` : méthode non déterminée par l'analyse du panel (souvent un `GET` de recherche).
- Ces endpoints non documentés peuvent changer sans préavis : la surveillance quotidienne
  (`.github/workflows/api-watch.yml`) vérifie ceux dont l'app dépend.

## Compte

### Connexion et sécurité

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| POST | `/user/discord/authorization/url` | — | écriture, non testé |  |
| POST | `/user/login` | — | écriture, non testé |  |
| POST | `/user/login/2fa` | — | écriture, non testé |  |
| POST | `/user/login/discord` | — | écriture, non testé |  |
| POST | `/user/logout` | — | écriture, non testé |  |
| POST | `/user/oauth/google/authorization/url` | — | écriture, non testé |  |
| POST | `/user/register` | — | écriture, non testé |  |
| POST | `/user/register/discord` | — | écriture, non testé |  |
| ? | `/user/{id_user}/2fa/disable` | — | écriture, non testé |  |
| ? | `/user/{id_user}/2fa/enable` | — | écriture, non testé |  |
| GET | `/user/{id_user}/2fa/setup` | — | non testé | Non testé : un `GET` qui peut générer un nouveau secret 2FA. |
| GET / POST / PATCH | `/user/{id_user}/apikey` | — | non testé | Gestion de la clé API (une clé principale, sans permissions par portée). |
| PATCH | `/user/{id_user}/apikey/note` | — | écriture, non testé |  |
| PATCH / DELETE | `/user/{id_user}/apikey/{id}` | — | écriture, non testé |  |
| POST | `/user/{id_user}/delete` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/delete/{id}` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/discord/account` | — | écriture, non testé |  |
| POST | `/user/{id_user}/email` | — | écriture, non testé |  |
| POST | `/user/{id_user}/email/validate` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/email/validate/{id}` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/email/{id}` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/oauth/google/account` | — | écriture, non testé |  |
| POST | `/user/{id_user}/passkey/registration/options` | — | écriture, non testé |  |
| POST | `/user/{id_user}/passkey/registration/verify` | — | écriture, non testé |  |
| PATCH / DELETE | `/user/{id_user}/passkey/{id}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/passkeys` | — | ✗ GET (403 API_FORBIDDEN) | Refusé à une clé API (`403 API_FORBIDDEN`). |
| POST | `/user/{id_user}/password` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/password/{id}` | — | écriture, non testé |  |
| GET / DELETE | `/user/{id_user}/sessions` | — | ✓ GET |  |
| DELETE | `/user/{id_user}/sessions/{id}` | — | écriture, non testé |  |

### Crédits, achats et programmes

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/payment-methods` | — | ✗ GET (403 API_FORBIDDEN) | Refusé à une clé API (`403 API_FORBIDDEN`). |
| GET / POST / PUT / PATCH | `/user/{id_user}/affiliate` | — | ✓ GET |  |
| GET | `/user/{id_user}/credits` | — | ✓ GET |  |
| POST | `/user/{id_user}/credits/transfer` | — | écriture, non testé |  |
| GET | `/user/{id_user}/dailydrop` | — | ✓ GET |  |
| POST | `/user/{id_user}/gift` | — | écriture, non testé |  |
| GET | `/user/{id_user}/gifts` | — | ✓ GET |  |
| POST | `/user/{id_user}/gifts/physical/address` | — | écriture, non testé |  |
| GET | `/user/{id_user}/goodies/transactions` | — | ✓ GET |  |
| GET | `/user/{id_user}/hosting-program` | — | ✓ GET |  |
| POST | `/user/{id_user}/hosting-program/accept` | — | écriture, non testé |  |
| POST | `/user/{id_user}/hosting-program/refuse` | — | écriture, non testé |  |
| GET | `/user/{id_user}/partnership` | — | ✓ GET |  |
| POST | `/user/{id_user}/partnership/message` | — | écriture, non testé |  |
| POST | `/user/{id_user}/partnership/messages/readed` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/subscription/{id}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/subscriptions` | — | ✓ GET |  |
| GET | `/user/{id_user}/transaction/{id}/pdf` | — | non testé |  |
| DELETE | `/user/{id_user}/transaction/{id}/refund` | — | écriture, non testé |  |
| GET | `/user/{id_user}/transactions` | — | ✓ GET |  |

### Profil et préférences

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/user` | ✓ | ✓ GET |  |
| PATCH | `/user/{id_user}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/activity` | — | ✓ GET |  |
| PATCH | `/user/{id_user}/alert` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/language` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/minecraft/account` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/notification` | — | écriture, non testé |  |
| GET / PATCH | `/user/{id_user}/notifications` | — | ✓ GET |  |
| GET | `/user/{id_user}/notifications/unreaded` | — | ✓ GET |  |
| GET / PATCH | `/user/{id_user}/preferences/{id}` | — | non testé |  |

### Serveurs, sauvegardes et archives

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| ? | `/user/{id_user}/archive` | — | écriture, non testé |  |
| ? | `/user/{id_user}/archive/download/{chemin}` | — | écriture, non testé |  |
| ? | `/user/{id_user}/archive/{id}` | — | écriture, non testé |  |
| ? | `/user/{id_user}/archive_v2` | — | écriture, non testé |  |
| ? | `/user/{id_user}/archive_v2/download/{chemin}` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/archive_v2/{id}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/archives` | — | ✓ GET |  |
| POST / PATCH | `/user/{id_user}/ark/cluster` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/ark/cluster/{id}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/ark/clusters` | — | ✓ GET |  |
| GET | `/user/{id_user}/backup_job/{id}` | — | non testé |  |
| GET | `/user/{id_user}/backup_jobs/active` | — | ✓ GET |  |
| GET | `/user/{id_user}/database_archive_v2/download/{chemin}` | — | non testé |  |
| DELETE | `/user/{id_user}/database_archive_v2/{id}` | — | écriture, non testé |  |
| POST | `/user/{id_user}/migration` | — | écriture, non testé |  |
| GET | `/user/{id_user}/migrations` | — | ✓ GET |  |
| GET / POST | `/user/{id_user}/pimpmybox` | — | ✓ GET |  |
| DELETE | `/user/{id_user}/pimpmybox/{id}` | — | écriture, non testé |  |
| GET | `/user/{id_user}/servers` | ✓ | ✓ GET |  |
| GET | `/user/{id_user}/servers/websocket` | — | ✓ GET | Jetons WebSocket (Wings) de tous les serveurs actifs en un appel. Utilisé par l'app pour le rafraîchissement des jetons. |
| POST / PATCH | `/user/{id_user}/servers_groups` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/servers_groups/assign` | — | écriture, non testé |  |
| DELETE | `/user/{id_user}/servers_groups/{id}` | — | écriture, non testé |  |
| POST / PUT | `/user/{id_user}/snapshot` | ✓ | écriture, non testé |  |
| ? | `/user/{id_user}/snapshot/download/{chemin}` | — | écriture, non testé |  |
| ? | `/user/{id_user}/snapshot/legacy` | — | écriture, non testé |  |
| ? | `/user/{id_user}/snapshot/legacy/download/{chemin}` | — | écriture, non testé |  |
| ? | `/user/{id_user}/snapshot/legacy/{id}` | — | écriture, non testé |  |
| PATCH / DELETE | `/user/{id_user}/snapshot/{id}` | ✓ | écriture, non testé |  |
| GET | `/user/{id_user}/snapshots` | ✓ | ✓ GET |  |
| GET | `/user/{id_user}/tasks` | — | ✓ GET | Tâches en cours du compte (`global`, `nodes`, `targeted`). |

### Support et retours

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/user/{id_user}/ideas` | — | ✓ GET |  |
| POST | `/user/{id_user}/location-pings` | — | écriture, non testé |  |
| POST / PATCH | `/user/{id_user}/ticket` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/ticket/close` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/ticket/rate` | — | écriture, non testé |  |
| PATCH | `/user/{id_user}/ticket/reopen` | — | écriture, non testé |  |
| GET | `/user/{id_user}/ticket/{id}` | — | non testé |  |
| GET | `/user/{id_user}/tickets` | — | ✓ GET |  |

## MyBox

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| ? | `/mybox/downgrade` | — | écriture, non testé |  |
| POST | `/mybox/subscription` | — | écriture, non testé |  |
| ? | `/mybox/type/downgrade` | — | écriture, non testé |  |
| GET / DELETE | `/mybox/{id_mybox}` | ✓ | ✓ GET |  |
| GET | `/mybox/{id_mybox}/activity` | — | ✓ GET |  |
| POST | `/mybox/{id_mybox}/database` | — | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/database/description` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/database/disk` | — | ✓ GET |  |
| PATCH | `/mybox/{id_mybox}/database/password` | — | écriture, non testé |  |
| DELETE | `/mybox/{id_mybox}/database/{id}` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/database/{id}/backups` | — | non testé |  |
| GET | `/mybox/{id_mybox}/database/{id}/backups/{id}/download` | — | non testé |  |
| POST | `/mybox/{id_mybox}/database/{id}/backups/{id}/restore` | — | écriture, non testé |  |
| POST | `/mybox/{id_mybox}/database/{id}/pma-token` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/disk` | — | ✓ GET |  |
| POST | `/mybox/{id_mybox}/free/renew` | — | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/name` | — | écriture, non testé |  |
| POST | `/mybox/{id_mybox}/option` | — | écriture, non testé |  |
| DELETE | `/mybox/{id_mybox}/option/{id}` | — | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/owner` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/players` | — | ✓ GET |  |
| DELETE | `/mybox/{id_mybox}/refund` | — | écriture, non testé |  |
| POST | `/mybox/{id_mybox}/review` | — | écriture, non testé |  |
| POST / PATCH | `/mybox/{id_mybox}/server` | ✓ | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/server/disable` | ✓ | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/server/enable` | ✓ | écriture, non testé |  |
| DELETE | `/mybox/{id_mybox}/server/{id_server}` | ✓ | écriture, non testé |  |
| POST / DELETE | `/mybox/{id_mybox}/server/{id_server}/dedicated_ip` | — | écriture, non testé |  |
| POST | `/mybox/{id_mybox}/server/{id_server}/snapshot-before-delete` | — | écriture, non testé |  |
| PATCH | `/mybox/{id_mybox}/servers/order` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/smartscan` | — | non testé |  |
| GET | `/mybox/{id_mybox}/stats/{début}/{fin}` | — | ✓ GET | Historique de la MyBox ; même format de période. |
| POST / PATCH | `/mybox/{id_mybox}/subuser` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/subuser/permissions` | — | ✓ GET |  |
| POST | `/mybox/{id_mybox}/subuser/preset` | — | écriture, non testé |  |
| PATCH / DELETE | `/mybox/{id_mybox}/subuser/preset/{id}` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/subuser/presets` | — | ✓ GET |  |
| DELETE | `/mybox/{id_mybox}/subuser/{id}` | — | écriture, non testé |  |
| GET | `/mybox/{id_mybox}/websocket` | — | ✓ GET |  |

## Serveurs

### Configuration et démarrage

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| PATCH | `/server/{id_server}/docker/image` | — | écriture, non testé |  |
| GET | `/server/{id_server}/docker/images` | — | ✓ GET |  |
| PATCH | `/server/{id_server}/java/version` | — | écriture, non testé |  |
| GET | `/server/{id_server}/minecraft_version` | — | ✓ GET |  |
| PATCH | `/server/{id_server}/php/version` | — | écriture, non testé |  |
| POST | `/server/{id_server}/pod/mariadb/database` | — | écriture, non testé |  |
| GET / PATCH | `/server/{id_server}/properties` | ✓ | ✓ GET |  |
| POST | `/server/{id_server}/reinstallation` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/startup/file` | — | écriture, non testé |  |
| GET | `/server/{id_server}/startup/files` | — | ✓ GET |  |
| PATCH | `/server/{id_server}/startup/params` | — | écriture, non testé |  |
| POST | `/server/{id_server}/version` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/world/active` | — | écriture, non testé |  |
| GET | `/server/{id_server}/worlds` | — | ✓ GET |  |

### Fichiers

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/server/{id_server}/files/bulk-upload-token` | — | non testé | Non testé : génère un jeton d'envoi. |
| GET / POST | `/server/{id_server}/files/compress/jobs` | — | ✓ GET |  |
| GET | `/server/{id_server}/files/compress/jobs/{id}` | — | non testé |  |
| PATCH | `/server/{id_server}/files/content` | — | écriture, non testé | Écriture d'un fichier (corps non relevé). |
| GET | `/server/{id_server}/files/content/{chemin}` | — | ✓ GET | Contenu d'un fichier : `{ content, encoding }`. |
| POST | `/server/{id_server}/files/create/file` | — | écriture, non testé |  |
| POST | `/server/{id_server}/files/create/folder` | — | écriture, non testé |  |
| POST | `/server/{id_server}/files/create/folders` | — | écriture, non testé |  |
| POST | `/server/{id_server}/files/delete` | — | écriture, non testé |  |
| GET | `/server/{id_server}/files/download/{chemin}` | — | non testé |  |
| POST | `/server/{id_server}/files/exists` | — | ✓ POST | Corps `{ files: [chemins] }` → `{ exists: { chemin: bool } }`. |
| POST | `/server/{id_server}/files/extract` | — | écriture, non testé |  |
| POST | `/server/{id_server}/files/gzread` | — | écriture, non testé |  |
| GET | `/server/{id_server}/files/list/{chemin}` | — | ✓ GET | API fichiers REST : liste d'un dossier (`{chemin}` vide = racine), sans SFTP. |
| PUT | `/server/{id_server}/files/move` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/files/rename` | — | écriture, non testé |  |
| GET | `/server/{id_server}/files/search` | — | non testé |  |
| GET | `/server/{id_server}/files/upload/url` | — | non testé | Non testé : génère une URL d'envoi. |

### Joueurs et commandes

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| PUT | `/server/{id_server}/command` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/ban` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/inventory` | — | écriture, non testé | Édition d'un joueur **connecté** : `{ player, slot (hotbar.N, inventory.N, armor.head…, weapon.offhand), item, count, namespaced?, enchantments?, syntax? }` ; le panel confirme en relisant la console. Non repris par l'app. |
| PUT | `/server/{id_server}/command/minecraft/kick` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/op/add` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/op/remove` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/unban` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/whitelist/add` | ✓ | écriture, non testé |  |
| PUT | `/server/{id_server}/command/minecraft/whitelist/remove` | ✓ | écriture, non testé |  |
| GET | `/server/{id_server}/players/offline` | — | ✓ GET | Joueurs déjà venus (dossier `playerdata` du monde). `?limit` (200 au plus) `&offset` `&search` (sous-chaîne du pseudo : `total` = correspondances, `total_all` = tous). **Toujours trié par dernière sauvegarde décroissante** (aucun paramètre de tri). `play_time` en ticks (20 par seconde), `mtime` en secondes Unix. Utilisé par l'app (`players.rs`). |
| GET | `/server/{id_server}/players/{id}/inventory` | — | ✓ GET | Inventaire lu dans le fichier du joueur : `inventory` (emplacements `0`-`8` barre d'action, `9`-`35` sac), `equipment` (`head`, `chest`, `legs`, `feet`, `offhand`), objets `{ id, count, components }` ; `online.is_online` (le fichier peut dater de la dernière sauvegarde). Utilisé par l'app, en lecture seule. |
| PUT | `/server/{id_server}/players/{id}/inventory/slots` | — | écriture, non testé | Édition d'un joueur **hors ligne** : `{ sha256 (celui de la lecture), ops: [{ slot: hotbar.N, inventory.N, armor.head…, weapon.offhand ; item ; count ; enchantments? }] }`, conflit si le fichier a changé. Non repris par l'app. |
| GET | `/server/{id_server}/players/{id}/profile` | — | ✓ GET | Fiche d'un joueur (`{id}` = UUID) : `times` (première venue, dernière connexion, dernière venue), `state` (`game_mode` 0-3, vie, faim, niveau), `stats` (`play_time` en ticks, sessions, morts, éliminations ; `null` si jamais incrémentées ; `distances` en cm par mode). Erreurs `API_PLAYERDATA_*`. Utilisé par l'app. |
| PUT | `/server/{id_server}/poweraction` | ✓ | écriture, non testé |  |
| PATCH | `/server/{id_server}/poweraction/messages` | ✓ | écriture, non testé |  |

### Mods, plugins et contenus

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| POST | `/server/{id_server}/ark/cluster` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/ark/cluster/{id}` | — | écriture, non testé |  |
| POST | `/server/{id_server}/bedrock/addon` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/bedrock/addon/{id}` | — | écriture, non testé |  |
| GET | `/server/{id_server}/bedrock/addons` | — | non testé |  |
| POST | `/server/{id_server}/curseforge/bedrock/addon` | — | écriture, non testé |  |
| POST | `/server/{id_server}/curseforge/datapack` | — | écriture, non testé |  |
| POST | `/server/{id_server}/curseforge/hytale/mod` | — | écriture, non testé |  |
| POST | `/server/{id_server}/curseforge/mod` | — | écriture, non testé |  |
| POST | `/server/{id_server}/curseforge/world` | — | écriture, non testé |  |
| GET | `/server/{id_server}/datapacks` | — | ✓ GET |  |
| POST | `/server/{id_server}/datapacks/updates` | — | écriture, non testé |  |
| GET / PATCH | `/server/{id_server}/enshrouded/groups` | — | non testé |  |
| PATCH | `/server/{id_server}/hytale/mod` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/hytale/mod/{id}` | — | écriture, non testé |  |
| GET | `/server/{id_server}/hytale/mods` | — | non testé |  |
| GET / PUT / DELETE | `/server/{id_server}/hytale/verification` | — | non testé |  |
| PATCH | `/server/{id_server}/mod` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/mod/{id}` | — | écriture, non testé |  |
| POST | `/server/{id_server}/modrinth` | — | écriture, non testé |  |
| POST | `/server/{id_server}/modrinth/datapack` | — | écriture, non testé |  |
| POST | `/server/{id_server}/modrinth/mod` | — | écriture, non testé |  |
| GET | `/server/{id_server}/mods` | — | ✓ GET |  |
| POST | `/server/{id_server}/mods/updates` | — | écriture, non testé |  |
| GET / POST / PATCH / DELETE | `/server/{id_server}/palworld/import` | — | non testé |  |
| POST / PATCH | `/server/{id_server}/plugin` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/plugin/{id}` | — | écriture, non testé |  |
| GET | `/server/{id_server}/plugins` | — | ✓ GET |  |
| POST | `/server/{id_server}/plugins/updates` | — | écriture, non testé |  |
| GET / PUT | `/server/{id_server}/steamworkshop/autoupdate` | — | non testé |  |
| GET / PUT | `/server/{id_server}/steamworkshop/mods` | — | non testé |  |
| POST | `/server/{id_server}/steamworkshop/mods/sync` | — | écriture, non testé |  |

### Réseau et accès

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| PATCH / DELETE | `/server/{id_server}/address` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/bungeecord` | — | écriture, non testé |  |
| POST / PATCH / DELETE | `/server/{id_server}/livemap` | — | écriture, non testé |  |
| POST | `/server/{id_server}/port` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/port/description` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/port/primary` | — | écriture, non testé |  |
| DELETE | `/server/{id_server}/port/{id}` | — | écriture, non testé |  |
| GET / PATCH | `/server/{id_server}/sftp` | — | ✓ GET | **Seul endpoint qui renvoie le mot de passe SFTP à une clé API** (le bloc `sftp` de `GET /server/{id}` ne l'a plus). `PATCH` : probablement la réinitialisation. |

### Sauvegardes

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| PUT | `/server/{id_server}/backup` | ✓ | écriture, non testé |  |
| GET | `/server/{id_server}/backup/download/{chemin}` | — | non testé |  |
| GET | `/server/{id_server}/backups` | ✓ | ✓ GET |  |
| GET | `/server/{id_server}/snapshots` | — | ✓ GET |  |

### Serveur

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/server/{id_server}` | ✓ | ✓ GET | Le bloc `sftp` n'y contient plus `password` pour une clé API (il le contient pour la session web). |

### Supervision et statistiques

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/server/{id_server}/activity` | — | ✓ GET |  |
| GET | `/server/{id_server}/disk/total` | — | ✓ GET |  |
| GET | `/server/{id_server}/disk/usage` | — | ✓ GET |  |
| GET | `/server/{id_server}/live` | ✓ | ✓ GET |  |
| GET | `/server/{id_server}/live/light` | ✓ | ✓ GET |  |
| GET | `/server/{id_server}/stats/{début}/{fin}` | — | ✓ GET | Historique sur une période : `{début}` et `{fin}` en ISO 8601 (`2026-11-18T15:00:04`). |
| GET | `/server/{id_server}/websocket` | — | ✓ GET |  |
| POST | `/servers/live/light/bulk` | — | ✓ POST | État de plusieurs serveurs en un appel. Corps `{ server_ids: [ids] }`. |

### Tâches planifiées du panel

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| POST | `/server/{id_server}/schedule` | — | écriture, non testé | Création : `{ name, minute, hour, day_of_month, month, day_of_week, is_active, only_when_online }` (et `tasks` facultatif). |
| PATCH / DELETE | `/server/{id_server}/schedule/{id}` | — | écriture, non testé |  |
| POST | `/server/{id_server}/schedule/{id}/task` | — | écriture, non testé | Action : `{ action: command ou power, payload, time_offset (0-900 s), continue_on_failure }`. |
| PATCH / DELETE | `/server/{id_server}/schedule/{id}/task/{id}` | — | écriture, non testé |  |
| PATCH | `/server/{id_server}/schedule/{id}/tasks/order` | — | écriture, non testé | Corps `{ task_ids: [ids dans l'ordre] }`. |
| GET | `/server/{id_server}/schedules` | — | ✓ GET | Tâches planifiées **du panel** (distinctes des automatisations de l'app). Utilisé par l'app : liste, création, modification, actions, ordre (`panel_schedules.rs`). |

### Divers

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| GET | `/server/catalog/eggs` | ✓ | non testé |  |
| GET | `/server/catalog/locations` | ✓ | non testé |  |
| GET | `/server/{id_server}/console/logs` | ✓ | non testé |  |
| PATCH | `/server/{id_server}/icon` | ✓ | écriture, non testé |  |
| PATCH | `/server/{id_server}/name` | ✓ | écriture, non testé |  |
| PATCH | `/server/{id_server}/onboarding` | — | écriture, non testé |  |
| GET / POST / DELETE | `/server/{id_server}/shortcuts` | — | ✓ GET |  |
| POST | `/server/{id_server}/{id}/modpack` | — | écriture, non testé |  |

## Catalogues du site

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| POST | `/site/bug/report` | — | écriture, non testé |  |
| ? | `/site/curseforge/bedrock/addons/search` | — | écriture, non testé |  |
| GET | `/site/curseforge/bedrock/versions` | — | non testé |  |
| GET | `/site/curseforge/datapack/{id}/versions` | — | non testé |  |
| ? | `/site/curseforge/datapacks/search` | — | écriture, non testé |  |
| ? | `/site/curseforge/hytale/mods/search` | — | écriture, non testé |  |
| GET | `/site/curseforge/hytale/{id}/versions` | — | non testé |  |
| ? | `/site/curseforge/mods/search` | — | écriture, non testé |  |
| GET | `/site/curseforge/world/{id}/versions` | — | non testé |  |
| ? | `/site/curseforge/worlds/search` | — | écriture, non testé |  |
| GET | `/site/curseforge/{id}/versions` | — | non testé |  |
| GET | `/site/dashboard_news` | — | ✓ GET |  |
| GET | `/site/databases` | — | ✓ GET |  |
| GET | `/site/datacenters` | — | ✓ GET |  |
| GET | `/site/eggs` | — | ✓ GET |  |
| GET | `/site/eggs/experimentals` | — | ✓ GET |  |
| GET | `/site/feature_flags` | — | ✓ GET |  |
| POST | `/site/giftcode` | — | écriture, non testé |  |
| GET | `/site/index` | — | ✓ GET |  |
| GET | `/site/java/versions` | — | ✓ GET |  |
| GET | `/site/minecraft/versions` | — | ✓ GET |  |
| GET | `/site/modpacks/featured` | — | ✓ GET |  |
| GET | `/site/modpacks/search/{id}` | — | non testé |  |
| GET | `/site/modrinth/datapack/{id}/versions` | — | non testé |  |
| ? | `/site/modrinth/datapacks/search` | — | écriture, non testé |  |
| ? | `/site/modrinth/mods/search` | — | écriture, non testé |  |
| ? | `/site/modrinth/mods/{id}` | — | écriture, non testé |  |
| ? | `/site/modrinth/search` | — | écriture, non testé |  |
| GET | `/site/modrinth/{id}` | — | ✓ GET |  |
| GET | `/site/modrinth/{id}/versions` | — | non testé |  |
| GET | `/site/mybox/options` | — | ✓ GET |  |
| GET | `/site/offers` | — | ✓ GET |  |
| GET | `/site/php/versions` | — | ✓ GET |  |
| GET | `/site/plugin/{id}/versions` | — | non testé |  |
| ? | `/site/plugins/search` | — | écriture, non testé |  |
| ? | `/site/plugins/{id}` | — | écriture, non testé |  |
| ? | `/site/steamworkshop/mods/search` | — | écriture, non testé |  |
| GET | `/site/versions/categories/{id}` | — | non testé |  |
| ? | `/site/versions/fabric/builds/{id}` | — | écriture, non testé |  |
| ? | `/site/versions/forge/builds/{id}` | — | écriture, non testé |  |
| ? | `/site/versions/neoforge/builds/{id}` | — | écriture, non testé |  |
| GET | `/site/versions/search/{id}` | — | non testé |  |
| GET | `/site/{source}/modpacks/files/{id}` | — | non testé |  |

## Hébergement web

| Méthode | Chemin | Doc. | Clé API | Remarque |
|---|---|---|---|---|
| POST | `/webhosting/downgrade` | — | écriture, non testé |  |
| GET / DELETE | `/webhosting/{id_webhosting}` | — | non testé |  |
| POST | `/webhosting/{id_webhosting}/backup` | — | écriture, non testé |  |
| GET | `/webhosting/{id_webhosting}/backups` | — | non testé |  |
| PATCH | `/webhosting/{id_webhosting}/database/password` | — | écriture, non testé |  |
| PATCH | `/webhosting/{id_webhosting}/domain` | — | écriture, non testé |  |
| PATCH | `/webhosting/{id_webhosting}/ftp/password` | — | écriture, non testé |  |
| PATCH | `/webhosting/{id_webhosting}/home` | — | écriture, non testé |  |
| POST | `/webhosting/{id_webhosting}/install` | — | écriture, non testé |  |
| GET | `/webhosting/{id_webhosting}/logs` | — | non testé |  |
| POST | `/webhosting/{id_webhosting}/mail` | — | écriture, non testé |  |
| PATCH | `/webhosting/{id_webhosting}/mail/password` | — | écriture, non testé |  |
| DELETE | `/webhosting/{id_webhosting}/mail/{id}` | — | écriture, non testé |  |
| PATCH | `/webhosting/{id_webhosting}/php` | — | écriture, non testé |  |
| POST | `/webhosting/{id_webhosting}/reset` | — | écriture, non testé |  |
| GET / POST | `/webhosting/{id_webhosting}/ssl` | — | non testé |  |
| GET | `/webhosting/{id_webhosting}/stats` | — | non testé |  |
| POST / PATCH | `/webhosting/{id_webhosting}/subdomain` | — | écriture, non testé |  |
| ? | `/webhosting/{id_webhosting}/upgrade` | — | écriture, non testé |  |

## Structure des réponses (lectures testées)

Noms de champs et types relevés avec une clé API (`str`, `int`, `float`, `bool`, `null`) ; `[]` :
élément d'un tableau, `{n}` : clé numérique (identifiant). Un tableau vide lors du relevé n'a pas
révélé la forme de ses éléments.

<details><summary><code>GET /mybox/{id_mybox}</code></summary>

```
offer.id : int
offer.name : str
offer.price : float
offer.public : int
offer.cpu : int
offer.flexcpu : int
offer.flexcore : int
offer.ram : int
offer.disk : int
offer.disk_total : int
offer.dbsize : int
offer.dbnb : int
offer.pods : int
offer.components.cpu : str
offer.components.ram : str
offer.components.disk : str
databases[] : tableau (vide lors du relevé)
review : int
options[] : tableau (vide lors du relevé)
dedicated_ips[] : tableau (vide lors du relevé)
dedicated_ip_pending_servers[] : tableau (vide lors du relevé)
dedicated_ip.available : int
dedicated_ip.is_free : int
servers[].id : int
servers[].permissions[] : tableau (vide lors du relevé)
servers[].name : str
servers[].hashsupport : str
servers[].egg : int
servers[].egg_icon : str
servers[].egg_name : str
servers[].ip : str
servers[].dedicated_ip : int
servers[].dedicated_ip_pending : int
servers[].cpu : int
servers[].ram : int
servers[].disabled : int
servers[].is_expired : int
servers[].is_suspended : int
servers[].location : str
servers[].country : str
servers[].country_iso : str
servers[].order : int
webhostings[] : tableau (vide lors du relevé)
pimpmybox.eggs[] : tableau (vide lors du relevé)
pimpmybox.history[] : tableau (vide lors du relevé)
subusers[] : tableau (vide lors du relevé)
mybox.id : int
mybox.hashsupport : str
mybox.name : str
mybox.tstart : str
mybox.tend : str
mybox.is_trial : int
mybox.is_free : int
mybox.optimizable : int
mybox.owner : int
mybox.is_expired : int
mybox.is_suspended : int
mybox.is_pro : int
mybox.has_bonus : int
mybox.tend_days : int
mybox.promotion_recurrent : int
mybox.promotion_reduction : int
mybox.subscription : int
mybox.cpu_remaining : int
mybox.ram_remaining : int
extension_preview.available : bool
extension_preview.days : int
extension_preview.days_exact : float
extension_preview.credits_to_consume : int
extension_preview.new_tend : str
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/activity</code></summary>

```
mybox.id : int
mybox.name : str
activity[].id : int
activity[].action_name : str
activity[].action_category : str
activity[].action_result : str
activity[].resource_type : str
activity[].resource_id : int
activity[].actor_type : str
activity[].context.server_name : str
activity[].created_at : str
activity[].actor_name : str
activity[].ip : str
activity[].is_staff : bool
pagination.total : int
pagination.limit : int
pagination.offset : int
pagination.has_more : bool
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/database/disk</code></summary>

```
total_size : int
databases[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/disk</code></summary>

```
disk_servers : int
disk_servers_unit : str
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/players</code></summary>

```
total.current : int
total.max : int
servers[].id : int
servers[].uuid : str
servers[].name : str
servers[].players.current : int
servers[].players.max : int
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/stats/{début}/{fin}</code></summary>

```
stats[].cpu : int
stats[].ram : int
stats[].disk : int
stats[].players : int
stats[].date : str
by_server.{n}.id : int
by_server.{n}.uuid : str
by_server.{n}.name : str
by_server.{n}.actif : int
by_server.{n}.stats[].cpu : int
by_server.{n}.stats[].ram : int
by_server.{n}.stats[].disk : int
by_server.{n}.stats[].players : int
by_server.{n}.stats[].date : str
retention_days : int
data_available_from : str
date_start_clamped : bool
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/subuser/permissions</code></summary>

```
perms.mybox[].name : str
perms.mybox[].category : str
perms.mybox[].description : str
perms.server[].name : str
perms.server[].category : str
perms.server[].description : str
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/subuser/presets</code></summary>

```
presets[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /mybox/{id_mybox}/websocket</code></summary>

```
servers[].id : int
servers[].uuid : str
servers[].name : str
servers[].actif : int
servers[].socket : str
servers[].token : str
```

</details>

<details><summary><code>GET /server/{id_server}</code></summary>

```
websocket.url : str
websocket.token : str
options[] : tableau (vide lors du relevé)
mybox.name : str
mybox.hashsupport : str
mybox.resources.cpu : int
mybox.resources.ram : int
mybox.resources.disk : int
mybox.to_review : int
offer.name : str
offer.is_pro : int
offer.is_trial : int
offer.is_free : int
offer.cpu : int
offer.ram : int
offer.disk : int
egg.id : int
egg.type : int
egg.name : str
egg.icon : str
egg.uuid : str
settings.poweraction_message.restart : str
settings.poweraction_message.stop : str
settings.is_bedrock : int
settings.is_bungeecord : int
settings.java_memory : int
settings.image : str
settings.id_java_version : int
settings.php_runtime : int
settings.startup : str
settings.startup_file : str
history[].date : str
history[].ip : str
history[].is_staff : bool
history[].action : str
ports[].port : int
ports[].ip : str
ports[].description : str
sftp.protocol : str
sftp.host : str
sftp.port : int
sftp.user : str
livemap.option : int
livemap.enabled : int
livemap.is_free : int
server.id : int
server.hashsupport : str
server.id_mybox : int
server.id_daemon : int
server.tstart : str
server.tend : str
server.ip : str
server.port : int
server.dns : str
server.is_free : int
server.hibernation : int
server.backup_group : str
server.name : str
server.uuid : str
server.dedicated_ip : int
server.owner : int
server.permissions[] : tableau (vide lors du relevé)
onboarding.eligible : bool
onboarding.status : str
onboarding.user_disabled : bool
```

</details>

<details><summary><code>GET /server/{id_server}/activity</code></summary>

```
server.id : int
activity[].id : int
activity[].action_name : str
activity[].action_category : str
activity[].action_result : str
activity[].actor_type : str
activity[].context.account : str
activity[].created_at : str
activity[].actor_name : str
activity[].ip : str
activity[].is_staff : bool
pagination.total : int
pagination.limit : int
pagination.offset : int
pagination.has_more : bool
```

</details>

<details><summary><code>GET /server/{id_server}/backups</code></summary>

```
backups[].id : int
backups[].size : int
backups[].date : str
backups[].startup_config : bool
```

</details>

<details><summary><code>GET /server/{id_server}/datapacks</code></summary>

```
level_name : str
datapacks[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /server/{id_server}/disk/total</code></summary>

```
total_bytes : int
total_human : str
raw_bytes : int
raw_human : str
base_game_bytes : int
base_game_human : str
overhead_applied : bool
```

</details>

<details><summary><code>GET /server/{id_server}/disk/usage</code></summary>

```
analyzed_path : str
total_size : int
total_size_human : str
items_count : int
items[].path : str
items[].name : str
items[].type : str
items[].size : int
items[].size_human : str
```

</details>

<details><summary><code>GET /server/{id_server}/docker/images</code></summary>

```
images[].label : str
images[].name : str
images[].selected : int
```

</details>

<details><summary><code>GET /server/{id_server}/files/compress/jobs</code></summary>

```
jobs[] : tableau (vide lors du relevé)
count : int
```

</details>

<details><summary><code>GET /server/{id_server}/files/content/{chemin}</code></summary>

```
content : str
encoding : str
```

</details>

<details><summary><code>POST /server/{id_server}/files/exists</code></summary>

```
exists.{chemin} : bool
```

</details>

<details><summary><code>GET /server/{id_server}/files/list/{chemin}</code></summary>

```
files[].folder : int
files[].size : str
files[].size_bytes : int
files[].name : str
files[].mode : str
files[].symlink : int
files[].created : str
files[].modified : str
```

</details>

<details><summary><code>GET /server/{id_server}/live</code></summary>

```
installed_at : str
state : str
status : null
stats.state : str
stats.cpu.current : float
stats.cpu.dedicated : int
stats.cpu.flexcore : int
stats.cpu.limit : int
stats.cpu.percent : int
stats.cpu.is_bursting : bool
stats.memory.current : int
stats.memory.limit : int
stats.memory.percent : int
stats.disk.current : int
stats.disk.limit : int
stats.disk.percent : int
stats.network.rx_bytes : int
stats.network.tx_bytes : int
stats.uptime.days : int
stats.uptime.hours : int
stats.uptime.minutes : int
stats.uptime.total_seconds : int
stats.players.current : int
stats.players.limit : int
stats.version : str
stats.motd : str
stats.hostname : str
```

</details>

<details><summary><code>GET /server/{id_server}/live/light</code></summary>

```
status : null
cpu.dedicated : int
cpu.flexcore : int
cpu.limit : int
disk.limit : int
memory.limit : int
players.current : int
players.limit : int
version : str
motd : str
hostname : str
```

</details>

<details><summary><code>GET /server/{id_server}/minecraft_version</code></summary>

```
version : str
source : str
confidence : str
```

</details>

<details><summary><code>GET /server/{id_server}/mods</code></summary>

```
mods[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /server/{id_server}/players/offline</code></summary>

```
level_name : str
total : int
total_all : int
limit : int
offset : int
resolved_names : int
players[].uuid : str
players[].name : str
players[].size : int
players[].mtime : int
players[].play_time : int
players[].flags.op : bool
players[].flags.whitelisted : bool
players[].flags.banned : bool
other_playerdata_dirs[] : tableau (vide lors du relevé)
playerdata_dir_found : bool
online.available : bool
online.names[] : tableau (vide lors du relevé)
online.uuids[] : tableau (vide lors du relevé)
online.count : int
```

</details>

<details><summary><code>GET /server/{id_server}/players/{id}/inventory</code></summary>

```
uuid : str
data_version : int
mtime : int
sha256 : str
roundtrip_ok : bool
format.stack : str
format.equipment : str
inventory.0.id : str
inventory.0.count : int
inventory.0.components.minecraft:custom_name : obj
inventory.0.components.minecraft:custom_data : obj
equipment.head.id : str
equipment.head.count : int
online.is_online : bool
online.players_online : int
online.query_available : bool
online.uuids_available : bool
```

</details>

<details><summary><code>GET /server/{id_server}/players/{id}/profile</code></summary>

```
uuid : str
name : str
data_version : int
mtime : int
flags.op : bool
flags.whitelisted : bool
flags.banned : bool
state.game_mode : int
state.health : int
state.max_health : null
state.food_level : int
state.xp_level : int
state.xp_progress : int
state.xp_total : int
stats.play_time : int
stats.sessions : int
stats.deaths : null
stats.mob_kills : null
stats.player_kills : null
stats.distances.walk_one_cm : int
stats.distances.sprint_one_cm : int
stats.distances.fall_one_cm : int
stats.distances.fly_one_cm : int
times.first_played : int
times.last_login : int
times.last_played : int
times.last_seen : int
```

</details>

<details><summary><code>GET /server/{id_server}/plugins</code></summary>

```
plugins[].name : str
plugins[].filename : str
plugins[].version : str
plugins[].enabled : bool
plugins[].sha1 : str
plugins[].source : str
plugins[].project_id : str
```

</details>

<details><summary><code>GET /server/{id_server}/properties</code></summary>

```
properties.{propriété} : str
```

</details>

<details><summary><code>GET /server/{id_server}/schedules</code></summary>

```
schedules[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /server/{id_server}/sftp</code></summary>

```
sftp.protocol : str
sftp.host : str
sftp.port : int
sftp.user : str
sftp.password : str
```

</details>

<details><summary><code>GET /server/{id_server}/shortcuts</code></summary>

```
shortcuts[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /server/{id_server}/snapshots</code></summary>

```
server_id : int
snapshots[].id : int
snapshots[].server_id : int
snapshots[].name : str
snapshots[].type : str
snapshots[].size : int
snapshots[].size_bytes : int
snapshots[].date : str
snapshots[].status : int
snapshots[].is_legacy : bool
quota_used : int
quota_max : int
permissions.create : bool
permissions.restore : bool
permissions.download : bool
permissions.delete : bool
```

</details>

<details><summary><code>GET /server/{id_server}/startup/files</code></summary>

```
files[].name : str
files[].selected : int
startup_file : str
```

</details>

<details><summary><code>GET /server/{id_server}/stats/{début}/{fin}</code></summary>

```
stats[].cpu : int
stats[].ram : int
stats[].disk : int
stats[].players : int
stats[].date : str
```

</details>

<details><summary><code>GET /server/{id_server}/websocket</code></summary>

```
websocket.url : str
websocket.token : str
```

</details>

<details><summary><code>GET /server/{id_server}/worlds</code></summary>

```
active_level_name : str
worlds[].name : str
worlds[].is_active : bool
worlds[].size_bytes : int
worlds_base : str
```

</details>

<details><summary><code>POST /servers/live/light/bulk</code></summary>

```
servers.{n}.players.current : int
servers.{n}.players.limit : int
statuses.{n} : null
```

</details>

<details><summary><code>GET /site/dashboard_news</code></summary>

```
cards[].id : int
cards[].position : int
cards[].image_dark : str
cards[].image_light : str
cards[].lang.fr.title : str
cards[].lang.fr.link : str
cards[].lang.en.title : str
cards[].lang.en.link : str
```

</details>

<details><summary><code>GET /site/databases</code></summary>

```
databases[].id : int
databases[].name : str
```

</details>

<details><summary><code>GET /site/datacenters</code></summary>

```
datacenters[].id : int
datacenters[].location : str
datacenters[].country : str
datacenters[].country_iso : str
datacenters[].in_stock : bool
datacenters[].geo_masked : bool
datacenters[].ping : str
```

</details>

<details><summary><code>GET /site/eggs</code></summary>

```
eggs[].id : int
eggs[].name : str
eggs[].icon : str
eggs[].resources.cpu : str
eggs[].resources.ram : str
eggs[].resources.disk : str
eggs[].resources.disk_install : str
eggs[].certification : int
eggs[].is_official : int
eggs[].required_fields[] : tableau (vide lors du relevé)
eggs_categories[].category : str
eggs_categories[].eggs[].id : int
eggs_categories[].eggs[].name : str
eggs_categories[].eggs[].icon : str
eggs_categories[].eggs[].resources.cpu : str
eggs_categories[].eggs[].resources.ram : str
eggs_categories[].eggs[].resources.disk : str
eggs_categories[].eggs[].resources.disk_install : str
eggs_categories[].eggs[].certification : int
eggs_categories[].eggs[].is_official : int
eggs_categories[].eggs[].required_fields[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /site/eggs/experimentals</code></summary>

```
eggs.APPLICATION[].id : int
eggs.APPLICATION[].name : str
eggs.APPLICATION[].fields[] : tableau (vide lors du relevé)
eggs.VIDEO_GAME[].id : int
eggs.VIDEO_GAME[].name : str
eggs.VIDEO_GAME[].fields[].name : str
eggs.VIDEO_GAME[].fields[].description : str
eggs.GENERIC[].id : int
eggs.GENERIC[].name : str
eggs.GENERIC[].fields[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /site/feature_flags</code></summary>

```
highlight_enabled_site : bool
highlight_enabled_panel : bool
ai_onboarding_enabled : bool
posthog_enabled_site : bool
posthog_enabled_panel : bool
palworld_import_enabled_panel : bool
```

</details>

<details><summary><code>GET /site/index</code></summary>

```
offers.mybox[].id : int
offers.mybox[].name : str
offers.mybox[].name_min : str
offers.mybox[].name_com : str
offers.mybox[].descr : str
offers.mybox[].price : float
offers.mybox[].cpu : int
offers.mybox[].flexcore : int
offers.mybox[].ram : int
offers.mybox[].disk : int
offers.mybox[].dbnb : int
offers.mybox[].dbsize : int
offers.mybox[].pods : int
offers.mybox[].home : int
offers.mybox[].ai_credits_value_eur : float
offers.mybox[].reviews_score : float
offers.mybox[].reviews_count : int
offers.mybox[].reviews_percentage : float
offers.myboxperf[].id : int
offers.myboxperf[].name : str
offers.myboxperf[].name_min : str
offers.myboxperf[].name_com : str
offers.myboxperf[].descr : str
offers.myboxperf[].price : float
offers.myboxperf[].cpu : int
offers.myboxperf[].flexcore : int
offers.myboxperf[].ram : int
offers.myboxperf[].disk : int
offers.myboxperf[].dbnb : int
offers.myboxperf[].dbsize : int
offers.myboxperf[].pods : int
offers.myboxperf[].home : int
offers.myboxperf[].ai_credits_value_eur : float
offers.myboxperf[].reviews_score : float
offers.myboxperf[].reviews_count : int
offers.myboxperf[].reviews_percentage : float
sales : int
eggs[].name : str
eggs[].games[].id : int
eggs[].games[].name : str
eggs[].games[].icon : str
eggs[].games[].url : str
eggs[].games[].certification : int
eggs[].games[].experimental : int
promotion.status : str
promotion.expire : str
promotion.code : str
wheel.start : str
wheel.end : str
wheel.token_amount : int
wheel.double : int
discord.online : int
discord.total : int
reviews.count : int
reviews.score : float
reviews.percentage : float
reviews.list[].name : str
reviews.list[].offer : str
reviews.list[].score : int
reviews.list[].message : str
reviews.list[].date : str
count_mybox_tested : str
test_mybox_available : int
test_myboxperf_available : int
footer_news.content : str
footer_news.url : str
footer_news.date : str
global_message.fr : str
global_message.en : str
global_message_link : null
global_message_icon : str
global_message_color_bg : str
global_message_color_text : str
banner : null
promo_popup : null
shop_popup : null
```

</details>

<details><summary><code>GET /site/java/versions</code></summary>

```
versions[].id : int
versions[].name : str
```

</details>

<details><summary><code>GET /site/minecraft/versions</code></summary>

```
versions[] : str
```

</details>

<details><summary><code>GET /site/modpacks/featured</code></summary>

```
modpacks[].id : int
modpacks[].name : str
modpacks[].url : str
modpacks[].dateLatest : str
modpacks[].icon : str
modpacks[].gameVersions : str
modpacks[].downloadCount : str
modpacks[].isCustom : bool
modpacks[].source : str
```

</details>

<details><summary><code>GET /site/modrinth/{id}</code></summary>

```
plugins[].id : str
plugins[].slug : str
plugins[].name : str
plugins[].tag : str
plugins[].downloads : int
plugins[].icon.url : str
plugins[].rating.average : int
plugins[].testedVersions[] : str
plugins[].premium : bool
plugins[].version.id : null
plugins[].categories[] : str
plugins[].source : str
total_hits : int
cached : bool
```

</details>

<details><summary><code>GET /site/mybox/options</code></summary>

```
options[].id : int
options[].identifier : str
options[].price : str
```

</details>

<details><summary><code>GET /site/offers</code></summary>

```
offers.mybox[].id : int
offers.mybox[].name : str
offers.mybox[].name_min : str
offers.mybox[].name_com : str
offers.mybox[].price : float
offers.mybox[].cpu : int
offers.mybox[].flexcore : int
offers.mybox[].ram : int
offers.mybox[].disk : int
offers.mybox[].dbnb : int
offers.mybox[].dbsize : int
offers.mybox[].pods : int
offers.mybox[].components.cpu : str
offers.mybox[].components.ram : str
offers.mybox[].components.disk : str
offers.myboxperf[].id : int
offers.myboxperf[].name : str
offers.myboxperf[].name_min : str
offers.myboxperf[].name_com : str
offers.myboxperf[].price : float
offers.myboxperf[].cpu : int
offers.myboxperf[].flexcore : int
offers.myboxperf[].ram : int
offers.myboxperf[].disk : int
offers.myboxperf[].dbnb : int
offers.myboxperf[].dbsize : int
offers.myboxperf[].pods : int
offers.myboxperf[].components.cpu : str
offers.myboxperf[].components.ram : str
offers.myboxperf[].components.disk : str
offers.webhosting[].id : int
offers.webhosting[].name : str
offers.webhosting[].name_min : str
offers.webhosting[].price : str
offers.webhosting[].disk : int
offers.webhosting[].subdomains : int
offers.webhosting[].mails : int
offers.webhosting[].mails_disk : int
```

</details>

<details><summary><code>GET /site/php/versions</code></summary>

```
versions[].id : int
versions[].name : str
versions[].default : bool
```

</details>

<details><summary><code>GET /user</code></summary>

```
user.datas.id : int
user.datas.pseudo : str
user.datas.mail : str
user.datas.partner : int
user.datas.money : int
user.datas.2fa : int
user.datas.blackmod : int
user.datas.lang : str
user.datas.settings : null
user.datas.register_date : str
user.datas.promotional : int
user.datas.mail_validated : int
user.datas.wheel_fortune_tokens : int
user.datas.active_mybox_count : int
user.datas.panel_mybox_count : int
user.api_authkey : str
user.ip.actual.ip : str
user.ip.actual.date : str
user.ip.last.ip : str
user.ip.last.date : str
user.ip.register.ip : str
user.ip.register.date : str
user.2fa.enabled : int
user.2fa.src : null
user.2fa.secret : null
user.payment.total : float
user.notifications[] : tableau (vide lors du relevé)
user.alerts[] : tableau (vide lors du relevé)
user.support_alert : int
user.partnership_alert : int
user.partner_platform_url : null
user.next_daily_drop : str
user.linked_account.minecraft.uuid : str
user.linked_account.minecraft.name : str
user.linked_account.discord.id : int
user.linked_account.discord.name : str
user.physical_prices[] : tableau (vide lors du relevé)
user.physical_daily_prices[] : tableau (vide lors du relevé)
user.global_message : str
user.global_message_link : str
user.global_message_icon : str
user.global_message_color_bg : str
user.global_message_color_text : str
user.global_alert.icon : str
user.global_alert.color_bg : str
user.global_alert.color_text : str
user.global_alert.message : str
user.global_alert.display_site : bool
user.global_alert.display_panel : bool
user.global_alert.cta : null
user.event_alert : null
user.panel_nav_link : null
```

</details>

<details><summary><code>GET /user/{id_user}/activity</code></summary>

```
user.id : int
activity[].id : int
activity[].action_name : str
activity[].action_category : str
activity[].action_result : str
activity[].resource_type : str
activity[].resource_id : int
activity[].actor_type : str
activity[].context : null
activity[].created_at : str
activity[].actor_name : str
activity[].ip : str
activity[].is_staff : bool
pagination.total : int
pagination.limit : int
pagination.offset : int
pagination.has_more : bool
```

</details>

<details><summary><code>GET /user/{id_user}/affiliate</code></summary>

```
affiliate.pending_withdrawal : null
affiliate.withdrawals_history[].id : int
affiliate.withdrawals_history[].amount : float
affiliate.withdrawals_history[].type : str
affiliate.withdrawals_history[].date_processed : str
affiliate.withdrawals_history[].status : str
affiliate.withdrawals_history[].reject_reason : null
affiliate.code : str
affiliate.discount_for_users : int
affiliate.percentage : int
affiliate.clicks : int
affiliate.count : int
affiliate.new_users : int
affiliate.count_for_upgrade : int
affiliate.pool.remaining : int
affiliate.pool.total : float
affiliate.history[].amount : str
affiliate.history[].percentage : int
affiliate.history[].name : null
affiliate.history[].date : str
affiliate.history[].is_refunded : bool
affiliate.history[].auto_renew : bool
platform.managed : bool
platform.url : str
```

</details>

<details><summary><code>GET /user/{id_user}/archives</code></summary>

```
archives.server[].id : int
archives.server[].name : str
archives.server[].size : int
archives.server[].date : str
archives.server[].status : int
archives.server[].is_legacy : bool
archives.server[].type : str
archives.database[].id : int
archives.database[].name : str
archives.database[].size : int
archives.database[].date : str
archives.database[].status : int
archives.database[].is_legacy : bool
```

</details>

<details><summary><code>GET /user/{id_user}/ark/clusters</code></summary>

```
clusters[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/backup_jobs/active</code></summary>

```
jobs[] : tableau (vide lors du relevé)
count : int
```

</details>

<details><summary><code>GET /user/{id_user}/credits</code></summary>

```
credits[].type : str
credits[].amount : str
credits[].date : str
```

</details>

<details><summary><code>GET /user/{id_user}/dailydrop</code></summary>

```
check_daily_drop : bool
first : bool
day_counter : int
tokens : int
last_daily_drop : str
next_daily_drop : str
next_daily_drop_seconds : int
date_begin_streak : str
streak_remaining_time : int
history[].price : str
history[].physical : int
history[].from : str
history[].date : str
```

</details>

<details><summary><code>GET /user/{id_user}/gifts</code></summary>

```
gifts[] : tableau (vide lors du relevé)
history[].id : int
history[].price : str
history[].physical : int
history[].from : str
history[].date : str
```

</details>

<details><summary><code>GET /user/{id_user}/goodies/transactions</code></summary>

```
transactions[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/hosting-program</code></summary>

```
hosting_program : null
```

</details>

<details><summary><code>GET /user/{id_user}/ideas</code></summary>

```
ideas[].id : int
ideas[].description : str
ideas[].images[] : tableau (vide lors du relevé)
ideas[].reason : str
ideas[].votes_up : int
ideas[].votes_down : int
ideas[].date : str
ideas[].status : int
```

</details>

<details><summary><code>GET /user/{id_user}/migrations</code></summary>

```
migrations[].id : int
migrations[].server_start_id : int
migrations[].server_end_id : int
migrations[].is_external : int
migrations[].external_host : str
migrations[].external_port : int
migrations[].external_username : str
migrations[].external_password : str
migrations[].database_host : str
migrations[].database_port : int
migrations[].database_username : str
migrations[].database_password : str
migrations[].server_end_hashsupport : str
migrations[].server_end_name : str
migrations[].date : str
migrations[].status : int
```

</details>

<details><summary><code>GET /user/{id_user}/notifications</code></summary>

```
notifications[].id : int
notifications[].type : str
notifications[].color : str
notifications[].icon : str
notifications[].message : str
notifications[].date : str
notifications[].viewed : int
notifications[].readed : int
```

</details>

<details><summary><code>GET /user/{id_user}/notifications/unreaded</code></summary>

```
notifications[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/partnership</code></summary>

```
partnership.id : int
partnership.status : int
partnership.date : str
partnership.presentation : str
partnership.receive : str
partnership.give : str
partnership.summary.partner : str
partnership.summary.mystrator : str
partnership.messages[].message : str
partnership.messages[].date : str
partnership.messages[].is_staff : int
partnership.messages[].staff_name : str
partnership.messages[].staff_job : str
platform.managed : bool
platform.url : str
```

</details>

<details><summary><code>GET /user/{id_user}/pimpmybox</code></summary>

```
eggs.validated[].id : int
eggs.validated[].name : str
eggs.validated[].required_fields[] : tableau (vide lors du relevé)
eggs.history[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/preferences/dashboard_groups</code></summary>

```
· : null
```

</details>

<details><summary><code>GET /user/{id_user}/preferences/sidebar</code></summary>

```
· : null
```

</details>

<details><summary><code>GET /user/{id_user}/servers</code></summary>

```
servers_groups.{n}.id : int
servers_groups.{n}.owner : int
servers_groups.{n}.name : str
servers_groups.{n}.hashsupport : str
servers_groups.{n}.order : int
servers_groups.{n}.mybox : int
servers_groups.{n}.tend : str
servers_groups.{n}.note : str
servers_groups.{n}.tend_days : int
servers_groups.{n}.is_trial : int
servers_groups.{n}.is_free : int
servers_groups.{n}.has_bonus : int
servers_groups.{n}.is_pro : int
servers_groups.{n}.is_expired : int
servers_groups.{n}.is_suspended : int
servers_groups.{n}.promo_recurrent : null
servers_groups.{n}.promo_reduction : null
servers_groups.{n}.subscription : int
servers_groups.{n}.servers[] : tableau (vide lors du relevé)
servers_groups.{n}.offer : str
servers_groups.{n}.resources.cpu : int
servers_groups.{n}.resources.ram : int
servers_groups.{n}.resources.disk : int
servers_groups.{n}.webhosting[] : tableau (vide lors du relevé)
user_groups[] : tableau (vide lors du relevé)
servers[].id : int
servers[].hashsupport : str
servers[].id_mybox : int
servers[].id_daemon : int
servers[].tstart : str
servers[].tend : str
servers[].ip : str
servers[].port : int
servers[].dns : str
servers[].is_free : int
servers[].hibernation : int
servers[].note : null
servers[].id_egg : int
servers[].is_disabled : int
servers[].name : str
servers[].owner : int
servers[].is_bedrock : int
servers[].egg_uuid : str
servers[].egg_name : str
servers[].egg_icon : str
servers[].is_expired : int
servers[].is_suspended : int
servers[].reduction : int
servers[].last_backup : str
servers[].poweraction_message_restart : str
servers[].poweraction_message_stop : str
servers[].id_group : int
```

</details>

<details><summary><code>GET /user/{id_user}/servers/websocket</code></summary>

```
servers[].id : int
servers[].uuid : str
servers[].name : str
servers[].mybox_id : int
servers[].socket : str
servers[].token : str
count : int
```

</details>

<details><summary><code>GET /user/{id_user}/snapshots</code></summary>

```
snapshots[].id : int
snapshots[].server_id : int
snapshots[].name : str
snapshots[].type : str
snapshots[].size : int
snapshots[].size_bytes : int
snapshots[].date : str
snapshots[].status : int
snapshots[].is_legacy : bool
```

</details>

<details><summary><code>GET /user/{id_user}/subscriptions</code></summary>

```
actives_subscriptions[].cycle : int
actives_subscriptions[].date_started : str
actives_subscriptions[].id_payment_method : int
actives_subscriptions[].payment_method_type : str
actives_subscriptions[].payment_method_descr : str
actives_subscriptions[].payment_failed : bool
actives_subscriptions[].try : int
actives_subscriptions[].discount_version : int
actives_subscriptions[].id_mybox : int
actives_subscriptions[].name : str
actives_subscriptions[].hashsupport : str
actives_subscriptions[].mybox_active : bool
actives_subscriptions[].offer : str
actives_subscriptions[].price : float
inactives_subscriptions[].cycle : int
inactives_subscriptions[].date_started : str
inactives_subscriptions[].id_payment_method : int
inactives_subscriptions[].payment_method_type : str
inactives_subscriptions[].payment_method_descr : str
inactives_subscriptions[].payment_failed : bool
inactives_subscriptions[].try : int
inactives_subscriptions[].discount_version : int
inactives_subscriptions[].date_ended : str
inactives_subscriptions[].id_mybox : int
inactives_subscriptions[].mybox_active : bool
inactives_subscriptions[].offer : str
mybox_without_subscription[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/tasks</code></summary>

```
tasks.global[] : tableau (vide lors du relevé)
tasks.nodes[] : tableau (vide lors du relevé)
tasks.targeted[] : tableau (vide lors du relevé)
```

</details>

<details><summary><code>GET /user/{id_user}/tickets</code></summary>

```
tickets[].id : int
tickets[].title : str
tickets[].message : str
tickets[].date : str
tickets[].type : int
tickets[].id_server : int
tickets[].origin : null
tickets[].origin_ref : null
tickets[].status : int
```

</details>
