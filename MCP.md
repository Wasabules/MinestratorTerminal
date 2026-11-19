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
- **Standard** : JSON-RPC 2.0, spec MCP `2024-11-05` à `2026-07-28` (voir §2).
- **Sans configuration de clé** : réutilise la clé API stockée dans le trousseau de l'OS par l'app
  desktop. (Connecte-toi une fois dans l'app.)
- **Fonctionne app fermée** : parle directement à l'API MineStrator + SFTP.

## 2. Transport & protocole

Code : `mcp_transport.rs` (transport), `mcp_ctx.rs` (contexte d'appel des outils),
`mcp_catalog.rs` (groupes, annotations, instructions), `mcp_prompts.rs`, `mcp_resources.rs` ; la
logique des outils reste dans `mcp.rs` (`handle_message`).

| | |
|---|---|
| Transport | **stdio** (JSON-RPC, un message par ligne). stdout = protocole, stderr = journaux. |
| Versions gérées | `2026-07-28` (sans état) ; `2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05` (poignée de main) |
| Capacités annoncées | `tools`, `prompts`, `resources` (sans abonnement : `subscribe: false`) ; aucune notification de changement de liste, ni journalisation, ni complétion |
| Méthodes | `initialize`, `server/discover`, `tools/list`, `tools/call`, `prompts/list`, `prompts/get`, `resources/list`, `resources/templates/list`, `resources/read`, `ping` ; notifications `notifications/cancelled`, `notifications/progress` (sortante) |

**Deux ères dans le même processus** (serveur « dual-era » de la spécification) :

- **Poignée de main** (`2024-11-05` à `2025-11-25`) : `initialize` négocie la version — celle
  demandée si nous la gérons, sinon `2025-11-25` — et fixe pour la connexion l'identité du client
  (`clientInfo.name`) et ses capacités. `2024-11-05` reste gérée pour les agents CLI actuels.
- **Sans état** (`2026-07-28`) : chaque requête porte `io.modelcontextprotocol/protocolVersion`,
  `…/clientCapabilities` (et `…/clientInfo`) dans `_meta`. `server/discover` décrit le serveur
  (versions, capacités, instructions). Chaque résultat porte `resultType` et
  `_meta["io.modelcontextprotocol/serverInfo"]` ; les listes et `resources/read` portent
  `ttlMs` et `cacheScope: "private"` (données du compte). Version inconnue : erreur `-32022`
  avec la liste des versions gérées ; `_meta` incomplet : `-32602`. Un client « dual-era » qui
  sonde par `server/discover` reste donc en sans état ; la variable `MINESTRATOR_MCP_STATELESS=0`
  coupe cette ère (`server/discover` → méthode inconnue, repli du client sur `initialize`). Un
  serveur lancé par le Copilote (liste blanche d'env) garde la poignée de main, comme avant.

**Concurrence.** Chaque requête est traitée dans sa propre tâche : un outil long (profiler Spark,
compression, snapshot avant suppression) ne bloque plus les autres appels. Un seul rédacteur écrit
sur stdout (canal) ; un message illisible est ignoré (journal) ; une panique dans un outil devient
une erreur `-32603` sans arrêter le serveur. En fin d'entrée (stdin fermé), les requêtes en cours
sont menées à leur terme et leurs réponses envoyées, puis le processus se termine.

**Annulation** (`notifications/cancelled`) : `mcp_ctx::cancelled()` passe à vrai pour l'appel
visé et **aucune réponse** n'est envoyée. Une lecture est abandonnée net ; une action modifiante
n'est jamais coupée en plein milieu (elle consulte `cancelled()` entre deux étapes). Une question
posée à l'humain pour cet appel est retirée côté client.

**Progression** : si la requête porte `_meta.progressToken`, les outils longs émettent
`notifications/progress` (valeurs croissantes, envois espacés d'au moins 250 ms ; `message`
depuis `2025-03-26`) — compression, extraction et copie par archive y relaient leur avancement.
Sans progression depuis 15 s, un signe de vie (« toujours en cours (N s) ») garde le client à
l'écoute, sans inventer d'avancement.

**Élicitation** (confirmation par l'humain) : si le client déclare la capacité `elicitation`
(formulaire ; `2025-06-18` et après), une action destructive demande à **l'utilisateur** de
retaper le nom exact de sa cible, au lieu de faire confiance au `confirm_name` du modèle :
`mybox_delete_server` et les outils destructifs relayés par la passerelle (§6). Le formulaire n'a
qu'un champ texte (jamais pré-rempli) ; seule une acceptation avec le nom exact (casse comprise)
confirme. Refus, annulation, erreur du client, connexion fermée ou délai de **5 minutes** dépassé
= refus, rien n'est fait. Avec la poignée de main, c'est une requête `elicitation/create` ; sans
état, l'appel répond `resultType: "input_required"` (`inputRequests`) et le client le rejoue avec
`inputResponses`. Le schéma de ces outils annonce alors `confirm_name` comme facultatif. Client
sans élicitation : comportement inchangé (`confirm_name` obligatoire et vérifié).

**Annotations** (`tools/list`) : chaque outil local porte `title`, `readOnlyHint` (= liste
`READ_TOOLS`), `openWorldHint` (marketplaces, ficsit.app, Spark) et, s'il est modifiant,
`destructiveHint` (vrai sauf pour les ajouts purs : `create_dir`, `create_snapshot`,
`compress_files`, `install_mod`, création de serveur ou de tâche planifiée, renommages, ordre) et
`idempotentHint` (valeurs finales : `write_file`, `set_property`, redimensionnement…). Indications
pour le client, jamais une garde : la sécurité reste `READ_TOOLS` / `allow_writes`. Les outils
officiels relayés gardent leurs propres annotations.

**Instructions** (`initialize`, `server/discover`) : un court texte en français donne les
enchaînements recommandés — `list_servers` d'abord, `error_digest` avant `search_logs`,
`diagnose_startup` pour un serveur qui ne démarre pas, `create_snapshot` avant une intervention
risquée, vérifier après chaque action (`wait_for`, `tail_console`, `run_command`), `set_config`
avec `dry_run` et `plan_manifest` avant `apply_manifest`, `edit_file` plutôt que
`read_file` + `write_file` pour un fichier qui contient des secrets.

**Prompts** (`prompts/list`, `prompts/get`) : scénarios guidés, en français, qui enchaînent les
outils. Arguments vérifiés (identifiants entiers, texte court sur une ligne).

| Prompt | Arguments | Rôle |
|---|---|---|
| `diagnose_crash` | `server_id` | Trouver la cause d'un crash ou d'un démarrage impossible, corriger, vérifier. |
| `prepare_public_launch` | `server_id` | Liste de contrôle avant l'ouverture aux joueurs (sécurité, stabilité, capacité, filets). |
| `setup_velocity_network` | `proxy_server_id`, `backend_server_ids` | Proxy Velocity + serveurs Paper : `network_topology`, puis `link_proxy` en dry_run et application après accord (transfert moderne ; secret réutilisé ou généré par l'outil, jamais lu ni écrit par l'agent). |
| `deploy_plugin` | `server_id`, `local_path`, `plugin_name?` | Déployer un plugin en développement, redémarrer, vérifier, revenir en arrière. |
| `optimize_performance` | `server_id` | Mesurer, proposer, appliquer un réglage à la fois et mesurer à nouveau. |
| `security_audit` | `server_id` | Accès, opérateurs, plugins, journaux, actions récentes des agents. |

**Ressources** (`resources/list`, `resources/templates/list`, `resources/read`) : chaque
ressource est un outil de lecture, appelé par le même chemin que `tools/call` — mêmes gardes
(activation, liste blanche d'env, portée du Copilote, groupes), même anonymisation, même cache.
Pas d'abonnement.

| URI | Outil | Type |
|---|---|---|
| `minestrator://servers` | `list_servers` | `application/json` |
| `minestrator://server/{server_id}/status` | `server_status` | `application/json` |
| `minestrator://server/{server_id}/console` | `read_console` (anonymisée) | `text/plain` |
| `minestrator://server/{server_id}/properties` | `read_properties` (secrets masqués) | `application/json` |
| `minestrator://server/{server_id}/file/{+path}` | `read_file` (2 Mo au plus, secrets masqués) | `text/plain` |

Ressource inconnue : erreur `-32002` (`-32602` en sans état).

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

## 5. Capacités — catalogue d'outils (94)

`✎` = action **modifiante** (soumise au réglage « autoriser les actions modifiantes » ; refusée en
mode lecture seule). Les entrées suffixées `?` sont **optionnelles**.

> **Source unique de vérité** : ce catalogue est dérivé de `mcp::tool_list()` (+ des constantes
> `READ_TOOLS` / `WRITE_TOOLS`) dans `crates/minestrator-core/src/mcp.rs`. **52 outils de lecture**
> + **42 modifiants** (dont 23 jamais donnés au Copilote, `INFRA_TOOLS` : infrastructure MyBox, tâches planifiées, plusieurs serveurs, commande de démarrage, manifeste appliqué, clone, annulation, prégénération, base de données, réseau proxy). Les outils qui touchent au disque de l'ordinateur (`download_file`, `upload_file`, `deploy_and_verify` : `LOCAL_DISK_TOOLS`) ne sont jamais donnés au Copilote non plus, même en lecture. Des tests vérifient que chaque
> outil listé est classé lecture **ou** écriture, que chaque outil classé a son schéma, appartient à
> un seul groupe (`TOOL_GROUPS`, `mcp_catalog.rs`) et, s'il est modifiant, a une règle d'annulation
> dans la garde des agents.

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

#### Tâches planifiées du panel (exécutées par l'hébergeur)

Lecture avec `list_schedules` (ci-dessus). Les écritures passent par les endpoints du panel
(`panel_schedules.rs`) et sont **vérifiées avant envoi** : cron en 5 champs aux bornes valides,
actions `command` (une seule ligne, 500 caractères au plus) ou `power`
(`start`/`restart`/`stop`/`kill`), délai `time_offset` de 0 à 900 s. Configuration **persistante**
(rejouée chaque jour, même app fermée) : ces outils rejoignent `INFRA_TOOLS` et ne sont **jamais**
donnés au Copilote.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `create_schedule` | ✎ | `server_id`, `name`, `cron`, `is_active?`, `only_when_online?`, `tasks?[{action, payload, time_offset?, continue_on_failure?}]` | Crée la tâche planifiée puis ses actions dans l'ordre (toutes vérifiées **avant** la création ; id retrouvé par le nom, l'API ne le renvoyant pas). `cron` = « minute heure jour-du-mois mois jour-de-semaine », ex. `0 4 * * *`. |
| `update_schedule` | ✎ | `server_id`, `schedule_id`, `name?`, `cron?`, `is_active?`, `only_when_online?` | Champs omis = valeur actuelle. `is_active=false` suspend sans supprimer. |
| `delete_schedule` | ✎ | `server_id`, `schedule_id`, `confirm_name` | Supprime la tâche et ses actions ; `confirm_name` = nom **exact** de la tâche (vérifié, jamais révélé dans l'erreur). |
| `add_schedule_task` | ✎ | `server_id`, `schedule_id`, `action`, `payload`, `time_offset?`, `continue_on_failure?` | Ajoute une action **à la fin**. |
| `update_schedule_task` | ✎ | `server_id`, `schedule_id`, `task_id`, `action?`, `payload?`, `time_offset?`, `continue_on_failure?` | Modifie une action ; champs omis = valeur actuelle. |
| `delete_schedule_task` | ✎ | `server_id`, `schedule_id`, `task_id` | Retire une action. |
| `reorder_schedule_tasks` | ✎ | `server_id`, `schedule_id`, `task_ids` | Nouvel ordre ; chaque action citée **exactement une fois**. |

#### Plusieurs serveurs

Opérations qui portent sur plusieurs serveurs du compte (`core/src/fleet.rs`), absentes du MCP
officiel (qui ne sait copier un fichier que d'un serveur vers un webhosting).

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `copy_between_servers` | ✎ | `source_server_id`, `source_paths[]`, `target_server_id`, `target_dir`, `overwrite?`, `strategy?` | **Copie** des fichiers ou dossiers d'un serveur vers un autre (ou un autre dossier du même serveur), récursive et **en flux** (rien d'entier en mémoire) ; chaque source arrive dans `target_dir` sous son nom. **N'écrase jamais** un fichier existant sans `overwrite=true` (ignoré et listé ; un remplacement passe par un fichier temporaire voisin, l'ancien reste intact jusqu'au bout) ; ne remplace jamais un dossier par un fichier ni l'inverse ; refuse la racine `/` comme source, la copie d'un chemin sur lui-même ou dans lui-même, deux sources de même nom. `strategy` : `files` (fichier par fichier, 6 en parallèle, **5 000 fichiers / 2 Gio** au plus), `archive` (compression par le serveur source, transfert de l'archive, **8 Gio** au plus, extraction par le serveur cible dans un dossier de transit puis mise en place ; archives temporaires et dossier de transit **supprimés des deux côtés, même en cas d'échec** ; sources d'un même dossier, destination existante seulement avec `overwrite=true`), `auto` (défaut : archive au-delà de 300 fichiers ou 200 Mio quand c'est possible, repli fichier par fichier si la compression échoue). Place disque de la cible vérifiée avant. Rapport JSON : fichiers et octets copiés, remplacés, dossiers créés, ignorés et pourquoi, durée, nettoyage incomplet éventuel. |
| `diff_files` | | `server_id`, `path`, `other_server_id?`, `other_path?`, `recursive?`, `context_lines?` | **Compare** un fichier ou un dossier entre deux serveurs, ou deux chemins d'un même serveur (A = référence). Fichiers texte : **diff unifié** avec contexte (3 lignes par défaut, 400 lignes de sortie au plus ; fins de ligne CRLF/LF ignorées). Binaires ou de plus de 1 Mio : signalés sans diff. Dossiers : ajoutés, absents, modifiés (taille, puis même taille mais date différente), type différent ; premier niveau ou récursif (5 000 entrées par côté). Chaque ligne passe par `redact.rs` **avant** son préfixe de diff : les secrets sortent en `[SECRET]`. |
| `find_player` | | `query`, `exact?`, `server_ids?`, `limit?` | **Retrouve un joueur** (pseudo exact, début de pseudo, UUID ou alias `joueur-xxxxxx`) sur **tous** les serveurs Minecraft Java du compte : connectés en un appel (`live_players_bulk`), joueurs déjà venus par serveur (cache d'une minute, 4 serveurs en parallèle). Par joueur et par serveur : en ligne ou non (`null` si le serveur ne sait pas le dire), dernière venue, temps de jeu, op / whitelist / banni ; serveurs non fouillés listés avec la raison. Pseudos remplacés par un alias stable si l'anonymisation IA est active (comme `list_players`) ; UUID jamais renvoyés. |
| `broadcast_command` | ✎ | `command`, `server_ids?`, `all_running?`, `only_running?` | Envoie la **même commande** console à plusieurs serveurs par l'endpoint REST (`PUT /server/{id}/command`), résultat serveur par serveur. Cibles : une liste (20 au plus) **ou** `all_running=true` — « tous » n'est jamais déduit d'une liste vide. Par défaut seulement les serveurs démarrés (`online`) ; un état illisible n'est pas un feu vert. Commande non vide, sur une seule ligne, 1 000 caractères au plus. |

Les deux outils modifiants sont soumis à `allow_writes` et rangés dans `INFRA_TOOLS` : **jamais**
donnés au Copilote (il lit du contenu non fiable et n'agit que sur un serveur), ni via
« Appliquer ». Les deux lectures font partie des outils exclusifs gardés face au MCP officiel
(`LOCAL_KEEP_TOOLS`) ; données au Copilote, elles restent bornées au serveur de la session
(`find_player` n'y fouille que lui, `diff_files` ne compare que ses propres chemins).

#### Contenu, monde et audit

Absents du MCP officiel : gardés par le Copilote quand l'officiel gère le reste (`LOCAL_KEEP_TOOLS`),
réservés à Minecraft côté Copilote. Lecture seule ; constats triés **critique / important / info**,
chacun avec sa recommandation ; sorties anonymisées comme les autres lectures.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `check_plugin_deps` | | `server_id`, `details?` | **Dépendances** des plugins et mods (Minecraft Java) d'après le descripteur de chaque `.jar` de `plugins/` et `mods/` (`plugin.yml`, `paper-plugin.yml`, `bungee.yml`, `velocity-plugin.json`, `fabric.mod.json`, `quilt.mod.json`, `META-INF/mods.toml`, `META-INF/neoforge.mods.toml`, `mcmod.info`) : dépendance obligatoire manquante ou désactivée, optionnelle absente (info), version de Minecraft, d'`api-version` ou de dépendance incompatible avec le serveur (`GET /server/{id}/minecraft_version`), doublons, jar d'une autre plateforme (plugin Velocity sur Paper, mod Forge sur Fabric…) ou du mauvais dossier, mod côté client, Folia, fichiers `.disabled`, jars illisibles. Plateforme de l'hébergeur, ou **déduite du contenu** (`explicit_jar`, dit dans la sortie). Lecture **à la demande** : fin d'archive, répertoire central et descripteurs, jamais le jar entier ; plafonds affichés (400 jars, répertoire central 4 Mio, descripteur 512 Kio, 192 Mio lus au total). `details` ajoute l'inventaire (id, version, plateforme, dépendances). |
| `world_info` | | `server_id`, `world?` | **Monde** (Minecraft Java) lu dans `level.dat` et, depuis 26.1, dans `data/minecraft/*.dat` : nom, **graine** (`WorldGenSettings.seed` ou `RandomSeed`), version (nom + DataVersion), mode, difficulté, hardcore, apparition, jour et heure, dernière partie, météo, bordure, règles de jeu, datapacks activés/désactivés, **taille de chaque dimension** (fichiers de région de l'overworld, du Nether et de l'End par SFTP, dossier de la dimension et total du monde par l'endpoint d'espace disque) et autres dimensions. Monde actif (`level-name`) par défaut. |
| `audit_server` | | `server_id`, `force_updates?` | **Audit de configuration et de sécurité** : `online-mode=false` (sauf derrière Velocity avec secret ou BungeeCord avec BungeeGuard, d'après `spigot.yml`/`paper-global.yml` ; atténué par un plugin d'authentification), RCON et force de son mot de passe (**qualifiée** vide/faible/correct, jamais affichée), liste blanche, `enable-query`, distances de vue/simulation face à la RAM, nombre d'opérateurs (`ops.json`), `-Xmx` face à la RAM du conteneur, mises à jour disponibles (vérification sans effet de bord, propositions d'une autre plateforme écartées), Java face à Minecraft. Contrôles selon l'édition (Bedrock : authentification et liste blanche) ; autre jeu : « non géré ». Liste aussi les contrôles sans remarque et ceux non vérifiés. |

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

#### Agir puis vérifier

Ce qui manquait à un agent pour ne plus agir à l'aveugle (`core/src/agent_loop.rs`) : lire la
console **par curseur** au lieu de relire les 100 dernières lignes, obtenir la **réponse** d'une
commande, **attendre** un état, une ligne ou une tâche, et **déployer puis vérifier** un plugin en
développement. Absents du MCP officiel : gardés quand il prend la gestion (`LOCAL_KEEP_TOOLS`),
jamais mis en cache. Universels (tous les jeux), sauf l'analyse du déploiement, propre à Minecraft
Java.

**Console en direct.** À la première demande pour un serveur, le processus MCP ouvre sa propre
connexion WebSocket (`agent:<id>:<époque>`, distincte des onglets et du superviseur) et tient un
tampon de **5 000 lignes** numérotées. Les lignes que Wings rejoue à la connexion sont marquées
historiques (`history`) ; une reconnexion est raccordée sans doublon (recouvrement des dernières
lignes) ou signalée dans `notes` ; un tampon qui déborde entre deux lectures le dit (`missed`).
Curseur `<époque>-<numéro>` : celui d'une connexion fermée depuis est accepté et signalé
(`cursor_reset`), pas une erreur. Fermeture après **10 min** sans lecture, au changement de compte
ou de clé. Sans WebSocket (connexion impossible, serveur suspendu), repli sur l'API REST
(`source: rest`, sans curseur), dit dans la sortie. Envoi toujours par REST, WebSocket en lecture
seule ; tout passe par l'anonymisation IA, et les motifs (`filter`, `until`, `pattern` : texte, ou
expression régulière avec `regex`, casse ignorée) s'appliquent au texte anonymisé, celui que voit
l'agent.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `tail_console` | | `server_id`, `after?`, `wait_secs?`, `max_lines?`, `filter?`, `regex?` | Sans `after` : les dernières lignes (100 par défaut, max 1 000) et un `cursor`. Avec `after` : **seulement** les lignes apparues depuis, en attendant au plus `wait_secs` (max 60 s, 30 s pour le Copilote) la première ; `more` si le plafond (lignes ou 48 Kio) coupe la lecture, à reprendre avec le nouveau curseur. Toujours `state` (`running`, `starting`, `stopping`, `offline`), `missed`, `history`, `notes` au besoin. |
| `run_command` | ✎ | `server_id`, `command`, `until?`, `regex?`, `timeout_secs?`, `max_lines?` | Attend la console prête (historique reçu), **refuse sans rien envoyer** si le serveur est hors ligne, envoie par `PUT /server/{id}/command`, puis rend les lignes apparues **après l'envoi** : jusqu'à `until`, ou sans `until` jusqu'à 0,8 s de silence après la première ligne, au plus `timeout_secs` (défaut 5, max 60). `{ sent, lines, matched, matched_line, timed_out, cursor, state }` ; le curseur permet de lire la suite avec `tail_console`. Une seule ligne de commande. |
| `wait_for` | | `server_id`, une condition : `state` \| `pattern` (+ `regex?`, `after?`) \| `job_id` ; `timeout_secs?` | Attente bornée (défaut 60 s, max 600, **120 pour le Copilote**), avancement signalé, arrêt si le client annule. `{ reached, elapsed_secs, state, reason?, matched_line?, job?, cursor }`. **`state=running`** : juste après un arrêt ou un redémarrage demandé (moins de 2 min), l'ancien « running » n'est retenu qu'après un passage par un autre état ou si le processus a démarré après la demande ; un arrêt pendant le démarrage (`starting` → `offline` sans `stopping`) rend la main aussitôt (`reason`) ; résumé **`startup`** : durée, ligne « Done » et durée annoncée, erreurs et avertissements de la console regroupés par signature avec leur plugin ou mod probable (regroupement d'`error_digest`). État lu sur le WebSocket, sinon sondé par l'API toutes les 4 s. `job_id` : `GET /user/{id}/backup_job/{job}` toutes les 3 s (snapshot, reprise d'un serveur en pause). |
| `deploy_and_verify` | ✎ | `server_id`, `local_path`, `remote_path?`, `restart?`, `replace_previous?`, `timeout_secs?` | Boucle d'un agent qui développe un plugin : lit le `.jar` **local** et son descripteur (nom, version, plateforme ; avertit si le serveur charge une autre plateforme), l'envoie par SFTP (défaut `/plugins/<fichier>` pour un plugin, `/mods/<fichier>` pour un mod), cherche dans le dossier un **autre jar du même plugin** (même identifiant de descripteur) : mis de côté en `.old` avec `replace_previous=true`, sinon signalé. Puis redémarre (démarre un serveur éteint ; `restart=none` : envoi seul, non vérifié), attend `running` (défaut 180 s, max 600) et analyse la console **du démarrage seulement**. Verdict : `ok`, `plugin`, `version`, `uploaded_bytes`, `startup_secs`, `server_reported_secs`, `enabled` + `evidence` (Bukkit/Paper : « Enabling Nom vX » ; Spigot « Could not load », « Error occurred while enabling », désactivation juste après l'activation ; Velocity, BungeeCord ; Fabric : liste des mods chargés ; Forge/NeoForge : **déduit** d'un démarrage complet sans erreur liée au mod), `loaded_version` (une autre version chargée = un autre jar a gagné), `errors` et `warnings` dont la source probable est ce plugin, `other_errors`, `deploy_warnings`, `console_tail`. `ok` exige le démarrage, l'activation confirmée, aucune erreur du plugin, ni doublon ni autre version chargée. Avancement à chaque étape. |

`deploy_and_verify` lit un fichier de l'ordinateur : il rejoint `LOCAL_DISK_TOOLS` et n'est
**jamais** donné au Copilote. `run_command` est modifiant comme `send_command` : offert au Copilote
en mode autonome, borné au serveur de la session ; `tail_console` et `wait_for` sont des lectures,
offertes même au diagnostic automatique (délais plafonnés plus bas hors transport MCP).

#### Construire et reproduire un serveur

Monter, reproduire ou ajuster un serveur de A à Z sans casser ses fichiers
(`core/src/config_edit.rs`, `core/src/manifest.rs`). Absents du MCP officiel.

**Configs structurées.** `get_config` et `set_config` adressent une valeur par **chemin de clé**
au lieu de chercher et remplacer du texte (ce qui casse l'indentation YAML ou les types). Format
d'après l'extension : YAML (`.yml`, `.yaml`, analyseur YAML 1.2 strict à positions), TOML
(`toml_edit`), JSON (`.json`, `.json5`, `.jsonc`, `.mcmeta`, commentaires et virgules finales
tolérés) et `.properties` (règles de `java.util.Properties`, valeurs écrites en ASCII avec
`\uXXXX`). Chemin de clé : `a.b.c`, index de liste `a.list[0]` (`[n]` égal à la longueur ajoute
un élément), clé contenant un point entre guillemets ou crochets
(`forced-hosts."lobby.example.com"`, `a['b.c']`) ou échappée (`a\.b`) ; dans un `.properties`,
la clé entière (`rcon.password`).

- **Édition ciblée** : seule la plage de la valeur est réécrite (commentaire de fin de ligne,
  guillemets d'origine gardés) ; une clé absente est insérée après la dernière ligne de sa
  section, à l'indentation de ses sœurs (pas d'indentation et style des listes détectés). Les
  types des scalaires YAML suivent SnakeYAML (`yes`/`on` = booléens) : un texte qui serait lu
  autrement est écrit entre guillemets (`'true'`, `'1.20'`).
- **Vérification avant écriture** : le résultat est relu par l'analyseur strict et comparé à
  l'arbre attendu (l'ancien plus les seules modifications demandées). S'il ne s'analyse plus,
  si la valeur relue diffère ou si une autre clé a bougé, **rien n'est écrit**. Fichier déjà
  invalide : refusé (à corriger avec `edit_file`) ; `get_config` le lit alors avec l'analyseur
  tolérant (`yaml_lite`), avec un avertissement. Refusés aussi : clé en double, nœud YAML
  ancré (`&`) ou alias (`*`), plusieurs documents YAML.
- **Types** : valeurs JSON typées. Un texte envoyé sur un nombre ou un booléen existant en garde
  le type (`"10"` → `10`) sauf `keep_type=false` ; un nombre envoyé sur un texte est gardé
  nombre (`view-distance: default` → `8`), signalé dans `warnings`.
- **Secrets** : toute valeur sous une clé de type mot de passe, secret, jeton ou clé (`password`,
  `secret`, `token`, `key`, `api-key`, `webhook`, `credential`, `pass`, `pwd`…, à n'importe quel
  niveau du chemin) sort masquée en `[SECRET]` : lecture, ancien et nouvel état, diff des
  deux côtés. Booléens et valeurs vides ne sont pas masqués. Une valeur contenant `[SECRET]`,
  `[CREDS]`, `[IP]` ou `[EMAIL]` (masques de lecture) est refusée : elle écraserait la vraie
  valeur. Sorties passées ensuite par l'anonymisation IA (`redact.rs`).

**Manifeste.** Un agent écrit mieux une description complète qu'il n'enchaîne quarante appels :
`export_manifest` décrit un serveur existant, `plan_manifest` compare un manifeste au serveur
réel, `apply_manifest` exécute l'écart, `clone_server` crée une copie de test d'un serveur.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `get_config` | | `server_id`, `path`, `key?` | Avec `key` : valeur **typée** (`found`, `type`, `value` ; section trop longue → son arborescence). Clé absente : `deepest_existing` et `available_keys` (clés du niveau le plus proche). Sans `key` : arborescence aplatie (`keys[{key, type, value}]`, 300 valeurs au plus, listes de scalaires d'un bloc), `secrets_masked`. |
| `set_config` | ✎ | `server_id`, `path`, `changes[{key, value}]`, `create_missing?`, `keep_type?`, `dry_run?` | Modifications vérifiées puis écrites (SFTP) : par clé `modifiée`, `créée` ou `inchangée` avec ancienne et nouvelle valeur (masquées si secrètes) et note de type ; diff unifié masqué ; `written` ; rien n'est écrit si tout est déjà conforme ou en `dry_run`. 100 modifications au plus par appel. `create_missing=false` : une clé absente fait tout refuser. |
| `export_manifest` | | `server_id`, `include_configs?[chemin \| {path, keys}]`, `include_players?` | `{ manifest, notes }` : manifeste v1 du serveur — jeu (`game`, `egg_id`), `game_version` et `loader` (détectés par l'hébergeur), `java_version` (image d'exécution), `properties` (secrets en `[SECRET]` ; `server-port`, `server-ip`, `query.port`, `rcon.port`, propres à l'allocation du serveur, omis), `startup`, `content` (fichier, version, source et projet quand l'hébergeur les identifie, sinon `source: null`), `players` (alias si l'anonymisation IA est active), `schedules`, `configs` demandées (toutes les valeurs, 300 au plus, ou les clés données). Notes : contenu non identifié, secrets masqués, ce qui n'est pas exporté (mondes, fichiers). |
| `plan_manifest` | | `server_id`, `manifest` | **Ne modifie rien.** `steps[{id, status, summary, current?, target?, diff?, depends_on?, official?}]` dans l'ordre d'application, `counts` par statut, `to_apply`, `unlisted` (présent sur le serveur, absent du manifeste, gardé), `next`. `official` : outil du MCP officiel qui fait une étape `manual` (voir ci-dessous). |
| `apply_manifest` | ✎ | `server_id`, `manifest`, `steps?`, `snapshot?` | Plan recalculé puis étapes `change`/`add`/`remove` exécutées dans l'ordre. Snapshot préalable **attendue** par défaut (15 min au plus ; sans elle rien n'est modifié). Rapport : `steps[{id, status, outcome, summary, detail?}]` (`réussie`, `conforme`, `ignorée`, `échouée` + raison, `non sélectionnée`, `non exécutée` après annulation), totaux, `manual` (ce qui reste à faire), `official` (`[{step, official}]` : outil officiel de chaque étape manuelle qui en a un), `next`. Progression par étape, arrêt propre sur annulation. |
| `clone_server` | ✎ | `source_server_id`, `mybox_id`, `name`, `cpu?`, `ram?`, `include_world?`, `exclude?[]`, `location_id?`, `required_fields?` | Crée un serveur du même jeu (validations de `mybox_create_server` ; CPU/RAM de la source si la MyBox a la place, sinon ce qui reste ; localisation de la source si en stock), attend la fin de l'installation (20 min au plus), l'arrête s'il a démarré, copie les fichiers de la source (`copy_between_servers`, écrasement des fichiers par défaut du serveur neuf ; mondes exclus — `level-name`, ses dimensions et tout dossier racine contenant un `level.dat` — sauf `include_world` ; `logs`, `crash-reports`, `debug` jamais ; `exclude` peut viser un sous-dossier ; ports et adresse du clone rétablis dans `server.properties` après la copie), puis applique le manifeste de la source sans snapshot. Sortie : `created_server_id`, ressources, durée d'installation, lots de copie, rapport du manifeste, `warnings`. En cas d'échec après la création, l'erreur cite `created_server_id=<id>`. |

Classement : `get_config`, `export_manifest`, `plan_manifest` sont des lectures (cache de 10 s,
purgé par toute écriture) ; avec `set_config`, ils font partie des exclusifs gardés face au MCP
officiel (`LOCAL_KEEP_TOOLS`). `set_config` est offert au Copilote en mode autonome, borné au
serveur de la session comme `edit_file`. `apply_manifest` (commande de démarrage, tâches
planifiées : configuration persistante) et `clone_server` (création de serveur, copie entre
serveurs) sont dans `INFRA_TOOLS` : jamais donnés au Copilote. Aucun ne lit ni n'écrit sur le
disque de l'ordinateur : le manifeste passe en argument et revient en sortie.

##### Format du manifeste (v1)

Objet JSON passé tel quel dans `manifest` (un texte JSON est aussi accepté). Toute section est
facultative ; un champ inconnu est refusé (faute de frappe). `[SECRET]` (ou toute valeur
contenant un masque d'anonymisation) veut dire **« garder la valeur actuelle »** : l'étape est
ignorée, le masque jamais écrit.

```json
{
  "version": 1,
  "server": { "name": "Survie", "game": "Minecraft", "game_version": "1.21.4", "loader": "paper", "java_version": 21 },
  "properties": { "max-players": 50, "white-list": true, "rcon.password": "[SECRET]" },
  "startup": { "parameters": "java -Xms128M -Xmx6G -jar {{SERVER_JARFILE}} nogui" },
  "content": {
    "items": [
      { "kind": "plugin", "source": "modrinth", "project": "luckperms", "version": "v5.4.145-bukkit" },
      { "kind": "plugin", "source": "spigot", "project": "34315", "file": "Vault.jar" },
      { "kind": "datapack", "source": "modrinth", "project": "terralith" }
    ],
    "remove_unlisted": false
  },
  "configs": [ { "path": "/plugins/LuckPerms/config.yml", "set": { "storage-method": "mysql", "data.address": "db:3306" } } ],
  "files": [ { "path": "/plugins/Essentials/motd.txt", "content": "Bienvenue sur Survie !" } ],
  "players": { "whitelist": ["Alice", "Bob"], "ops": ["Alice"], "remove_unlisted": false },
  "schedules": [
    { "name": "Redémarrage nocturne", "cron": "0 4 * * *", "tasks": [
      { "action": "command", "payload": "say Redémarrage dans 60 s" },
      { "action": "power", "payload": "restart", "time_offset": 60 } ] }
  ]
}
```

| Section | Comparé à | Appliqué par | `manual` / `unsupported` |
|---|---|---|---|
| `server` | nom, jeu (`egg_id` ou `game`), version et plateforme détectées (`GET /server/{id}/minecraft_version`), Java (image de démarrage) | `rename_server` (nom seulement) | Autre jeu, version du jeu, plateforme, Java : **manual** (endpoints d'écriture non documentés, jamais appelés : `POST /server/{id}/version`, `PATCH …/java/version`, `PATCH …/docker/image`) ; version, plateforme et Java indiquent l'outil officiel à appeler (`official`, ci-dessous). Valeur actuelle illisible : manual. |
| `properties` | `server.properties` (API) | `set_property` ; clé absente du fichier : ajoutée par SFTP (`set_config`) | Valeur liste/objet : unsupported. Autre jeu que Minecraft : unsupported. |
| `startup` | commande et JAR de démarrage | `set_startup_params` | `jar_file` différent : manual (`PATCH …/startup/file` non géré ; outil officiel `set_startup_config` indiqué). Commande vide : unsupported. |
| `content` | plugins, mods, datapacks installés (rapprochés par source + projet, puis fichier, puis nom) | installation par la marketplace (`install_mod` : Modrinth/Spigot ; CurseForge et datapacks par les mêmes endpoints que les mises à jour), version résolue au plan (identifiant, numéro de version ou fichier ; sans version : la plus récente compatible) ; changement de version = installation puis retrait de l'ancien fichier ; `remove_unlisted: true` retire le reste | Élément absent sans `source`/`project` (non identifié) : manual (à copier). (Dés)activation différente : manual. Plugin CurseForge, mod Spigot : unsupported. Version introuvable : unsupported. Un élément présent sans version imposée est conforme. |
| `configs` | clés des fichiers (comme `set_config`, diff dans le plan) | `set_config` | Fichier absent : manual (démarrer le serveur une fois pour que le plugin le génère, puis relancer : idempotent). Fichier illisible ou édition refusée : unsupported. |
| `files` | contenu exact | écriture SFTP (dossiers créés) | Contenu avec un masque, fichier de plus de 512 Kio (2 Mio au total) : unsupported. |
| `players` | `/whitelist.json`, `/ops.json` | `player_action` (serveur **démarré**) ; `remove_unlisted: true` retire les autres | Alias d'anonymisation (`joueur-xxxxxx`) non reconnu sur ce serveur : manual. |
| `schedules` | tâches du panel par nom (cron, drapeaux, actions dans l'ordre) | création, mise à jour (en-tête et actions une à une), suppression avec `remove_unlisted: true` | Cron ou action invalide, commande masquée : unsupported (vérifié au plan, avant tout envoi). |

Identifiants d'étape stables : `server.name`, `startup.parameters`, `properties.<clé>`,
`content.<plugin|mod|datapack>.<projet ou fichier>`, `configs.<chemin>`, `files.<chemin>`,
`players.<whitelist|ops>.<pseudo>`, `schedules.<nom>`. `apply_manifest` les accepte dans `steps`,
exactement ou en préfixe (`"properties"`, `"content.plugin"`). Une config ou un fichier sous
`/plugins/<X>/` ou `/config/<x>` dépend de l'installation de `<X>` dans le même manifeste : si
elle échoue, l'étape est ignorée (`depends_on`). **Idempotence** : le plan est recalculé à chaque
application, une étape déjà conforme n'est jamais refaite ; relancer après une coupure reprend là
où le serveur en est. Après des changements de contenu, de réglages ou de configs, un
redémarrage les charge (`next`).

**Étapes `manual` reliées aux outils officiels** (`core/src/manifest/official.rs`). Le manifeste
n'applique ni la version du jeu, ni la plateforme, ni Java, ni le JAR, mais l'étape `manual` porte
`official { tool, arguments, before?, note?, available, availability }` : l'outil du MCP officiel
qui le fait, ses arguments (`<…>` = valeur à obtenir d'abord par `before`, ou à demander à
l'utilisateur — jamais devinée), et s'il est appelable **maintenant** par ce client : passerelle
vers le MCP officiel active (`official_gateway`, Réglages → MCP, ou `MINESTRATOR_MCP_OFFICIAL=1`),
groupe `official` actif, écritures permises, outil relayé ; sinon `availability` dit quoi activer
(ou le panel). Arguments d'après les schémas de l'officiel :

| Étape | Outil officiel | Arguments | Avant / notes |
|---|---|---|---|
| `server.game_version`, `server.loader` | `change_version` (destructif : `confirm_name` = nom exact du serveur, donné par l'utilisateur) | chargeur `fabric` / `forge` / `neoforge` : `loader`, `game_version` (+ `build` facultatif) ; autre plateforme (Paper, Spigot, Vanilla…) : `version_id`, `game_version` | `get_content_files type=loader_builds` pour un build précis ; `search_game_content type=version` pour l'`id` d'une entrée. Une version **antérieure** est signalée : la plateforme la refuse, seule une réinstallation propre (`reinstall`, contenu perdu) le permet. Quilt : aucun outil officiel. Les deux étapes pointent le même appel. |
| `server.java` | `manage_java` | `java_version_id` | `manage_java` sans autre paramètre liste les versions et leurs identifiants ; redémarrage ensuite. |
| `startup.jar_file` | `set_startup_config` | `file` | Le JAR doit déjà exister sur le serveur (`list_files`) ; redémarrage ensuite. La passerelle relaie cet outil à côté de `set_startup_params`, qui ne change que la commande. |

#### Monter un serveur : prégénération et base de données

Deux étapes d'une mise en place complète que rien ne couvrait (`core/src/pregen.rs`,
`core/src/db_connect.rs`). Absentes du MCP officiel ; dans `INFRA_TOOLS` (jamais au Copilote :
opération lourde pour l'une, identifiants d'une ressource de la MyBox et mot de passe partagé pour
l'autre) ; aucun accès au disque de l'ordinateur.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `pregenerate_world` | ✎ | `server_id`, `action` (`start` \| `status` \| `pause` \| `continue` \| `cancel`), `radius?` (blocs, requis pour `start`), `world?`, `center?`, `shape?`, `install_chunky?`, `replace_existing?`, `wait_secs?` | Prégénération avec **Chunky** (plugin Paper/Spigot/Folia/Sponge, mod Fabric/Quilt/Forge/NeoForge). `start` : sélection (`chunky world`, `shape`, `spawn` ou `center x z`, `radius`) puis `chunky start`, chaque réponse attendue dans la console. `world` : `level-name` par défaut ; `overworld`, `nether`, `end` et les identifiants `minecraft:…` traduits selon la plateforme (dossier `<monde>_nether` sur un plugin, dimension sur un mod). `center` : `spawn` (défaut) ou `"x,z"`. `shape` : `square` (défaut), `circle`, `diamond`, `triangle`, `pentagon`, `hexagon`, `star`. Sortie : `chunky` (installé, version), `selection`, `estimate`, `started`, `tasks[{world, state, percent, chunks, total_chunks, rate_cps, eta, eta_secs, current, total_time}]`, `console` (réponses de Chunky par commande), `notes`, `next`. |
| `connect_database` | ✎ | `server_id`, `mybox_id?`, `database_id?`, `target?` \| (`path` + `keys`, `set?`), `path?`, `rotate_password?`, `dry_run?` (défaut **true**) | Accès de la base (hôte, port, nom, utilisateur ; mot de passe « lu, jamais affiché ») écrits dans la config du plugin par `set_config` : `changes` et `diff` (mot de passe masqué des deux côtés), `written`, `rotated`, mention de la garde (étape annulable), `notes`, `next` (redémarrage). |

**`pregenerate_world`.**

- **Chunky** est cherché dans le contenu installé (plugins ou mods selon la plateforme détectée
  par l'hébergeur ; `ChunkyBorder` et consorts ne comptent pas). Absent : erreur qui explique
  `install_chunky=true`, qui l'installe depuis Modrinth — version la plus récente (stable d'abord)
  qui déclare un loader de la plateforme (Folia : `folia` ; Spigot : `spigot`/`bukkit`, jamais
  un build réservé à Paper ; Quilt accepte Fabric) et la version du jeu — par `install_mod`, passé
  par la garde des agents (étape annulable) ; il faut ensuite redémarrer (`next`). Désactivé :
  erreur.
- **Estimation du disque** avant `start`, **ordre de grandeur** : chunks de la zone (aire de la
  forme ÷ 256) × 6 Kio (bas), 12 Kio (typique), 20 Kio (haut) par chunk, d'après des mondes 1.18+
  prégénérés (régions compressées, entités et points d'intérêt) ; comparée au plus petit espace
  libre entre le serveur et la MyBox. Avertissement si l'estimation haute dépasse (ou si la
  typique dépasse 80 %), **refus seulement si même l'estimation basse dépasse** ; espace libre
  inconnu : dit dans la sortie. Les chunks déjà générés ne reprennent pas de place.
- **Commandes** par la boucle « agir puis vérifier » (`run_command` : envoi REST, réponse lue dans
  la console par curseur) ; **serveur hors ligne, en démarrage ou en arrêt : refus explicite,
  rien n'est envoyé**. Commande `chunky` inconnue (plugin non chargé) : erreur qui conseille le
  redémarrage. Commande de sélection refusée par Chunky (monde introuvable, valeur invalide) :
  arrêt avant `chunky start`, qui partirait sinon sur l'ancienne sélection. Tâche déjà existante (Chunky demande `/chunky confirm`) : `start` refuse et
  propose `continue`, sauf `replace_existing=true` (confirmation envoyée, progression perdue) ;
  pour `cancel`, demandé explicitement, la confirmation est envoyée. Tâche déjà en cours : erreur
  (`status`, `pause`, `cancel`).
- **Suivi** : `status` envoie `chunky progress` (repli : dernières lignes de Chunky de la
  console). Lignes analysées : progression (`Task running for <monde>. Processed: N chunks
  (P%), ETA: h:mm:ss, Rate: R cps, Current: x, z`) et fin (`Task finished … Total time`),
  **relevées sur un serveur réel** (Chunky 1.5.66, Paper) ; Chunky y nomme le monde par son
  identifiant (`minecraft:overworld`) même sur Paper, rapproché du dossier choisi. Démarrage
  (`Task started in <monde> for the <forme> region centered at x, z with radius r`), « aucune
  tâche » (`No tasks running.`, `No tasks to continue.`), réponses de sélection, pause (`Task
  paused for <monde>.`), demande de confirmation de `cancel` puis `Task stopped for <monde>.` :
  relevés aussi en réel. Seule la reprise (`Task continuing…`) reste un format **supposé** ; une
  réponse non reconnue est signalée, jamais interprétée. Particularité de Chunky : juste après une
  pause, `chunky progress` décrit encore la tâche « running » à 0 cps. Une tâche déjà terminée
  dans la réponse au lancement n'est pas suivie. `wait_secs` (max 600) : suit la console, avancement MCP
  (`notifications/progress`, pourcentage), arrêt à la fin de la tâche, à l'échéance ou si le
  client annule (la tâche continue sur le serveur).
- **Garde des agents** : `start`, `pause`, `continue`, `cancel` sont des étapes **non annulables**
  (les chunks générés restent) ; `status` n'en crée pas.

**`connect_database`.**

- **Accès** : `GET /mybox/{id}` → `databases[]` (l'outil officiel `get_mybox` annonce « host,
  port, nom, mot de passe »). Champs relevés sur une vraie base (MariaDB 11.8, 2026-11-19) :
  `id`, `dbname`, `host`, `port`, `username`, `password`, `description` — **aucun champ de
  moteur** : il est déduit de l'hôte (`mariadb1.…`), sinon MySQL/MariaDB est supposé et dit. La
  lecture reste tolérante (noms candidats par champ — `host`/`hostname`/`address`…, `port`,
  `database`/`dbname`/`db_name`/`name`…, `username`/`user`/`login`…, `password`/`pass`/`pwd`… —,
  un niveau d'imbrication, `hôte:port` dans l'hôte, port 3306 supposé et dit s'il manque). Champ indispensable absent : erreur qui liste les **noms** des champs vus, jamais leurs
  valeurs. Mot de passe absent ou masqué : repli sur l'outil officiel `get_mybox` (connexion en
  lecture seule), jamais montré au modèle. `database_id` absent : la seule base de la MyBox,
  sinon la liste (identifiant et nom) est rendue en erreur. MyBox sans base : renvoi vers l'outil
  officiel `create_database` (passerelle) ou le panel — l'outil **ne crée pas** de base.
- **Préréglages** (`target`, MySQL/MariaDB ; clés jamais créées : le plugin doit avoir généré sa
  config, sinon erreur avant toute modification ; `path` remplace le fichier par défaut) :

  | `target` | Fichier | Clés écrites |
  |---|---|---|
  | `luckperms` | `/plugins/LuckPerms/config.yml` | `storage-method` (`mariadb` ou `mysql` selon le moteur), `data.address` (hôte:port), `data.database`, `data.username`, `data.password` |
  | `coreprotect` | `/plugins/CoreProtect/config.yml` | `use-mysql: true`, `mysql-host`, `mysql-port`, `mysql-database`, `mysql-username`, `mysql-password` |
  | `litebans` | `/plugins/LiteBans/config.yml` | `sql.driver` (`MariaDB` ou `MySQL`), `sql.address` (hôte:port), `sql.database`, `sql.username`, `sql.password` |
  | `plan` | `/plugins/Plan/config.yml` | `Database.Type: MySQL`, `Database.MySQL.Host`, `.Port`, `.User`, `.Password`, `.Database` |

  Correspondance explicite : `path` + `keys` { `host` \| `port` \| `address` \| `database` \|
  `username` \| `password` : clé de config } et `set` (réglages non secrets en plus). La clé du
  mot de passe doit être reconnue comme secrète par l'édition structurée (`password`, `pass`,
  `pwd`, `secret`, `credential`…), sinon refus : sa valeur apparaîtrait dans le diff.
- **Secret** : le mot de passe ne figure jamais dans les arguments de l'outil (journalisés) ; il
  ne passe que dans l'appel interne à `set_config`, dont la sortie et le diff le masquent (clé
  secrète), puis toute sortie et toute erreur sont encore nettoyées de sa valeur. Un test vérifie,
  pour chaque préréglage sur un fichier réaliste, qu'il est écrit dans le fichier mais absent de
  la sortie et du diff.
- **`dry_run`** (défaut) : simulation complète, rien n'est écrit. Sinon : simulation d'abord (un
  fichier ou une clé manquants arrêtent tout), puis écriture par `set_config` passée par la garde
  des agents (copie du fichier avant : annulable avec `rollback_changeset`).
- **`rotate_password`** (défaut false), seulement si le mot de passe n'est pas lisible : la
  confirmation est demandée à l'humain (élicitation MCP) **avant tout effet** — tout ce qui
  utilise l'ancien mot de passe sera cassé ; refus : rien n'est fait. Réinitialisation par l'outil
  officiel `manage_database` (`action: reset_password`, corps connu) plutôt que par `PATCH
  /mybox/{id}/database/password` (corps non documenté, jamais essayé), relecture du nouveau mot de
  passe, puis écriture directe. Non annulable (le mot de passe ne revient pas en arrière).

#### Réseaux proxy (Velocity, BungeeCord)

Monter et contrôler un réseau proxy → serveurs de jeu sans que l'agent voie jamais le secret de
transfert (`crates/minestrator-core/src/network.rs`, `network/link.rs`). Absents du MCP officiel.
Minecraft Java seulement.

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `network_topology` | | `server_id?` | **Lecture seule.** Proxys du compte, repérés par la plateforme de l'hébergeur (`GET /server/{id}/minecraft_version`), le nom du jar de démarrage, ou leurs fichiers (`velocity.toml`, `config.yml` de BungeeCord avec `listeners`). Pour chacun : type, config, `online_mode`, mode de transfert, secret **défini ou absent** (Velocity) ou BungeeGuard (BungeeCord), liste de repli `try`, et chaque entrée : nom, adresse, `in_try`, serveur du compte reconnu (`account_server` : id, nom, rapprochement) et ses réglages (`settings` : plateforme, version, `online_mode`, transfert Velocity, secret **identique / DIFFÉRENT / absent**, `spigot_bungeecord`, BungeeGuard, FabricProxy-Lite). `problems[]` triés (critique, important, info) avec la correction, `summary`, `not_read` (serveurs suspendus, proxys illisibles). `server_id` : seulement les réseaux qui contiennent ce serveur (proxy ou serveur de jeu) ; un serveur réglé pour un proxy que rien ne référence est signalé. Cache de 15 s. |
| `link_proxy` | ✎ | `proxy_server_id`, `backend_server_ids[]`, `names?`, `try?`, `mode?`, `dry_run?` (défaut **true**), `snapshot?` (défaut true) | Plan puis, sans `dry_run`, application : entrées du proxy (`servers[{name, address, server_id, entry: ajoutée \| mise à jour \| inchangée}]`), liste de repli, `secret` (réutilisé ou généré, jamais affiché), `steps[{id, server_id, status: ok \| change \| manual \| refused, summary, changes?, diff?, outcome?}]`, `counts`, `manual`, `warnings`, `snapshots`, `status`, `next`. |

**Rapprochement des adresses.** Une entrée est associée à un serveur du compte par son IP et son
port principal (`GET /server/{id}` : `server.ip`, `server.port` ; repli sur l'adresse de
`list_servers`), un port secondaire (`ports[]`), le nom DNS du serveur (`server.dns`, avec le bon
port), un nom d'hôte résolu, ou le port seul pour une adresse privée (signalé à confirmer). Une
adresse locale (`127.0.0.1`, `localhost`) ne mène à aucun autre serveur : chaque serveur
MineStrator a son conteneur, et deux serveurs d'une même MyBox peuvent tourner sur des machines
différentes (`id_daemon`). L'API ne publie pas d'adresse interne entre serveurs (le type
`internal` des ports, côté MCP officiel `manage_ports`, n'apparaît pas dans `ports[]`) :
`link_proxy` écrit donc l'`ip:port` publique.

**Problèmes signalés** : proxy en `online-mode=false` ; transfert désactivé alors que des serveurs
sont reliés ; transfert moderne sans secret (Velocity refuse de démarrer) ; serveur de repli ou hôte
forcé non déclaré ; entrée locale, inconnue du compte, sans le bon port, ou qui vise le proxy
lui-même ; serveur de jeu en `online-mode=true` derrière le proxy ; transfert Velocity désactivé
(critique s'il est aussi en `online-mode=false`) ; secret différent ou absent ;
`settings.bungeecord: true` avec le transfert moderne ; Spigot ou vanilla derrière le transfert
moderne, Minecraft < 1.13 ; FabricProxy-Lite absent ou secret différent ; transfert legacy sans
BungeeGuard alors que le serveur est joignable directement (usurpation) ; jeton BungeeGuard
absent des `allowed-tokens`.

**`link_proxy`.** Modes : `modern` (défaut pour Velocity : transfert signé, Paper 1.13+) ou
`legacy` (défaut et seul mode de BungeeCord ; Velocity `legacy`). Côté proxy, en une édition
vérifiée comme `set_config` : `servers.<nom>` (Velocity) ou `servers.<nom>.address` (BungeeCord,
avec `motd` et `restricted` pour une entrée neuve), liste de repli (`servers.try`,
`listeners[0].priorities`), `player-info-forwarding-mode` (casse de la valeur actuelle gardée) ou
`ip_forward: true`. Nom d'une entrée : `names`, sinon celui d'une entrée qui vise déjà le serveur,
sinon tiré du nom du serveur (`Serveur Créatif` → `serveur-creatif`) ; une entrée locale du même
nom (exemple `lobby` de la configuration par défaut) est reprise, les autres entrées locales sont
gardées et signalées. Liste de repli : `try` (noms ou IDs des serveurs de l'appel), sinon
l'actuelle si tous ses noms restent déclarés et qu'au moins un mène à un serveur, sinon le premier
serveur de jeu. Côté serveurs Paper : `proxies.velocity.enabled`, `online-mode` (celui du proxy) et
`secret` dans `config/paper-global.yml` (`settings.velocity-support.*` de `paper.yml` avant 1.19),
`settings.bungeecord` dans `spigot.yml` (à `false` en modern, `true` en legacy, avec
`proxies.bungee-cord.online-mode`), `online-mode=false` (`set_property`).

- **Secret** : celui du proxy (`forwarding-secret-file`, défaut `forwarding.secret`, ou
  `forwarding-secret` des anciennes versions) est lu en interne et réutilisé ; à défaut, 32
  caractères tirés du générateur aléatoire du système (`ring`, via rustls), écrits d'abord dans le
  fichier du proxy (une relance après coupure réutilise donc le même), puis dans chaque
  `paper-global.yml`. Il ne figure ni dans la sortie, ni dans les diffs (masqués par
  `config_edit`), ni dans le journal des agents (qui ne consigne que l'appel `link_proxy`, sans
  secret) ; toute valeur lue est en plus effacée des sorties. La garde copie l'ancien contenu des
  fichiers modifiés dans `agent_guard.db` (local) pour pouvoir les rétablir, comme pour toute
  écriture.
- **Application** : rien n'est écrit si une étape est `refused` (fichier invalide, chemin du secret
  hors du serveur) ; snapshot de chaque serveur modifié, **attendue** (sans elle rien n'est
  modifié ; `snapshot=false` pour s'en passer) ; puis une écriture par fichier, proxy d'abord, par
  `set_config`, `write_file` et `set_property` via la garde des agents : chacune est une étape
  annulable (`rollback_changeset`). Arrêt au premier échec ; progression et annulation suivies.
  Effet au redémarrage des serveurs de jeu puis du proxy (`next`).
- **`manual`** (expliqué, rien d'improvisé) : Spigot ou vanilla en modern, Minecraft < 1.13,
  Fabric (FabricProxy-Lite et son secret, à recopier à la main), Forge/NeoForge (mod de transfert),
  serveur jamais démarré (`paper-global.yml` ou `spigot.yml` absent : le démarrer une fois, puis
  relancer), serveur illisible ou suspendu, BungeeGuard à installer en legacy. Les serveurs restent
  déclarés sur le proxy. **Idempotent** : relancé sur un réseau relié, tout est `ok`.

Classement : `network_topology` est une lecture (offerte au Copilote, bornée au serveur de sa
session) ; `link_proxy` est dans `INFRA_TOOLS` (plusieurs serveurs, secret) : jamais donné au
Copilote. En mode constructeur, `link_proxy` vise le proxy **et** chaque serveur de jeu. Le
prompt `setup_velocity_network` enchaîne `network_topology` puis `link_proxy`.

#### Garde des agents

Journal et annulation des actions des agents (`crates/minestrator-core/src/agent_guard.rs`),
absents du MCP officiel. Données locales partagées par l'app et les processus MCP
(`agent_guard.db`) ; fonctionnement, plafonds et réglages au §6 (« Garde des agents »).

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_agent_actions` | | `server_id?`, `session?`, `since?`, `only_writes?`, `limit?` | **Journal** des appels reçus par le serveur MCP (clients externes, agents CLI du Copilote), du plus récent au plus ancien : horodatage, session, client, outil (✎ si modifiant), serveurs visés, arguments résumés et **anonymisés** (`redact`, contenus de fichiers remplacés par leur taille, secrets masqués), résultat (ok, erreur, refusé + motif), durée, étape de changeset créée. `since` : `30m`, `12h`, `7d`, `AAAA-MM-JJ` ou horodatage Unix ; `limit` 30 par défaut, 200 au plus. |
| `list_changesets` | | `changeset_id?`, `server_id?`, `limit?` | **Modifications** des agents regroupées par session (changeset) : étapes dans l'ordre (outil, cible, serveur), annulable ou **non** et pourquoi, déjà annulée ou passée. `server_id` ne garde que les étapes de ce serveur ; `limit` 10 par défaut, 50 au plus. |
| `rollback_changeset` | ✎ | `changeset_id`, `steps?`, `force?` | **Annule** les étapes encore en place (les `steps` dernières, toutes par défaut), de la plus récente à la plus ancienne. Toutes les cibles d'une étape sont vérifiées avant d'y toucher : une cible changée depuis l'action (sha256, ou taille et date) **refuse** l'étape et arrête l'annulation, sauf `force`. Étapes non annulables signalées et passées. Confirmation demandée à l'humain quand le client sait le faire (élicitation MCP) ; sinon l'annulation a lieu. Rapport étape par étape. Dans `INFRA_TOOLS` : jamais au Copilote. |

Données au Copilote : `list_agent_actions` et `list_changesets` ne lui montrent que le serveur de la
conversation (imposé à ses agents CLI, `server_id` exigé dans l'app).

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
- **Plusieurs serveurs** : `copy_between_servers` (copie de serveur à serveur, par archive pour un
  monde), `diff_files` (comparer configs ou dossiers), `find_player` (un joueur sur tout le compte),
  `broadcast_command` (une commande à plusieurs serveurs)
- **Contenu, monde et audit** : `check_plugin_deps` (dépendances des jars), `world_info` (`level.dat`,
  dimensions), `audit_server` (configuration et sécurité)
- **Journaux et historique** : `error_digest` (ce qui plante, par signature et plugin probable),
  `search_logs` (journaux archivés compris), `uptime_report` (disponibilité, coupures),
  `list_incidents` (crashs, surcharges et diagnostics du Copilote)
- **Agir puis vérifier** : `tail_console` (console en direct par curseur), `run_command` (commande
  et sa réponse), `wait_for` (état, motif de console ou tâche), `deploy_and_verify` (jar local
  déployé, redémarrage, verdict d'activation)
- **Construire et reproduire** : `get_config` / `set_config` (configs YAML, TOML, JSON, properties
  sans casser la mise en forme), `export_manifest` → `plan_manifest` → `apply_manifest` (serveur
  déclaratif, idempotent ; étapes manuelles reliées aux outils officiels `change_version`,
  `manage_java`, `set_startup_config`), `clone_server` (copie de test d'un serveur)
- **Monter un serveur de A à Z** : `pregenerate_world` (prégénération Chunky : installation,
  estimation du disque, lancement, progression, pause, reprise, annulation), `connect_database`
  (base de la MyBox branchée à un plugin sans exposer son mot de passe)
- **Réseaux proxy** : `network_topology` (proxys Velocity / BungeeCord → serveurs du compte,
  cohérence et problèmes, secrets comparés sans être montrés) → `link_proxy` (relier des serveurs
  à un proxy, secret réutilisé ou généré sans jamais apparaître, dry_run par défaut)
- **Garde des agents** : `list_agent_actions` (journal), `list_changesets` → `rollback_changeset`
  (annuler ce qu'un agent a modifié)

## 6. Contrôle & sécurité (réglages)

Réglages modifiables depuis le GUI (Réglages → MCP) et persistés (`mcp.json`), lus par le serveur
(le troisième, `official_gateway`, active la passerelle décrite plus bas) :

- **`enabled`** — si désactivé : `tools/list` renvoie une liste vide et tout appel est refusé
  (prompts et ressources aussi).
- **`allow_writes`** — si désactivé (**mode lecture seule**) : les 42 outils marqués `✎`
  (`WRITE_TOOLS` : commandes, alimentation, fichiers, réglages, contenu, snapshots, infrastructure
  MyBox, tâches planifiées, manifeste appliqué, clone, annulation…) sont refusés ; les outils de
  lecture/diagnostic restent disponibles (dont `plan_placement` et `plan_manifest`, qui ne
  modifient rien). Idéal pour donner un accès *observation* sans risque.

**Garde-fous by design :**

- **Default-deny** : tout outil qui n'est **pas** dans `READ_TOOLS` est traité comme modifiant. Un
  nouvel outil est donc gaté par défaut (impossible de l'exposer en écriture par oubli).
- **Liste blanche d'env** (`MINESTRATOR_MCP_ALLOWED_TOOLS`) : le lanceur peut restreindre le sous-ensemble
  d'outils exposés (utilisée quand le Copilote lance un agent CLI en mode lecture).
- **Actions destructives volontairement absentes** du catalogue local : `restore_snapshot`,
  `delete_snapshot`, `restore_backup`, `repair_region` ne sont **pas** des outils MCP. Elles
  n'existent que dans l'app, derrière un « Appliquer » (intention explicite de l'utilisateur). Seul
  `create_snapshot` (additif) est exposé. Seule la passerelle (ci-dessous, désactivée par défaut)
  relaie les restaurations et suppressions de l'officiel, et uniquement avec une confirmation
  `confirm_name` vérifiée localement.
- **Suppression d'un serveur gardée** : `mybox_delete_server` exige le nom exact du serveur
  (`confirm_name`, vérifié avant tout appel ; retapé par l'utilisateur lui-même si son client
  gère l'élicitation, §2) et fait par défaut une snapshot préalable. Les
  écritures MyBox sont validées localement avant envoi (ressources, emplacements, champs requis) :
  une saisie incohérente est refusée sans toucher à l'API.

> **Portée de ce document** : il décrit le serveur MCP **local** exposé aux clients externes (Claude
> Desktop, Claude Code…). L'usage inverse — le Copilote interne comme *client* du **MCP officiel
> MineStrator** (délégation de la gestion, ne gardant en local que `LOCAL_KEEP_TOOLS`) — est décrit
> dans [`COPILOT.md`](./COPILOT.md).

### Passerelle vers le MCP officiel MineStrator

Le serveur local peut **relayer** les outils du MCP officiel (`https://mcp.sttr.io/minestrator`,
107 outils en 10 toolsets) : le client n'a qu'**une** connexion, et la clé API reste **au
trousseau** (compte actif, `secrets::read_key()`) au lieu d'être collée en clair dans sa config
MCP. Politique dans `crates/minestrator-core/src/official_relay.rs`.

- **Activation** : réglage `official_gateway` (Réglages → MCP, persisté dans `mcp.json`),
  **désactivé par défaut** ; la variable `MINESTRATOR_MCP_OFFICIAL` (`1`/`0`, `true`/`false`,
  `on`/`off`) prime, pour le binaire headless. Désactivé par défaut parce qu'il ajoute une
  soixantaine d'outils au contexte de chaque client (coût en jetons), une dépendance réseau au
  `tools/list`, des actions au niveau du compte (tickets, hébergements web, bases, offres) que
  l'utilisateur n'a pas demandées en installant le MCP de l'app — et qu'il ferait doublon avec un
  MCP officiel déjà configuré à côté.
- **Toolsets** : tous **sauf `files`**, que notre SFTP remplace — webhosting compris.
- **Sans doublon** : la table d'équivalences (`EQUIVALENTS`, partagée avec le Copilote) masque
  chaque outil officiel dont nous avons l'équivalent (`list_servers`, `get_server_live` →
  `server_status`, `send_console_command` → `send_command`, `manage_player` → `player_action`,
  MyBox → nos outils d'architecte, `delete_server_from_mybox` → `mybox_delete_server`, schedules →
  nos outils ci-dessus, `install_mod`/`install_plugin` → `install_mod`, paramètres, démarrage,
  snapshots, backups, disque, historique, docteur…) ; pour Spark, c'est notre
  `parse_spark_report` qui s'efface devant `get_spark_report`. Un officiel portant le nom d'un de
  nos outils est toujours masqué. Appeler un outil masqué renvoie l'outil local qui le remplace.
- **Gating** : un outil relayé est en **lecture** s'il figure dans la liste `readonly=1` de
  l'officiel (il est alors appelé sur une connexion `readonly=1`) ; tout autre outil, inconnu
  compris, est **modifiant** → soumis à `allow_writes` et à la liste blanche d'env. Après une
  écriture, le cache de lecture local est purgé.
- **Destructifs** : un outil modifiant dont l'annotation `destructiveHint` n'est pas
  explicitement `false` (défaut de la spécification MCP) n'est relayé qu'avec un argument
  **`confirm_name` obligatoire**, ajouté à son schéma et **vérifié localement avant relais** contre
  le nom exact de la ressource, lu à la source : serveur (`reinstall_server`, `change_version`,
  `install_modpack`, `restore_smartbackup`, `restore_snapshot` — la cible écrasée), snapshot
  (`delete_snapshot`), MyBox (`restore_db_backup`), site web (son domaine, via `get_webhosting` :
  `reset_webhosting`, `delete_webhosting`, `restore_webhosting_backup`, suppression de fichiers,
  extraction, changement de domaine, boîtes mail, installation de CMS). Le nom attendu n'est
  jamais révélé dans l'erreur. Client capable d'élicitation : c'est l'utilisateur qui retape ce
  nom dans un formulaire de son client (refus = rien n'est relayé, §2) ; `confirm_name` y devient
  facultatif. Un destructif sans ressource vérifiable est masqué ; un destructif
  qui a un équivalent local gardé l'est aussi (`delete_server_from_mybox`, `delete_schedule`).
- **Sorties** : anonymisées comme celles de nos outils (`redact_ai`) ; `reveal_credentials`
  (mots de passe en clair) est retiré des schémas et des appels.
- **Cache** : catalogue gardé 10 min par processus (empreinte de la clé : un changement de compte
  le renouvelle). Officiel injoignable ou sans clé : nos outils seuls, avertissement au journal,
  nouvel essai après une minute — jamais de plantage.
- **Serveur lancé par le Copilote** (`MINESTRATOR_MCP_COPILOT_SERVER=<id>`) : pas de passerelle,
  mais un relais restreint à la politique du Copilote (lecture + liste blanche, voir
  [`COPILOT.md`](./COPILOT.md) §4) ; tout appel, local ou relayé, visant un autre serveur est refusé.

### Groupes d'outils (toolsets)

Chaque outil local appartient à un **groupe** (`TOOL_GROUPS` dans `mcp_catalog.rs`) ; un client
peut n'exposer que les groupes utiles, pour alléger le contexte du modèle. Un outil d'un groupe
inactif n'apparaît pas dans `tools/list` et son appel est refusé (ressources comprises).

| Groupe | Contenu |
|---|---|
| `core` | `list_servers`, `server_status` — **toujours actif** (identifiants des serveurs). |
| `console` | `power_action`, `send_command`, `read_console` |
| `files` | `list_files`, `read_file`, `read_gz`, `list_archive`, `read_archive_entry`, `search_files`, `disk_usage`, `download_file`, `file_stat`, `write_file`, `upload_file`, `edit_file`, `create_dir`, `delete_path`, `rename_path`, `extract_archive`, `compress_files` |
| `config` | `read_startup`, `set_startup_params`, `read_properties`, `set_property` |
| `content` | `list_installed_mods`, `list_installed_plugins`, `market_search`, `list_mod_versions`, `install_mod`, `check_updates`, `update_mods`, `check_plugin_deps`, `ficsit_search`, `ficsit_versions`, `install_ficsit_mod` |
| `players` | `list_players`, `player_action`, `find_player` |
| `backups` | `list_backups`, `list_snapshots`, `create_snapshot` |
| `diagnostics` | `server_metrics`, `analyze_performance`, `parse_spark_report`, `diagnose_startup`, `inspect_region`, `world_info`, `audit_server`, `search_logs`, `error_digest`, `uptime_report`, `list_incidents` |
| `mybox` | `mybox_info`, `mybox_capacity`, `server_catalog`, `plan_placement`, `mybox_smartscan`, `mybox_create_server`, `mybox_resize_server`, `rename_server`, `rename_mybox`, `mybox_pause_server`, `mybox_reorder_servers`, `mybox_delete_server` |
| `schedules` | `list_schedules`, `create_schedule`, `update_schedule`, `delete_schedule`, `add_schedule_task`, `update_schedule_task`, `delete_schedule_task`, `reorder_schedule_tasks` |
| `fleet` | `copy_between_servers`, `diff_files`, `broadcast_command` |
| `agent` | Boucle « agir puis vérifier » (`run_command`, `wait_for`, `tail_console`, `deploy_and_verify`…). |
| `build` | Configurations structurées, manifestes et réseaux proxy (`get_config`, `set_config`, `export_manifest`, `plan_manifest`, `apply_manifest`, `clone_server`, `network_topology`, `link_proxy`…), prégénération et base de données (`pregenerate_world`, `connect_database`). |
| `guard` | Garde des agents (`list_agent_actions`, `rollback_changeset`…). |
| `official` | Outils du MCP officiel relayés par la passerelle (si elle est activée). |

**Sélection**, du plus prioritaire au moins prioritaire (la première non vide l'emporte) :

1. argument `--toolsets=files,console` (ou `--toolsets files,console`), sur `minestrator-mcp` comme
   sur `<app> --mcp` ;
2. variable `MINESTRATOR_MCP_TOOLSETS=files,console` ;
3. réglage `toolsets` de `mcp.json` (`McpConfig::toolsets`, liste vide par défaut).

Vide ou `all` = tous les groupes. Un nom inconnu est ignoré (avertissement sur stderr) ; si aucun
nom n'est reconnu, tout reste exposé. Un serveur lancé par le Copilote (liste blanche
`MINESTRATOR_MCP_ALLOWED_TOOLS`) ignore les groupes : sa liste blanche prime, inchangée.

Ajouter un outil : le classer dans `TOOL_GROUPS` (un test exige que chaque outil de `READ_TOOLS`
∪ `WRITE_TOOLS` y figure une fois), et, s'il est modifiant, le ranger au besoin dans
`NON_DESTRUCTIVE_TOOLS` / `IDEMPOTENT_TOOLS` (annotations ; destructif et non idempotent par
défaut).
### Garde des agents : journal, annulation, portée des écritures

Réglages dans Réglages → MCP (persistés dans `mcp.json`) ; code dans
`crates/minestrator-core/src/agent_guard.rs`.

**Journal des actions.** Chaque appel reçu par le protocole (clients externes et agents CLI du
Copilote) est consigné dans `<données>/agent_guard.db` (SQLite, WAL, partagé par l'app et les
processus `minestrator-mcp` / `--mcp`) : horodatage, session, client (`clientInfo.name`), outil,
serveurs visés, arguments **anonymisés** (`redact.rs` — commandes console comprises —, chaînes
tronquées, contenus de fichiers remplacés par leur taille, valeurs des clés secrètes et
`confirm_name` masqués), résultat et durée. Lectures **et** écritures sont journalisées ; les
lectures, plus courtes, sont gardées 7 jours, les écritures 30 jours, 20 000 lignes au plus. Les
refus de la politique générale, qui précèdent la garde dans `call()` (outil inconnu, lecture seule,
liste blanche d'env, portée du Copilote), ne sont pas journalisés ; les appels relayés vers le MCP
officiel non plus, sauf leurs refus en mode constructeur.

**Annulation (changesets).** Avant chaque outil modifiant passé par `mcp::dispatch` — clients MCP
**et** Copilote (diagnostic autonome, chat, « Appliquer ») — la garde copie de quoi revenir en
arrière, puis constate l'état après l'action et enregistre une étape avec ses opérations inverses.
Un changeset = une session MCP (par processus, et par connexion quand le transport l'identifie), ou
`copilote:<serveur>:<jour>` pour le Copilote. Une action en échec sans effet constaté n'ajoute pas
d'étape.

| Outil | Ce qui est défait |
|---|---|
| `write_file`, `edit_file`, `upload_file` | contenu précédent remis, ou fichier retiré s'il n'existait pas ; dossiers parents créés par l'écriture retirés s'ils sont vides |
| `set_config` | idem ; si le fichier a été **réécrit depuis** (Paper réécrit sa configuration à chaque démarrage), seules les clés de l'action sont remises à leur ancienne valeur, à condition qu'elles aient encore la valeur écrite (empreinte sha256 de chaque valeur, jamais la valeur) et qu'elles aient existé avant. Une simulation (`dry_run`) n'ajoute pas d'étape |
| `delete_path` | fichier recréé avec son contenu ; dossier recréé en entier s'il compte **500 entrées au plus** |
| `rename_path` | renommage inverse, et fichier écrasé à la destination remis |
| `create_dir` | dossier retiré s'il est vide (conservé sinon, sauf `force`) |
| `extract_archive`, `compress_files`, `copy_between_servers` (cible) | fichiers et dossiers créés retirés (listing avant/après, 5 000 entrées au plus) ; fichiers existants **remplacés** : signalés (annulation partielle), non restaurés |
| `install_mod` | jar ajouté dans `/mods` ou `/plugins` retiré |
| `update_mods` | nouveaux jars retirés, anciens remis (mods et plugins ; datapacks non couverts) |
| `set_property` | ancienne valeur remise — jamais pour un réglage **secret**, dont la valeur n'est pas conservée |
| `set_startup_params` | ancienne commande de démarrage remise |

Non annulables par nature (étape enregistrée, puis passée à l'annulation) : `send_command`,
`broadcast_command`, `power_action`, `player_action`, `create_snapshot`, `install_ficsit_mod`,
infrastructure MyBox, tâches planifiées, restaurations. `rollback_changeset` n'ajoute pas d'étape.

**Plafonds** : 16 Mio par fichier copié, 32 Mio par étape, 64 Mio par changeset, 256 Mio pour toute
la base (au-delà, les copies des changesets les plus anciens sont effacées et leurs étapes deviennent
non annulables) ; copie et constat limités à 2 minutes chacun. **Rétention** : 14 jours après la
dernière étape d'un changeset ; les copies d'une étape annulée sont effacées aussitôt. Si la copie
échoue ou dépasse un plafond, l'écriture a **quand même lieu** : l'étape est « non annulable » et la
sortie de l'outil le dit (`(Garde des agents : modification NON annulable — …)`) ; une annulation
partielle est signalée de même. Les copies sont des fichiers du serveur de l'utilisateur, gardés sur
cet ordinateur dans le dossier de données : pas des secrets de l'app (clé API et mots de passe restent
dans le stockage sécurisé), mais une config peut en contenir ; elles ne partent jamais vers un LLM et
disparaissent avec leur changeset.

**Mode constructeur** (`write_scope`). `all` (défaut) : comportement historique. `builder` : un outil
modifiant (infrastructure et écritures relayées par la passerelle comprises) n'est permis que sur les
serveurs **créés pendant la session MCP** (relevés après un `mybox_create_server` réussi) ou listés
dans `allowed_servers` ; lectures partout ; créer un serveur reste permis. Copie entre serveurs :
seule la cible compte. Outil qui touche une MyBox entière (`rename_mybox`, `mybox_reorder_servers`,
outil relayé qui ne désigne qu'une `mybox_id`) : permis seulement si **tous** ses serveurs le sont.
`broadcast_command` avec `all_running=true`, outil sans serveur identifiable ou identifiant illisible :
refusés. `rollback_changeset` : permis si tous les serveurs du changeset le sont. Refus explicite,
journalisé (`refusé`). Surcharges : `MINESTRATOR_MCP_WRITE_SCOPE` (`all` ou `builder` ; valeur
illisible = `builder`) et `MINESTRATOR_MCP_ALLOWED_SERVERS` (identifiants séparés par des virgules,
remplace la liste des réglages). Les sessions du Copilote n'y sont pas soumises : elles sont déjà
bornées au serveur de la conversation. Comme les autres réglages de `mcp.json`, la portée est lue au
démarrage du processus MCP : un changement dans l'app vaut pour les clients MCP relancés ensuite.

**Brancher un outil d'un autre module** : une arme dans `agent_guard::undo::plan_for` —
`Plan::Files` (fichiers modifiés, par chemin), `Plan::NewFiles` (fichiers créés dans un dossier),
`Plan::Skip` (outil composite dont chaque sous-action passe par `mcp::dispatch`, qui obtient ainsi
une étape par sous-action), `Plan::Fixed("raison")` (effet non réversible). Un test refuse un outil
de `WRITE_TOOLS` sans arme. Un outil qui crée un serveur appelle
`agent_guard::record_created_server_here` après succès et reçoit la règle `Targets::Create` dans
`agent_guard::scope::write_targets`.

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
  **crée** (`create_snapshot`), mais la **restauration** n'est pas un outil local (voir §6) ; seule
  la passerelle vers le MCP officiel la relaie, avec confirmation vérifiée.
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

- **Ressources** : abonnement (`subscriptions/listen`) à la console d'un serveur.
- **Automatisation** : planificateur (tâches programmées), règles (event→action).
- **Sauvegardes** : exposer une restauration *gardée* (confirmation forte) si l'API le permet.

> Déjà livré depuis la première version de ce document : inventaire et **installation** de mods/plugins
> (Modrinth/CurseForge/SpigotMC), **snapshots** (liste + création), analyse **Spark**, **Docteur
> démarrage** et inspection des **régions corrompues** — tous adossés à `minestrator-core` ; puis
> **prompts** et **ressources** MCP, élicitation, annotations et groupes d'outils (§2, §6).

---

*Implémentation : logique dans `crates/minestrator-core/src/mcp.rs` (transport-agnostique) ;
transports = binaire `minestrator-mcp` (stdio) et mode `--mcp` de l'app desktop.*
