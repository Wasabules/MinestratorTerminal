# Copilote — diagnostic & administration IA multi-LLM

Le **Copilote** est un agent IA qui **diagnostique automatiquement les incidents** de tes serveurs
Minecraft (crash, surcharge CPU/RAM/disque, erreurs console, échec de démarrage, lag) et **propose
des correctifs** — voire les applique. Il est **indépendant du fournisseur LLM** : Claude, GPT,
Gemini, Mistral, un modèle local, ou un **agent CLI** (Claude Code / OpenCode / Gemini CLI).

Il vit dans `minestrator-core` (logique métier réutilisable : desktop, futur daemon, CLI).

---

## 1. Pourquoi (et pourquoi pas juste MCP)

Un serveur MCP est *tiré* : un client (Claude Desktop…) l'interroge **quand tu le sollicites**.
Il ne se réveille pas tout seul quand un serveur crashe. Le Copilote comble ce manque :
c'est la boucle **autonome** *poussée* par le superviseur, hébergée dans l'app.

```
Superviseur              Copilote                        Couche d'outils
crash / seuil / erreur → agent LLM (lecture seule)   →  MCP LOCAL (SFTP, .mca, Spark, docteur)
console / perf / clic    diagnostique + corrèle       +  MCP OFFICIEL (gestion : power, backups…)
émet une Alert                  │
                                ▼
                Rapport (🩺) : cause + correctif + 0-3 actions (safe/caution/danger)
```

## 2. Multi-fournisseur (LLM)

Trois fournisseurs, deux transports (`crates/minestrator-core/src/{llm,cli}.rs`) :

| Fournisseur | Couvre | Réglage |
|---|---|---|
| **Anthropic** (API Messages native) | Claude | clé API (obligatoire) |
| **OpenAI-compatible** (Chat Completions) | **OpenAI (GPT)**, **Google Gemini**, **Mistral**, **Groq**, **xAI**, **DeepSeek**, **OpenRouter**, **local** (Ollama, LM Studio, vLLM) | URL de base + modèle (+ clé si requise) |
| **CLI locale** (sous-processus) | **Claude Code**, **OpenCode**, **Gemini CLI** | agent + commande + args — **aucune clé API** |

Pour les deux transports HTTP, l'agent parle un **format normalisé** (`ToolSpec`/`Msg`/`ToolCall`/
`ToolResult`) ; chaque adaptateur (dé)sérialise vers le format natif. Ajouter un fournisseur HTTP =
ajouter un variant + son (dé)sérialiseur, sans toucher à l'agent. Préréglages UI pour l'OpenAI-
compatible (OpenAI, Gemini, Mistral, Groq, Ollama : remplissent URL + modèle). Modèle par défaut :
`claude-sonnet-5` (Anthropic).

## 3. Agents CLI locaux — sans clé API (ton abonnement)

Si un **agent CLI** est installé et connecté, le Copilote le lance en **sous-processus** : il utilise
**ton abonnement / ton auth locale**, donc **aucune clé API**. Trois agents sont **pris en charge en
propre** (`enum CliAgent { ClaudeCode, OpenCode, Gemini }`, `cli_agent.rs`) — chacun a ses **flags**,
son **format de config MCP** et son **parsing de sortie** dédiés, isolés pour que `copilot.rs` reste
agnostique :

| Agent | Binaire | Format de modèle | Config MCP |
|---|---|---|---|
| **Claude Code** (défaut) | `claude` | `opus` · `sonnet` · `claude-opus-4-8…` | `--mcp-config mcp.json` |
| **OpenCode** | `opencode` | `anthropic/claude-sonnet-4-5` | `opencode.json` (CWD) |
| **Gemini CLI** | `gemini` | `gemini-2.5-flash` | `.gemini/settings.json` (CWD) |

- **Détection automatique** : `detect_clis()` sonde les trois binaires en parallèle
  (`probe --version`, timeout court) et renvoie un `CliStatus { agent, command, available, version }`.
  Les Réglages affichent un **sélecteur segmenté** des trois agents + un indicateur « ✓ détecté ·
  <version> » ou « ⚠ absent », avec bouton re-scan. Le binaire reste **surchargeable** (champ
  commande).
- **⚠ Avertissement sécurité** (en-tête de `cli_agent.rs`) : notre liste blanche d'outils ne borne
  que **notre serveur MCP**, pas les outils **natifs** de l'agent (shell, lecture/écriture fichier
  local, web). Claude Code restreint ses outils natifs via `--allowedTools` → **non concerné**. Mais
  **OpenCode** (`--auto`) et **Gemini** (`--approval-mode yolo`) sont lancés en **auto-approbation
  totale** de leurs outils natifs : un contenu injecté qu'ils lisent (console d'un joueur, crash-
  report, config) pourrait leur faire exécuter une commande shell sur la machine de l'admin. **En
  attendant un durcissement de leur config, préférer Claude Code.**

### Deux modes d'exécution

- **Mode agent** (`cli_agentic`, défaut) : on **branche notre serveur MCP** sur l'agent (notre
  binaire lancé en `--mcp`, liste blanche d'outils dans son env). L'agent lit lui-même console/
  métriques/fichiers, **croise**, puis rend le rapport. Les outils modifiants ne sont **pas** pré-
  autorisés en diagnostic → il propose sans exécuter. Sortie suivie en direct
  (`--output-format stream-json --verbose`, phases + streaming du texte).
- **Un coup** : le Copilote pré-collecte le contexte (console + métriques) dans un seul prompt et
  attend un JSON. Pour les CLI non-Claude ou si tu préfères.

### Session CLI persistante (chat)

Pour le chat, Claude Code est maintenu **vivant** entre les tours (`cli_session.rs`,
`PersistentCli`) : un seul process `claude` consomme les messages successifs via
`--input-format stream-json`, ce qui élimine, par message, le bootstrap Node + le respawn du serveur
MCP + la relecture `--resume`. Le process est **pré-chauffé** avant le 1er message et **tué**
(`kill_on_drop`) à la fermeture de l'onglet. En cas d'échec (2 fois de suite), on **retombe** sur le
one-shot éprouvé, qui enchaîne les tours via `--resume <session>` — l'utilisateur obtient toujours
une réponse.

### Robustesse CLI (`cli.rs`)

Toutes les issues sont couvertes et remontées lisiblement dans le 🩺 — binaire introuvable, échec
d'écriture, **code de sortie non nul** (avec extrait stderr), **timeout** (le process est alors
**tué et récupéré**, pas de fuite ; dernières lignes remontées), **sortie vide**. stdout/stderr sont
lus **en parallèle** (aucun interblocage de tube), sortie plafonnée (1 Mio). Sous **Windows**, une
commande « nue » (`claude`) est lancée via `cmd /c` pour résoudre les shims `.cmd`/`.bat` (npm) et le
PATH ; le flag `CREATE_NO_WINDOW` évite qu'une fenêtre de console surgisse (app GUI).

> **Claude Desktop** n'expose aucune interface programmable → il ne peut pas piloter le Copilote
> autonome. Il s'utilise en interactif comme client MCP (voir `MCP.md`).

## 4. MCP officiel MineStrator (gestion déléguée)

Réglage `use_official_mcp` (**activé par défaut**). L'hébergeur expose ~**60 outils de gestion**
(power, console, contenu, backups, MyBox, schedules, bases de données, config Java/ports) via un
serveur **MCP officiel** (`official_mcp.rs`, `https://mcp.sttr.io/minestrator`, transport Streamable
HTTP **stateless**, JSON-RPC POST authentifié `Bearer <clé API MineStrator>` — la même clé qu'au
trousseau). On **délègue la gestion** à ce serveur (maintenu par l'hôte, donc plus fiable) et on
garde **NOTRE MCP** pour le **SFTP fin + les outils exclusifs**.

- **Toolsets délégués** : `core,actions,content,backups,mybox,schedules,databases,config` — **tout
  sauf `files`** (on gère les fichiers en SFTP direct) → aucune collision de noms.
- **`merge_tools`** : quand l'officiel répond, notre catalogue local est **recentré** sur
  `LOCAL_KEEP_TOOLS` (~**15 outils** : SFTP, lecture d'archives/`.gz`, `.mca`, Spark, docteur) +
  `report_diagnosis` → **pas de doublon** pour l'IA. Le routing d'appel distingue outils officiels
  vs locaux.
- **Variante `readonly`** (`&readonly=1`) : en diagnostic auto et en chat non-autonome, les outils
  **modifiants n'existent pas** pour la connexion → impossible d'agir par mégarde.
- **Branché sur les deux voies** : les 3 agents CLI (serveur `minestrator_official` ajouté à leur
  config MCP) **et** la voie API HTTP (`list_tools`/`call_tool`, catalogue mis en cache par session).
  Repli automatique sur le **catalogue local complet** si l'officiel est désactivé ou injoignable.
- **Toggle** dans les Réglages (résultats d'outils **redactés** avant de repartir vers le LLM).

## 5. Ce que fait l'agent (outils)

En diagnostic, l'agent ne dispose que d'outils de **lecture** (jamais d'action modifiante directe).
Ce ne sont plus « 6 » outils : il y a **21 `READ_TOOLS`** (source unique `mcp.rs`, voir `MCP.md`) —
console, métriques, statut, SFTP (`list_files`/`read_file`/`read_gz`/archives/`search_files`),
`read_startup`, mods/plugins installés, backups/snapshots, marketplace (`market_search`/
`list_mod_versions`), `analyze_performance`, `parse_spark_report`, `diagnose_startup`,
`inspect_region`. Il croise ces sources, puis livre un **rapport structuré** via `report_diagnosis` :
résumé · cause probable justifiée · correctif détaillé · **0 à 3 actions** proposées, chacune classée
`safe` / `caution` / `danger`.

- **Actions** = les `WRITE_TOOLS` (power, commande, joueur, fichiers, `set_startup_params`,
  `install_mod`, `create_snapshot`…). Le prompt injecte les **arguments EXACTS** par outil
  (`ACTION_TOOLS_SPEC`) et un normaliseur (`prepare_action_args`) résout les synonymes + force le
  `server_id` — filet contre les hallucinations de paramètres. Les outils **destructifs**
  (`restore_snapshot`/`restore_backup`/`delete_snapshot`) sont volontairement **absents** de la liste
  auto-générée : jamais auto-exécutés, seulement via « Appliquer ».
- **Permissions fines par outil** : `disabled_tools` (noms MCP) interdit un outil à l'IA quelle que
  soit la source (HTTP **ou** CLI) — appliqué à la liste blanche du serveur MCP et au catalogue LLM.
  Les Réglages exposent un **catalogue « aiTools »** (~20 outils cochables, badge « écriture »).

## 6. Déclencheurs

- **Crash** : arrêt inattendu détecté par le superviseur (défaut : activé).
- **Seuil** : dépassement CPU/RAM/disque (désactivé par défaut).
- **Erreur / Avertissement console** : le superviseur classe en continu les lignes des serveurs
  actifs (connexion *monitor*) ; sur une ligne **ERROR** (ou **WARN**), un diagnostic peut partir.
  Réglables séparément, **désactivés par défaut** (WARN est bruyant). Marche en fond, sans ouvrir
  d'onglet console.
- **Sélection (clic droit)** : sélectionne du texte dans la **console** ou l'**éditeur de fichier**,
  clic droit → **🩺 Copilote** → analyse de l'extrait.
- **Performance (Spark)** : bouton **« Analyser les performances »** et/ou **auto sur surcharge
  prolongée** (voir §10).
- **Manuel** : bouton « Diagnostiquer » (Réglages → Copilote → Tester).

**Cooldowns séparés, par serveur** : incidents (crash/seuil) et logs (error/warn) ont chacun leur
compteur à **5 min** (`DIAGNOSE_COOLDOWN_S`) pour ne pas se priver mutuellement ; les analyses de
**performance** ont un cooldown **dédié de 30 min** (`PERF_COOLDOWN_S`) — une surcharge peut durer
des heures, inutile de rejouer (et payer) une analyse Spark toutes les 5 min. Un sémaphore plafonne
à **3** les diagnostics concurrents déclenchés par le superviseur. Les déclencheurs **manuels**
(Tester, clic droit) **ignorent le gate « activé »** — ils marchent même Copilote en veille.

## 7. Niveaux d'autonomie (réglables)

| Niveau | Comportement |
|---|---|
| **Suggérer seulement** (défaut) | N'exécute jamais. Propose ; tu appliques d'un clic. |
| **Appliquer sur validation** | Boutons « Appliquer » sur chaque action, exécutés après confirmation. |
| **Auto-correctifs sûrs** | Applique **seul** les actions `safe` ; le reste attend validation. |

En mode **Auto-correctifs sûrs**, l'auto-exécution est limitée à une **liste blanche codée en dur**
(`is_auto_safe`) — jamais sur la seule foi du `risk` du modèle (risque de mauvais étiquetage /
injection) : **`create_snapshot`** et **`power_action` `start`/`restart`/`restart10`** uniquement.
Dans tous les cas, l'exécution passe par la couche d'outils (mêmes garde-fous de normalisation).

## 8. Effort de raisonnement & recherche web

- **Effort** (`Low` / `Medium` par défaut / `High`) : mappé par fournisseur — flag natif
  `--effort low|medium|high` côté **Claude Code**, **indice de prompt** ajouté au système côté **API
  HTTP** (pas de contrôle natif universel). Sélecteur segmenté dans les Réglages.
- **Recherche web** (`web_search`, **activée par défaut**) : côté **Claude Code**, autorise les
  outils natifs `WebSearch`/`WebFetch` (utile pour « les meilleurs plugins PvP 2026… »). **Sans
  effet** sur la voie **API HTTP** (aucun outil web) ; OpenCode/Gemini s'appuient sur leurs propres
  outils web natifs (auto-approuvés, cf. §3).

## 9. Sécurité

- **Diagnostic en lecture seule** : l'agent ne peut pas modifier ni supprimer par lui-même ; il
  *propose*. Seul le **chat en mode Autonome** (opt-in, §15) exécute directement.
- **Redaction avant envoi IA** (`core.redact_ai()`, `redact.rs`) : **systématiquement** appliquée à
  tout ce qui part vers un LLM ou un client MCP — console (diagnostic & chat), contexte **Spark**
  (perf), rapport du **docteur** démarrage, résultats d'outils du **MCP officiel**. Masque, **sans
  regex** : mots de passe des commandes d'auth (`/login`, `/register`, `/authme`…), secrets de config
  (`rcon.password=`, `token:`…), identifiants d'URL (`user:pass@host` → `[CREDS]`), **IPv4** → `[IP]`,
  **e-mails** → `[EMAIL]`. Réglages **Confidentialité** : `redact_ai` (**activé** par défaut),
  `redact_console` (masque aussi l'affichage console, désactivé par défaut).
- **Clés API dans le trousseau OS** (`llm-key-<fournisseur>`), **une par fournisseur** — jamais en
  clair, jamais transmises ailleurs qu'au fournisseur choisi. Seul Anthropic exige une clé.
- **Désactivé par défaut** : rien ne part vers un LLM tant que tu ne l'as pas activé et configuré.
- **Par serveur** : exclus n'importe quel serveur du diagnostic.
- **Liste blanche d'outils** imposée au serveur MCP par variable d'env (honorée quel que soit
  l'agent), + `readonly` sur le MCP officiel en lecture seule, + permissions fines `disabled_tools`.

## 10. Analyse de performance (Spark)

Diagnostic **performance** assisté par le profileur **Spark** (`perf.rs`). Le cœur **orchestre**
(pas l'agent, pour rester sûr et déterministe) :

1. **Collecte + détection** : envoie `spark health` / `spark tps` / `spark gc`, attend, **lit la
   console**. La détection se fait **par la réponse console** (pas par le jar) → gère Spark **plugin**
   OU **intégré** (Paper/Purpur/Folia récents). Si « unknown command » sans marqueur Spark → note
   d'aide.
2. **Profiler** : `spark profiler start` → **30 s** → `stop` → extrait l'**URL** du rapport
   (`https://spark.lucko.me/…`).
3. **Points chauds** : télécharge le **rapport brut** (`spark-usercontent.lucko.me/<clé>`, protobuf
   `x-spark-sampler`), le **parse** (`prost`, structs à la main), calcule le **temps propre par
   méthode** → **top 15** (avec le plugin source si connu). L'analyse est *réellement* profonde
   (« FooMobs = 41 % du tick »).
4. **Analyse** : tout ce contexte (redacté) part au Copilote → il identifie la source du lag et
   **propose des correctifs** (flags JVM, config via `write_file`…).

Déclenchement : bouton **« Analyser les performances »** (serveur en ligne) ; et/ou **auto sur
surcharge PROLONGÉE** (`perf_on_overload`) — ne déclenche que si **CPU ou RAM restent ≥ seuil pendant
N minutes en continu** (réglables : `perf_overload_pct` / `perf_overload_minutes`, défaut **85 % /
3 min**), pas sur un pic, avec le cooldown dédié de 30 min (§6). Le résultat arrive dans **🩺** ;
étapes de progression visibles pendant la collecte (~35 s). L'outil `parse_spark_report` permet aussi
d'analyser une **URL Spark existante** (profiler OU heapsummary mémoire → plus gros consommateurs).

## 11. Docteur démarrage

Pour un serveur qui **ne démarre pas / crash-loop**, l'outil `diagnose_startup` (`doctor.rs`)
rassemble en **un appel** le contexte utile et **pré-scanne** les pannes connues :

- **Collecte** : commande de démarrage + fin de `/logs/latest.log` (repli console) + **dernier
  crash-report** de `/crash-reports` (le plus récent, tronqué).
- **Pré-scan de signatures** (déterministe) : **EULA** non acceptée, **port occupé**, **OOM**
  (heap/GC), **version de Java** incompatible, **dépendance de mod** manquante/incompatible, conflit
  de **mixin**, **monde/chunk corrompu**, échec générique de chargement de mod. Chaque signature
  renvoie un correctif ciblé (accepter l'EULA, `power_action kill`, désactiver un `.jar` en
  `.jar.disabled` via `rename_path`, `inspect_region`…).
- Rapport **redacté**, lisible tel quel ; l'agent traite les cas hors-signature.

## 12. Réparation des maps `.mca`

Boîte à outils « maps corrompues » (`world.rs`, via SFTP ; parsing pur testé dans `mca.rs`) :

- **`inspect_region`** (**lecture seule**, sûr) : télécharge un fichier de région, valide sa
  structure, liste les chunks corrompus avec leurs **coordonnées globales**.
- **`repair_region`** (**destructif** → action `danger`, snapshot recommandé d'abord) :
  - `clear_corrupt` : efface **uniquement** les chunks corrompus → régénération, perte minimale, le
    reste de la région préservé ;
  - `delete` : supprime tout le fichier → la zone (jusqu'à 512×512 blocs) est régénérée.

## 13. Réglages (GUI)

Réglages → **Copilote** : activer · **fournisseur** (Anthropic / OpenAI-compatible / CLI locale) ·
toggle **MCP officiel** · [CLI] **sélecteur d'agent** (Claude Code / OpenCode / Gemini) + détection +
commande + modèle + **mode agent** (+ args si un-coup) · [HTTP] préréglage + modèle + URL de base +
clé API · **niveau d'autonomie** · **effort** · **recherche web** · **permissions par outil**
(catalogue aiTools) · **déclencheurs** (crash / seuil / error / warn / perf sur surcharge + seuils
`%`/`min`) · **serveurs surveillés** · bouton **Tester**. Réglages → **Confidentialité** : `redact_ai`
/ `redact_console`. Les rapports apparaissent dans l'icône **🩺** (+ notification native).

## 14. UI Copilote (indicateur + onglet)

- **Icône 🩺** (barre supérieure) : vue minimaliste — analyses **en cours** avec **phase courante**,
  et derniers rapports en compact.
- **Onglet Copilote** (`CopilotView`) : page complète — **En cours** (progression + étapes) et
  **Historique** (rapports dépliables : cause, correctif, **actions avec boutons Appliquer**, log des
  étapes). Étapes émises via `CoreEvent::CopilotProgress { id, phase }`.
- **Suivi live de l'agent** : en mode agent CLI, la sortie `stream-json` est lue **ligne par ligne** —
  chaque appel d'outil / réflexion devient une étape en direct (« 📄 Lecture : plugins/config.yml »,
  « 📈 Analyse des métriques »…). Le mode API émet aussi une étape avant chaque outil. Meilleur
  diagnostic en cas de lenteur/timeout : on voit ce que fait l'agent.

## 15. Assistant conversationnel (chat)

Un **chat multi-tours par serveur** (onglet **Assistant** 💬) : questions en langage naturel
(« Optimise mon serveur pour plus de joueurs », « Mes plugins sont-ils traduits ? »). L'assistant
**investigue** via les outils de lecture (console, config, `/plugins`, métriques, `read_startup`,
marketplace, `analyze_performance`, `diagnose_startup`…) avant de répondre.

- **Deux modes** (interrupteur dans le chat) : **Suggéré** (défaut, propose des actions cliquables) ;
  **Autonome** (exécute directement, mêmes garde-fous de normalisation). Le changement de mode
  respawne le process persistant (le toolset MCP est figé au démarrage, parité de sécurité).
- **Multi-fournisseur** : HTTP (Anthropic/OpenAI) en multi-tours natif (transcript conservé, rollback
  transactionnel si un tour échoue) ; **Claude Code** en session **persistante** (§3), repli one-shot
  `--resume`.
- **Streaming** : texte token-par-token (Claude Code) + étapes (📄/📈…) pendant qu'il réfléchit.
- Une conversation **par onglet** (`session_id` = id d'onglet) ; bouton « Nouvelle conversation ».

Implémentation : `copilot::chat_turn` (`chat_http` / `chat_cli` → persistant/one-shot),
`ChatSession`/`ChatReply`, vue `AssistantView.svelte`.

## 16. Architecture

- `copilot.rs` — config, écoute des alertes, boucle agentique (HTTP & CLI), chat, rapport.
- `llm.rs` — couche multi-fournisseur HTTP (adaptateurs Anthropic / OpenAI-compatible).
- `cli.rs` / `cli_agent.rs` / `cli_session.rs` — exécution CLI, adaptateurs par agent, process
  persistant.
- `official_mcp.rs` — client du MCP officiel (gestion déléguée).
- `perf.rs` / `doctor.rs` / `world.rs` — Spark, docteur démarrage, réparation `.mca`.
- `redact.rs` — anonymisation avant IA/console.
- Réutilise `mcp::tool_list()` + `mcp::dispatch()` : **une seule** source de vérité pour les outils.
- Émet un `CoreEvent::Diagnosis` ; chaque frontend le relaie (desktop → 🩺 + notification).

## 17. Limites / évolutions

- La surveillance console (ERROR/WARN) dépend du **monitoring du superviseur** : superviseur
  désactivé → pas de déclenchement sur log.
- Classement des lignes = heuristique (`/ERROR]`, `SEVERE`, `FATAL`, `/WARN]`…), robuste pour les
  logs Java/Minecraft.
- Durcissement des outils **natifs** d'OpenCode/Gemini (auto-approbation) à faire — préférer Claude
  Code en attendant.
- Coût : chaque diagnostic HTTP est un appel LLM facturé par ton fournisseur (le CLI/local est
  gratuit via l'abonnement).

---

*Projet **Minestrator Terminal** v0.2.1 · GPL-3.0-or-later. Implémentation :
`crates/minestrator-core/src/{copilot,llm,cli,cli_agent,cli_session,official_mcp,perf,doctor,world,
redact}.rs`. Voir aussi [`MCP.md`](./MCP.md) pour la couche d'outils partagée.*
