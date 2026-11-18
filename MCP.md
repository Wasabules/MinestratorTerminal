# Serveur MCP MineStrator — scope & capacités

`minestrator-mcp` expose la gestion de tes serveurs Minecraft MineStrator via le
**Model Context Protocol (MCP)** — un protocole **ouvert et standard**. Il est donc utilisable
par **n'importe quel client MCP** (Claude Desktop, Claude Code, Cline, Continue, Zed, ou un
client maison), pas seulement par Claude.

Il est bâti sur `minestrator-core` : la même logique métier que l'app desktop et qu'un futur daemon.

---

## 1. Vue d'ensemble

- **Rôle** : donner à un assistant/agent la capacité de **gérer, diagnostiquer, suivre et optimiser**
  des serveurs Minecraft, en langage naturel.
- **Standard** : JSON-RPC 2.0, spec MCP `2024-11-05`.
- **Sans configuration de clé** : réutilise la clé API stockée dans le trousseau de l'OS par l'app
  desktop. (Connecte-toi une fois dans l'app.)
- **Fonctionne app fermée** : parle directement à l'API MineStrator + SFTP.

## 2. Transport & protocole

| | |
|---|---|
| Transport | **stdio** (JSON-RPC délimité par sauts de ligne). stdout = protocole, stderr = logs. |
| Version MCP | `2024-11-05` |
| Capacité annoncée | `tools` (list + call) |
| Handshake | `initialize` → `tools/list` → `tools/call` |

> Le serveur ne publie pas encore de `resources` ni de `prompts` (voir §8, extensions prévues).

## 3. Authentification & données

- **Auth** : clé API lue dans le trousseau OS (`service=MinestratorTerminal`). Aucune clé en clair.
  Avec plusieurs comptes enregistrés dans l'app, c'est toujours la clé du **compte actif**
  (entrée `api-key`) : le MCP agit sur ce compte-là. Un processus MCP déjà lancé garde en mémoire
  la clé lue à son démarrage — après un changement de compte dans l'app, relancer le client MCP
  (les agents CLI du Copilote, eux, sont relancés automatiquement).
- **Historique** : partage la base SQLite de métriques de l'app (mode WAL) → lecture concurrente sûre.
- **Anonymisation** : les sorties potentiellement sensibles (`read_file`, `read_console`,
  `analyze_performance`, `inspect_region`…) passent par le filtre de rédaction avant de partir vers
  l'agent (mots de passe d'auth, IPv4, e-mails, secrets `clé=valeur`, identifiants d'URL).

## 4. Deux façons de l'héberger (même logique `core::mcp`)

1. **App desktop en mode serveur** — `minestrator-terminal --mcp` : l'application *est* le serveur MCP
   (elle n'ouvre pas la fenêtre). App centralisante. Réglable depuis Réglages → MCP.
2. **Binaire autonome** — `minestrator-mcp` : léger, headless (idéal serveur/daemon).

## 5. Capacités — catalogue d'outils (59)

`✎` = action **modifiante** (soumise au réglage « autoriser les actions modifiantes » ; refusée en
mode lecture seule). Les entrées suffixées `?` sont **optionnelles**.

> **Source unique de vérité** : ce catalogue est dérivé de `mcp::tool_list()` (+ des constantes
> `READ_TOOLS` / `WRITE_TOOLS`) dans `crates/minestrator-core/src/mcp.rs`. **35 outils de lecture**
> + **24 modifiants** (dont 7 d'infrastructure MyBox, `INFRA_TOOLS`). Un test vérifie que chaque
> outil listé est classé lecture **ou** écriture, et que chaque outil classé a son schéma.

#### Découverte & état

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_servers` | | — | Serveurs (id, nom, statut, adresse, MyBox). **Point d'entrée** pour les `server_id`. |
| `server_status` | | `server_id` | État temps réel (`GET /server/{id}/live`) : consommation instantanée — CPU en centièmes de cœur dont **FlexCore** (`is_bursting`), RAM/disque en Mo —, réseau cumulé, uptime, joueurs, version, MOTD. Repli sur un échantillon WebSocket si l'endpoint échoue. |
| `list_players` | | `server_id`, `sort?`, `active_days?`, `search?`, `limit?` | Joueurs Minecraft Java **déjà venus** (`GET /server/{id}/players/offline`, non documenté) : statistiques (joueurs uniques, connectés, actifs 24 h / 7 j / 30 j, temps de jeu total) puis liste triée (`play_time` par défaut, `last_seen`, `name`) avec temps de jeu **cumulé** en heures, dernière venue, op/whitelist/banni. `active_days=7` + `sort=play_time` répond à « qui a le plus joué cette semaine » (le temps reste cumulé : l'API n'a pas de détail par période). Pseudos remplacés par un alias stable (`joueur-xxxxxx`) si l'anonymisation IA est active ; UUID jamais envoyés. |
| `list_schedules` | | `server_id` | Tâches planifiées **du panel MineStrator** (`GET /server/{id}/schedules`, non documenté) : nom, cron, active, seulement si en ligne, prochaine/dernière exécution, actions ordonnées (commande ou alimentation, délai). Exécutées par l'hébergeur, même PC éteint — distinctes des automatisations de l'app. Commandes anonymisées. |
| `mybox_info` | | `mybox_id` | Détail d'une MyBox (`GET /mybox/{id}`) : échéance, **renouvellement automatique**, jours avant suppression si expirée, CPU/RAM encore allouables, offre (bases de données, IP dédiées), serveurs et ressources allouées. |
| `server_metrics` | | `server_id`, `since_secs?` | **Historique croisé** CPU (% d'un cœur) / RAM / disque (octets) sur la fenêtre (défaut 1 h, max 30 j) : surveillance de l'app (`source: local`, 10 s) complétée par les statistiques à la minute de l'hébergeur (`source: host`, `GET /server/{id}/stats/…`, non documenté) là où l'app ne surveillait pas. **Résumé** (moyennes pondérées par durée, max, pic et moyenne de joueurs), `coverage` (`local_s`, `host_s`, `host_available`), 120 échantillons au plus (maximum par tranche), `players` (joueurs connectés, hébergeur). |

#### Architecture des MyBox (« architecte »)

Répartir des serveurs entre ses MyBox selon les ressources libres. Unités rappelées dans chaque
sortie : **CPU en cœurs entiers** (1 = un cœur, pas un %), **RAM en Go entiers** (pas en Mo) ;
le disque est **partagé** par toute la MyBox ; seuls les serveurs **actifs** consomment CPU et RAM.
Analyse complète des endpoints : [`docs/api/MYBOX.md`](./docs/api/MYBOX.md).

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `mybox_capacity` | | `mybox_id?` | **Point d'entrée** : pour chaque MyBox, CPU/RAM totaux, alloués, restants ; disque (Go, occupé en Mo) ; emplacements actifs/en pause ; FlexCore ; état ; serveurs (cœurs, Go, pause, localisation, ordre) ; `blockers` (ce qui empêche d'y créer un serveur). |
| `server_catalog` | | `query?`, `include_experimental?` | Jeux (`egg_id`, catégorie, ressources **recommandées**, champs requis) et localisations (`location_id`, ville, en stock). |
| `plan_placement` | | `servers[{label?, cpu, ram, disk_gb?}]`, `strategy?` | **Calcul pur, ne modifie rien** : MyBox proposée pour chaque serveur (`pack` = au plus juste, `spread` = répartir), ce qu'il y restera, raisons d'un refus, pistes (serveurs à mettre en pause). |
| `mybox_smartscan` | | `mybox_id` | Analyse SmartScan de l'hébergeur (usage p95) : serveurs surdimensionnés et allocation suggérée, rééquilibrage, conseil d'évolution. |
| `mybox_create_server` | ✎ | `mybox_id`, `name`, `egg_id`, `experimental?`, `location_id?`, `cpu`, `ram`, `required_fields?` | Crée un serveur après validation stricte **avant envoi** (ressources restantes, emplacement actif libre, jeu du catalogue, champs requis, localisation en stock). |
| `mybox_resize_server` | ✎ | `mybox_id`, `server_id`, `cpu`, `ram` | Valeurs **finales** de CPU/RAM ; le serveur rend d'abord ce qu'il occupe. L'API peut exiger le serveur arrêté. |
| `mybox_pause_server` | ✎ | `mybox_id`, `server_id`, `paused` | Pause (ressources rendues, fichiers gardés) ou reprise (emplacement et ressources requis). |
| `mybox_reorder_servers` | ✎ | `mybox_id`, `server_ids` | Ordre des serveurs dans la MyBox. |
| `rename_server` | ✎ | `server_id`, `name` | Renomme un serveur (1 à 50 caractères). |
| `rename_mybox` | ✎ | `mybox_id`, `name` | Renomme une MyBox (1 à 50 caractères). |
| `mybox_delete_server` | ✎ | `mybox_id`, `server_id`, `confirm_name`, `snapshot_first?` | **Suppression définitive.** `confirm_name` doit reprendre **exactement** le nom du serveur ; par défaut une snapshot est créée et attendue d'abord (suppression annulée si elle échoue ou dépasse 15 min). |

Ces 7 outils modifiants forment `INFRA_TOOLS` : disponibles pour un client MCP externe (si
`allow_writes`), **jamais** pour le Copilote, qui lit du contenu non fiable (chat des joueurs) —
ni dans son catalogue, ni dans sa liste blanche d'agent CLI, ni via « Appliquer ».

#### Contrôle & console

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `power_action` | ✎ | `server_id`, `action` | start / restart / restart10 / stop / stop10 / kill. |
| `send_command` | ✎ | `server_id`, `command` | Envoie une commande console (serveur en ligne). |
| `read_console` | | `server_id` | 100 dernières lignes de console (diagnostic de crash/erreur). |
| `player_action` | ✎ | `server_id`, `action`, `player` | Modération : kick / ban / unban / op_add / op_remove / whitelist_add / whitelist_remove. |

#### Fichiers & configuration (SFTP)

> Le mot de passe SFTP vient de l'API (`GET /server/{id}/sftp`). S'il manque ou ne fonctionne pas,
> l'utilisateur peut en saisir un dans l'onglet Fichiers de l'app (rangé dans le trousseau, partagé
> avec le MCP) ; sinon ces outils renvoient une erreur qui le dit.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_files` | | `server_id`, `path` | Contenu d'un répertoire (nom, chemin, dossier/fichier, taille). |
| `read_file` | | `server_id`, `path` | Contenu d'un fichier texte (refuse binaires / > 2 Mo). |
| `read_gz` | | `server_id`, `path` | Contenu d'un fichier texte **gzippé**, décompressé (ex. log tourné `latest.log.gz`) — pour diagnostiquer sur des logs archivés. |
| `list_archive` | | `server_id`, `path` | Entrées d'une archive `.zip`/`.tar`/`.tar.gz` **sans l'extraire** (repérer un fichier dans un backup/modpack). |
| `read_archive_entry` | | `server_id`, `path`, `entry` | Contenu **texte** d'une entrée d'archive, sans extraction disque. |
| `search_files` | | `server_id`, `query`, `root?` | **Recherche récursive** de fichiers/dossiers par sous-chaîne de nom (bornée) — localiser une config sans le chemin exact. |
| `write_file` | ✎ | `server_id`, `path`, `content` | Écrit/écrase un fichier de config. |
| `create_dir` | ✎ | `server_id`, `path` | Crée un dossier. |
| `delete_path` | ✎ | `server_id`, `path`, `is_dir?` | Supprime un fichier ou un dossier. |
| `rename_path` | ✎ | `server_id`, `from`, `to` | Renomme ou déplace un fichier/dossier. |
| `extract_archive` | ✎ | `server_id`, `path`, `dest_dir` | **Extrait** une archive présente sur le serveur, **par le serveur** (rien ne transite par la machine ; `.zip`, `.jar`, `.tar`, `.tar.gz/.bz2/.xz/.zst`, `.7z`, `.rar`, `.mcworld`…). Repli automatique par SFTP pour `.zip`/`.tar`/`.tar.gz` si l'API échoue (anti zip-slip, plafonné). Écrase les fichiers du même nom. |
| `compress_files` | ✎ | `server_id`, `paths[]`, `name?` | **Compresse en `.zip` sur le serveur** des fichiers/dossiers d'un même dossier (l'archive y est créée) : sauvegarde ponctuelle d'un monde avant une intervention, rapide même pour plusieurs Go. Suit le job jusqu'au bout (~30 min max) ; renvoie chemin, taille, nombre de fichiers, fichiers ignorés. N'écrase jamais une archive existante. |
| `disk_usage` | | `server_id`, `path?`, `limit?` | **Espace disque** calculé par le serveur : plus grosses entrées d'un dossier (tailles récursives, part en %), et à la racine l'occupation comptée face à la limite du serveur et au disque partagé de la MyBox. Pour un serveur plein : descendre dans les plus gros dossiers. |
| `upload_file` | ✎ | `server_id`, `local_path`, `remote_path` | **Téléverse** un fichier **local** (BINAIRE, sans la limite 2 Mo de `write_file`) — déployer un `.jar`, un resource pack, un monde. `local_path` = machine hôte du MCP. Crée le parent, écrase la cible. |
| `edit_file` | ✎ | `server_id`, `path`, `find`, `replace` | Édition **partielle** (remplace toutes les occurrences de `find`). Ne renvoie **jamais** le contenu → sûr même si le fichier contient un secret (contrairement à `read_file`+`write_file` qui réécrirait `[SECRET]`). Erreur si `find` absent. |
| `download_file` | | `server_id`, `remote_path`, `local_path` | **Télécharge** un fichier distant (BINAIRE) vers un chemin local (comparer/backup ciblé avant de pousser). `local_path` = machine hôte du MCP. |
| `file_stat` | | `server_id`, `path` | **Métadonnées** d'un fichier distant : taille + **sha256** — vérifier qu'un `upload_file` a réussi / correspond au local. |

#### Démarrage & JVM

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `read_startup` | | `server_id` | Commande de démarrage (flags JVM `-Xmx`/GC/Aikar…), JAR, mémoire, image. |
| `set_startup_params` | ✎ | `server_id`, `parameters` | Modifie la commande Java (optimiser les flags JVM). Garder `{{SERVER_JARFILE}}` ; effet au prochain démarrage. |
| `read_properties` | | `server_id` | Réglages `server.properties` via l'API (`GET /server/{id}/properties`), **sans SFTP**. Valeurs secrètes (`rcon.password`…) masquées en `[SECRET]`. Minecraft. |
| `set_property` | ✎ | `server_id`, `property`, `value` | Modifie **un** réglage existant (`PATCH`) ; effet au prochain démarrage. Refuse de réécrire le masque `[SECRET]`. Minecraft. |

#### Mods & plugins (marketplace + installés)

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_installed_mods` | | `server_id`, `query?` | Mods installés (nom + version, compact ; `query` filtre par sous-chaîne). |
| `list_installed_plugins` | | `server_id`, `query?` | Plugins installés (nom + version, compact ; `query` filtre par sous-chaîne). |
| `market_search` | | `kind?`, `source?`, `query?`, `loader?`, `game_version?`, `page?` | Recherche Modrinth / CurseForge / SpigotMC (id/slug, nom, downloads, loaders, versions de jeu). |
| `list_mod_versions` | | `source?`, `slug`, `loader?`, `game_version?` | Versions d'un projet → les `version_id` requis pour installer. |
| `install_mod` | ✎ | `server_id`, `source?`, `kind?`, `slug`, `version_id`, `loader?` | Installe un mod/plugin. **Modrinth** = mods **et** plugins (`{slug, version_id}`) ; **Spigot** = plugins (`slug` = id numérique, `version_id` = id de version). Refuse le premium/external (403). |
| `check_updates` | | `server_id`, `kind?`, `force?` | **Mises à jour** des mods, plugins et datapacks installés : version installée → nouvelle, d'après la vérification de l'hébergeur (`POST /server/{id}/{type}/updates`, non documenté, pure comparaison) tenant compte de la version de Minecraft et du loader. Passe par un **garde-fou** : les propositions qui visent une autre plateforme (variante Velocity d'un plugin Paper…) ou une version identique/plus ancienne sont **écartées** et listées à part. Donne la **clé** de chaque élément. Résultat gardé 15 min (`force` revérifie). |
| `update_mods` | ✎ | `server_id`, `kind?`, `keys?` | Applique des mises à jour : revérifie, installe la version retenue (mêmes endpoints que le panel) puis supprime l'ancien fichier si son nom change. `keys` = clés de `check_updates` ; vide = toutes celles des éléments **actifs** (filtrées par `kind`). Jamais une proposition écartée. Bilan par élément (mis à jour, ancien fichier conservé, échec, non appliqué) ; **effet au prochain redémarrage**. |

#### Mods Satisfactory (ficsit.app) — serveurs Satisfactory uniquement

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `ficsit_search` | | `query?`, `order_by?`, `page?` | Catalogue **ficsit.app** (SMR) : id, `mod_reference`, nom, downloads. |
| `ficsit_versions` | | `mod_id` | Versions d'un mod (par `mod_id` ficsit) : `id` de version + contrainte SML + dépendances. |
| `install_ficsit_mod` | ✎ | `server_id`, `reference`, `version_id` | Installe le mod + **SML** + dépendances (cible LinuxServer) via SFTP, avec **arrêt puis redémarrage** du serveur. |

#### Sauvegardes (filet avant intervention)

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_backups` | | `server_id` | Backups quotidiens **automatiques** (id, taille, date ; **restaurables**, pas de création à la demande). |
| `list_snapshots` | | — | Snapshots de l'utilisateur : points de sauvegarde **créés à la demande** (id, nom, taille, date, statut). |
| `create_snapshot` | ✎ | `server_id`, `name` | Crée un snapshot — **additif, sans risque** : le filet à poser AVANT une intervention risquée. |

#### Diagnostic avancé & optimisation

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `analyze_performance` | | `server_id`, `with_profiler?` | Analyse **Spark** (`health`/`tps`/`gc` + profiler CPU 30 s optionnel) → rapport **parsé** : TPS, MSPT, GC, points chauds par méthode. ⚠️ le profiler ajoute ~30 s + un léger à-coup. |
| `parse_spark_report` | | `url` | Télécharge et **parse** un rapport Spark existant (`spark.lucko.me/<clé>`) : profiler (CPU) ou heapsummary (mémoire). |
| `diagnose_startup` | | `server_id` | **Docteur démarrage** : en un appel, rassemble commande de démarrage + fin de `logs/latest.log` + dernier crash-report, et **pré-scanne** les pannes connues (EULA, port occupé, OOM, version Java, dépendance/mixin de mod, monde corrompu). |
| `inspect_region` | | `server_id`, `path` | Inspecte un fichier de région `.mca` et repère les chunks **structurellement corrompus** (pointeurs/longueurs invalides). Lecture seule. |

#### Journaux et historique

Ce que seul un outil local sait faire : fouiller les journaux archivés par SFTP et relire
l'historique que l'app consigne (disponibilité, incidents, diagnostics). Absents du MCP officiel :
gardés quand il prend la gestion (`LOCAL_KEEP_TOOLS`). Universels (tous les jeux).

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `search_logs` | | `server_id`, `query`, `regex?`, `case_sensitive?`, `context?`, `since?`, `until?`, `order?`, `dir?`, `include_debug?`, `max_results?`, `max_files?` | Recherche dans `logs/latest.log` **et** les journaux archivés (`AAAA-MM-JJ-N.log.gz`, décompressés à la volée) : texte ou expression régulière (temps linéaire), casse ignorée par défaut, 0 à 5 lignes de contexte. Période `since`/`until` (`AAAA-MM-JJ`, `AAAA-MM-JJ HH:MM`, relatif `12h`/`7d`) en **heure du serveur** : jour déduit du nom du fichier et de l'horodatage `[HH:MM:SS]` (passage de minuit suivi ligne à ligne ; `latest.log` daté en remontant depuis sa dernière écriture ; formats Forge, Unreal et ISO datés directement), décalage UTC du serveur déduit de la fin de `latest.log`. Résultats **groupés par fichier**, numéros de ligne façon `grep -n`, `order=oldest` pour une première apparition. `dir` : jeux dont les journaux sont ailleurs (défaut par jeu : `/logs`, Satisfactory `/FactoryGame/Saved/Logs`, Factorio `/`). **Plafonds** signalés dans la sortie : 20 fichiers (max 60), 32 Mio téléchargés, `.gz` de plus de 8 Mio ignoré, journal en clair lu sur ses 8 derniers Mio, décompression bornée à 48 Mio, 50 résultats (max 200), sortie de 64 Kio. |
| `error_digest` | | `server_id`, `source?`, `since_hours?`, `include_warnings?`, `limit?`, `dir?`, `max_files?` | Erreurs et avertissements des journaux (fenêtre par défaut 24 h, max 720 h) ou de la console récente, **regroupés par signature** : classe d'exception de la cause première + deux premières frames utiles (hors JDK, serveur, loader, bibliothèques), sans nombres, adresses, lambdas ni horodatages ; sans exception, message normalisé (pseudos, UUID, nombres retirés). Par groupe : niveau (`FATAL`, `SEVERE`, `ERROR`, `WARN`), nombre, première et dernière occurrence, **plugin ou mod probable** (`evidence` : `event` « Could not pass event … to X », `enable` « Error occurred while enabling X », `load`, `task`, `mod` (module Forge, « from mod x », `ModID:`), `jar` (jar des frames), `prefix` `[X]`, `logger`, `package`), exemple tronqué. Trié par gravité puis fréquence. Formats Paper, Spigot (frames préfixées), Fabric, Forge/NeoForge, Unreal. Mêmes plafonds de lecture (8 fichiers par défaut, max 30). |
| `uptime_report` | | `server_id` | Disponibilité sur **24 h / 7 j / 30 j** (% du temps surveillé, temps surveillé, indisponibilité, part déduite de l'hébergeur `host_share_pct`) et coupures (début, fin, durée, cause `crash`/`stop`/`unknown`, fin connue ou non) : le calcul de la carte Disponibilité de l'app, sans recalcul. `warning` explicite si aucune surveillance locale n'existe (l'hébergeur seul ne prouve jamais une coupure). |
| `list_incidents` | | `server_id?`, `limit?`, `since_days?`, `kind?` | Incidents du plus récent au plus ancien (défaut 20, max 100, 30 jours, max 32) : crash, surcharge (`cpu`/`ram`/`disk`), échéance de MyBox, redémarrage automatique, tâche planifiée, chacun avec les **diagnostics du Copilote** rattachés (résumé, cause, correctif, actions proposées) ; diagnostics sans alerte (ligne ERROR/WARN de console, surcharge prolongée) listés seuls. Sources : journal local de l'app (table `incident_log`), alertes du service d'arrière-plan quand il supervise le compte (sans doublon), crashs du suivi de disponibilité non couverts par une alerte. `coverage` dit ce qui a été consulté. |

**Données locales et MCP headless.** `minestrator-mcp` et `--mcp` ouvrent le même dossier de
données que l'app (même `metrics.db`, mode WAL) : ils relisent son suivi de disponibilité et son
journal des incidents, mais **ne surveillent rien eux-mêmes** (pas de superviseur, pas de
Copilote). Le journal des incidents est tenu par l'app de bureau quand elle tourne ; le service
d'arrière-plan, lui, garde ses observations dans son propre dossier machine : hors de l'app, le
serveur MCP lui demande (au plus une fois par minute, sans autre effet) s'il supervise le compte,
et inclut alors ses coupures et ses alertes par IPC. Sans aucune donnée, `list_incidents` répond
par un message explicite et `uptime_report` par un `warning` — jamais une liste vide ou un 100 %
présentés comme un bilan. Sorties anonymisées comme `read_console`/`read_file` (`redact`) ; pseudos
des journaux remplacés par l'alias de `list_players` quand l'anonymisation IA est active.

### Couverture par domaine

- **Découverte & état** : `list_servers`, `server_status`, `mybox_info`, `list_schedules`
- **Architecture des MyBox** : `mybox_capacity`, `server_catalog`, `plan_placement` (calcul pur),
  `mybox_smartscan` ; création, redimensionnement, pause, ordre, renommage et suppression gardée
- **Consommation & optimisation** : `server_metrics` (historique + résumé), `server_status` (live),
  `analyze_performance` (Spark : TPS/MSPT/GC/points chauds), `parse_spark_report`
- **Contrôle** : `power_action`, `send_command`
- **Modération & joueurs** : `player_action`, `list_players` (joueurs déjà venus, temps de jeu, activité)
- **Fichiers / config** : `list_files`, `read_file`, `write_file`, `edit_file` (patch find/replace,
  sûr avec secrets), `create_dir`, `delete_path`, `rename_path` (lister, lire, écrire, éditer, créer
  un dossier, supprimer, renommer/déplacer)
- **Transferts binaires** : `upload_file` (déployer un `.jar`/resource pack local → serveur, sans la
  limite 2 Mo), `download_file` (tirer un fichier distant en local), `file_stat` (taille + sha256 pour
  vérifier l'intégrité d'un upload)
- **Logs & archives** : `read_gz` (logs gzippés tournés), `list_archive` + `read_archive_entry`
  (fouiller un `.zip`/`.tar.gz` sans extraire) — utile pour diagnostiquer sur des logs/backups archivés
- **Opérations faites par le serveur** (API REST du panel, le SFTP restant le transport) :
  `compress_files` (archive `.zip` créée sur place), `extract_archive` (extraction sur place, repli
  SFTP), `disk_usage` (ce qui occupe le disque, limite du serveur et de la MyBox)
- **Démarrage / JVM** : `read_startup`, `set_startup_params` (mémoire, GC, Aikar…)
- **Réglages du serveur** : `read_properties`, `set_property` (`server.properties` via l'API, sans SFTP)
- **Mods & plugins** : `market_search`, `list_mod_versions`, `install_mod`, `list_installed_mods`,
  `list_installed_plugins` (chercher → choisir une version → installer ; inventaire),
  `check_updates` → `update_mods` (mises à jour des mods, plugins et datapacks, puis redémarrage)
- **Mods Satisfactory** : `ficsit_search`, `ficsit_versions`, `install_ficsit_mod` (ficsit.app / SMR)
- **Sauvegardes / filet** : `list_backups`, `list_snapshots`, `create_snapshot`
- **Diagnostic de panne** : `diagnose_startup` (crash-loop), `inspect_region` (maps corrompues),
  `read_console` (+ `server_metrics` pour corréler)
- **Journaux et historique** : `error_digest` (ce qui plante, par signature et plugin probable),
  `search_logs` (journaux archivés compris), `uptime_report` (disponibilité, coupures),
  `list_incidents` (crashs, surcharges et diagnostics du Copilote)

## 6. Contrôle & sécurité (réglages)

Deux réglages, modifiables depuis le GUI (Réglages → MCP) et persistés (`mcp.json`), lus par le serveur :

- **`enabled`** — si désactivé : `tools/list` renvoie une liste vide et tout appel est refusé.
- **`allow_writes`** — si désactivé (**mode lecture seule**) : les outils marqués `✎`
  (`power_action`, `send_command`, `player_action`, `write_file`, `upload_file`, `edit_file`,
  `create_dir`, `delete_path`, `rename_path`, `set_startup_params`, `set_property`, `install_mod`,
  `install_ficsit_mod`, `extract_archive`, `create_snapshot`, et l'infrastructure MyBox :
  `mybox_create_server`, `mybox_resize_server`, `mybox_pause_server`, `mybox_reorder_servers`,
  `rename_server`, `rename_mybox`, `mybox_delete_server`) sont refusés ; les outils de
  lecture/diagnostic restent disponibles (dont `plan_placement`, qui ne modifie rien). Idéal pour
  donner un accès *observation* sans risque.

**Garde-fous by design :**

- **Default-deny** : tout outil qui n'est **pas** dans `READ_TOOLS` est traité comme modifiant. Un
  nouvel outil est donc gaté par défaut (impossible de l'exposer en écriture par oubli).
- **Liste blanche d'env** (`MINESTRATOR_MCP_ALLOWED_TOOLS`) : le lanceur peut restreindre le sous-ensemble
  d'outils exposés (utilisée quand le Copilote lance un agent CLI en mode lecture).
- **Actions destructives volontairement absentes** du catalogue : `restore_snapshot`,
  `delete_snapshot`, `restore_backup`, `repair_region` ne sont **pas** des outils MCP → un agent ne
  peut jamais les déclencher. Elles n'existent que dans l'app, derrière un « Appliquer » (intention
  explicite de l'utilisateur). Seul `create_snapshot` (additif) est exposé.
- **Suppression d'un serveur gardée** : `mybox_delete_server` exige le nom exact du serveur
  (`confirm_name`, vérifié avant tout appel) et fait par défaut une snapshot préalable. Les
  écritures MyBox sont validées localement avant envoi (ressources, emplacements, champs requis) :
  une saisie incohérente est refusée sans toucher à l'API.

> **Portée de ce document** : il décrit le serveur MCP **local** exposé aux clients externes (Claude
> Desktop, Claude Code…). L'usage inverse — le Copilote interne comme *client* du **MCP officiel
> MineStrator** (délégation de la gestion, ne gardant en local que `LOCAL_KEEP_TOOLS`) — est décrit
> dans [`COPILOT.md`](./COPILOT.md).

## 7. Se connecter (depuis n'importe quel client MCP)

Le serveur se lance en tant que processus stdio :

```
<chemin>/minestrator-terminal --mcp        # app centralisante
# ou
<chemin>/minestrator-mcp                    # binaire autonome
```

La plupart des clients (Claude Desktop, Claude Code, Cline, Continue…) utilisent le format
`mcpServers` :

```json
{
  "mcpServers": {
    "minestrator": {
      "command": "C:\\...\\minestrator-terminal.exe",
      "args": ["--mcp"]
    }
  }
}
```

- **Claude Desktop** : Réglages → Développeur → éditer `claude_desktop_config.json`.
- **Claude Code** : `claude mcp add minestrator -- "<chemin>\minestrator-terminal.exe" --mcp`.
- **Autre client** : renseigne la **commande** + **args** (`--mcp`) ci-dessus.

Le GUI (Réglages → MCP) génère ce bloc avec le bon chemin, bouton **Copier**.

Build du binaire autonome : `cargo build -p minestrator-mcp --release`
→ `target/release/minestrator-mcp`.

## 8. Limites connues (héritées de l'API publique)

- Les **snapshots/backups** se **listent** (`list_snapshots`, `list_backups`) et un snapshot se
  **crée** (`create_snapshot`), mais la **restauration** n'est pas exposée à l'agent (voir §6).
- Pas d'endpoint **documenté** de listing des bans/ops — `list_players` donne les drapeaux
  op/whitelist/banni de chaque joueur déjà venu (endpoint non documenté), et `read_file` lit
  `banned-players.json` / `ops.json` ; la modération (`player_action`) reste disponible.
- Le temps de jeu de `list_players` est **cumulé** depuis la première venue : l'API n'en donne pas
  le détail par période.
- Stats **live** : `GET /server/{id}/live` (un appel) ; la connexion WebSocket éphémère ne sert plus que de
  repli. Les blocs de consommation sont absents d'un serveur éteint.
- L'historique fin (10 s, état Wings) de `server_metrics` n'existe que si l'app (superviseur) ou son
  service a tourné ; ailleurs, seules les mesures à la minute de l'hébergeur (zéros exclus :
  serveur éteint et minute non mesurée sont indiscernables) sont disponibles. Le binaire MCP
  headless étant relancé à chaque message, il relit ces mesures à chaque appel (cache du `Core`).
- Le marketplace ignore les projets **premium/external** (non installables → `install_mod` renvoie 403).
- La vérification des mises à jour de l'hébergeur ne reçoit **aucun loader pour les plugins** : elle
  propose parfois la variante d'une autre plateforme à version égale (relevé du 2026-11-18 :
  `LuckPerms-Bukkit-5.5.71.jar` → `LuckPerms-Velocity-5.5.71.jar` sur un serveur Paper). `check_updates`
  les écarte (plateforme nommée dans le fichier, le loader du mod ou la plateforme du serveur ; numéro
  de version identique ou plus ancien) ; un élément au numéro illisible garde la proposition de
  l'hébergeur. Les projets non identifiés par l'hébergeur (plugins maison) ne sont pas suivis.

## 9. Extensions prévues (scope futur)

- **`resources`** : exposer logs et listes de fichiers comme ressources MCP
  (lecture contextuelle sans appel d'outil).
- **`prompts`** : modèles prêts (« diagnostiquer un crash », « optimiser la RAM »).
- **Automatisation** : planificateur (tâches programmées), règles (event→action).
- **Sauvegardes** : exposer une restauration *gardée* (confirmation forte) si l'API le permet.

> Déjà livré depuis la première version de ce document : inventaire et **installation** de mods/plugins
> (Modrinth/CurseForge/SpigotMC), **snapshots** (liste + création), analyse **Spark**, **Docteur
> démarrage** et inspection des **régions corrompues** — tous adossés à `minestrator-core`.

---

*Implémentation : logique dans `crates/minestrator-core/src/mcp.rs` (transport-agnostique) ;
transports = binaire `minestrator-mcp` (stdio) et mode `--mcp` de l'app desktop.*
