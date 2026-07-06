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
- **Historique** : partage la base SQLite de métriques de l'app (mode WAL) → lecture concurrente sûre.
- **Anonymisation** : les sorties potentiellement sensibles (`read_file`, `read_console`,
  `analyze_performance`, `inspect_region`…) passent par le filtre de rédaction avant de partir vers
  l'agent (mots de passe d'auth, IPv4, e-mails, secrets `clé=valeur`, identifiants d'URL).

## 4. Deux façons de l'héberger (même logique `core::mcp`)

1. **App desktop en mode serveur** — `minestrator-terminal --mcp` : l'application *est* le serveur MCP
   (elle n'ouvre pas la fenêtre). App centralisante. Réglable depuis Réglages → MCP.
2. **Binaire autonome** — `minestrator-mcp` : léger, headless (idéal serveur/daemon).

## 5. Capacités — catalogue d'outils (30)

`✎` = action **modifiante** (soumise au réglage « autoriser les actions modifiantes » ; refusée en
mode lecture seule). Les entrées suffixées `?` sont **optionnelles**.

> **Source unique de vérité** : ce catalogue est dérivé de `mcp::tool_list()` (+ des constantes
> `READ_TOOLS` / `WRITE_TOOLS`) dans `crates/minestrator-core/src/mcp.rs`. **20 outils de lecture**
> + **10 modifiants**.

#### Découverte & état

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_servers` | | — | Serveurs (id, nom, statut, adresse, MyBox). **Point d'entrée** pour les `server_id`. |
| `server_status` | | `server_id` | Joueurs, version, MOTD, limites **+ consommation instantanée** (échantillon live CPU/RAM/disque). |
| `server_metrics` | | `server_id`, `since_secs?` | **Historique** CPU/RAM/disque + **résumé** (moyennes/max) sur la fenêtre (défaut 1 h, max 14 j). |

#### Contrôle & console

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `power_action` | ✎ | `server_id`, `action` | start / restart / restart10 / stop / stop10 / kill. |
| `send_command` | ✎ | `server_id`, `command` | Envoie une commande console (serveur en ligne). |
| `read_console` | | `server_id` | 100 dernières lignes de console (diagnostic de crash/erreur). |
| `player_action` | ✎ | `server_id`, `action`, `player` | Modération : kick / ban / unban / op_add / op_remove / whitelist_add / whitelist_remove. |

#### Fichiers & configuration (SFTP)

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_files` | | `server_id`, `path` | Contenu d'un répertoire (nom, chemin, dossier/fichier, taille). |
| `read_file` | | `server_id`, `path` | Contenu d'un fichier texte (refuse binaires / > 2 Mo). |
| `read_gz` | | `server_id`, `path` | Contenu d'un fichier texte **gzippé**, décompressé (ex. log tourné `latest.log.gz`) — pour diagnostiquer sur des logs archivés. |
| `list_archive` | | `server_id`, `path` | Entrées d'une archive `.zip`/`.tar`/`.tar.gz` **sans l'extraire** (repérer un fichier dans un backup/modpack). |
| `read_archive_entry` | | `server_id`, `path`, `entry` | Contenu **texte** d'une entrée d'archive, sans extraction disque. |
| `write_file` | ✎ | `server_id`, `path`, `content` | Écrit/écrase un fichier de config. |
| `create_dir` | ✎ | `server_id`, `path` | Crée un dossier. |
| `delete_path` | ✎ | `server_id`, `path`, `is_dir?` | Supprime un fichier ou un dossier. |
| `rename_path` | ✎ | `server_id`, `from`, `to` | Renomme ou déplace un fichier/dossier. |

#### Démarrage & JVM

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `read_startup` | | `server_id` | Commande de démarrage (flags JVM `-Xmx`/GC/Aikar…), JAR, mémoire, image. |
| `set_startup_params` | ✎ | `server_id`, `parameters` | Modifie la commande Java (optimiser les flags JVM). Garder `{{SERVER_JARFILE}}` ; effet au prochain démarrage. |

#### Mods & plugins (marketplace + installés)

| Outil | | Entrées | Renvoie |
|---|---|---|---|
| `list_installed_mods` | | `server_id`, `query?` | Mods installés (nom + version, compact ; `query` filtre par sous-chaîne). |
| `list_installed_plugins` | | `server_id`, `query?` | Plugins installés (nom + version, compact ; `query` filtre par sous-chaîne). |
| `market_search` | | `kind?`, `source?`, `query?`, `loader?`, `game_version?`, `page?` | Recherche Modrinth / CurseForge / SpigotMC (id/slug, nom, downloads, loaders, versions de jeu). |
| `list_mod_versions` | | `source?`, `slug`, `loader?`, `game_version?` | Versions d'un projet → les `version_id` requis pour installer. |
| `install_mod` | ✎ | `server_id`, `source?`, `kind?`, `slug`, `version_id`, `loader?` | Installe un mod/plugin. **Modrinth** = mods **et** plugins (`{slug, version_id}`) ; **Spigot** = plugins (`slug` = id numérique, `version_id` = id de version). Refuse le premium/external (403). |

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

### Couverture par domaine

- **Découverte & état** : `list_servers`, `server_status`
- **Consommation & optimisation** : `server_metrics` (historique + résumé), `server_status` (live),
  `analyze_performance` (Spark : TPS/MSPT/GC/points chauds), `parse_spark_report`
- **Contrôle** : `power_action`, `send_command`
- **Modération** : `player_action`
- **Fichiers / config** : `list_files`, `read_file`, `write_file`, `create_dir`, `delete_path`,
  `rename_path` (lister, lire, écrire, créer un dossier, supprimer, renommer/déplacer)
- **Logs & archives** : `read_gz` (logs gzippés tournés), `list_archive` + `read_archive_entry`
  (fouiller un `.zip`/`.tar.gz` sans extraire) — utile pour diagnostiquer sur des logs/backups archivés
- **Démarrage / JVM** : `read_startup`, `set_startup_params` (mémoire, GC, Aikar…)
- **Mods & plugins** : `market_search`, `list_mod_versions`, `install_mod`, `list_installed_mods`,
  `list_installed_plugins` (chercher → choisir une version → installer ; inventaire)
- **Sauvegardes / filet** : `list_backups`, `list_snapshots`, `create_snapshot`
- **Diagnostic de panne** : `diagnose_startup` (crash-loop), `inspect_region` (maps corrompues),
  `read_console` (+ `server_metrics` pour corréler)

## 6. Contrôle & sécurité (réglages)

Deux réglages, modifiables depuis le GUI (Réglages → MCP) et persistés (`mcp.json`), lus par le serveur :

- **`enabled`** — si désactivé : `tools/list` renvoie une liste vide et tout appel est refusé.
- **`allow_writes`** — si désactivé (**mode lecture seule**) : les outils marqués `✎`
  (`power_action`, `send_command`, `player_action`, `write_file`, `create_dir`, `delete_path`,
  `rename_path`, `set_startup_params`, `install_mod`, `create_snapshot`) sont refusés ; les outils de
  lecture/diagnostic restent disponibles. Idéal pour donner un accès *observation* sans risque.

**Garde-fous by design :**

- **Default-deny** : tout outil qui n'est **pas** dans `READ_TOOLS` est traité comme modifiant. Un
  nouvel outil est donc gaté par défaut (impossible de l'exposer en écriture par oubli).
- **Liste blanche d'env** (`MCP_ALLOWED_TOOLS`) : le lanceur peut restreindre le sous-ensemble
  d'outils exposés (utilisée quand le Copilote lance un agent CLI en mode lecture).
- **Actions destructives volontairement absentes** du catalogue : `restore_snapshot`,
  `delete_snapshot`, `restore_backup` ne sont **pas** des outils MCP → un agent ne peut jamais les
  déclencher. Elles n'existent que dans l'app, derrière un « Appliquer » (intention explicite de
  l'utilisateur). Seul `create_snapshot` (additif) est exposé.

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
- Pas d'endpoint de **listing** des bans/ops côté API — on les lit néanmoins via `read_file`
  (`banned-players.json` / `ops.json`) ; la modération (`player_action`) reste disponible.
- Stats **live** obtenues via une connexion WebSocket éphémère à la demande (~quelques secondes).
- L'historique de `server_metrics` n'existe que si l'app desktop (superviseur) a tourné.
- Le marketplace ignore les projets **premium/external** (non installables → `install_mod` renvoie 403).

## 9. Extensions prévues (scope futur)

- **`resources`** : exposer `server.properties`, logs, listes de fichiers comme ressources MCP
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
