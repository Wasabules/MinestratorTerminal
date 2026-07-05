# Copilote — diagnostic automatique multi-LLM

Le **Copilote** est un agent IA qui **diagnostique automatiquement les incidents** de tes serveurs
Minecraft (crash, surcharge CPU/RAM/disque) et **propose des correctifs**. Il est
**indépendant du fournisseur LLM** : Claude, GPT, Gemini, Mistral, ou un modèle local.

Il vit dans `minestrator-core` (logique métier réutilisable : desktop, futur daemon, CLI).

---

## 1. Pourquoi (et pourquoi pas juste MCP)

Un serveur MCP est *tiré* : un client (Claude Desktop…) l'interroge **quand tu le sollicites**.
Il ne se réveille pas tout seul quand un serveur crashe. Le Copilote comble ce manque :
c'est la boucle **autonome** *poussée* par le superviseur, hébergée dans l'app.

```
Superviseur          Copilote                      Couche d'outils MCP
détecte crash/seuil → agent LLM (lecture seule) →  read_console, server_metrics,
émet une Alert       diagnostique + propose        read_file, list_files, …
                            │
                            ▼
                     Rapport (🩺) : cause + correctif + actions proposées
```

## 2. Multi-fournisseur (LLM)

Trois fournisseurs, deux transports (`crates/minestrator-core/src/{llm,cli}.rs`) :

| Fournisseur | Couvre | Réglage |
|---|---|---|
| **Anthropic** (API Messages native) | Claude | clé API |
| **OpenAI-compatible** (Chat Completions) | **OpenAI (GPT)**, **Google Gemini**, **Mistral**, **Groq**, **xAI**, **DeepSeek**, **OpenRouter**, **local** (Ollama, LM Studio, vLLM) | URL de base + modèle (+ clé si requise) |
| **CLI locale** (sous-processus) | **Claude Code**, Gemini CLI, ou tout agent CLI local | commande + args — **aucune clé API** |

Pour les deux premiers, l'agent parle un **format normalisé** ; chaque adaptateur
(dé)sérialise vers le format natif. Ajouter un fournisseur HTTP = ajouter un variant + son
(dé)sérialiseur, sans toucher à l'agent. Préréglages UI (OpenAI, Gemini, Mistral, Groq, Ollama).

### Mode « CLI locale » — sans clé API (ton abonnement)

Si **Claude Code** (le CLI) est installé et connecté (`claude login`), le Copilote peut le lancer
en **sous-processus** (`claude -p`, prompt sur stdin) : il utilise **ton abonnement Pro/Max**,
donc **aucune clé API**. C'est générique : n'importe quel agent CLI « prompt in → texte out »
fonctionne (préréglage **Claude Code** fourni ; champ commande + args libre pour les autres).

- **Deux sous-modes** (réglable) :
  - **Mode agent** (par défaut, Claude Code) : on **branche notre serveur MCP** sur Claude Code
    (`--mcp-config` pointant sur l'app en `--mcp`, `--allowedTools` = nos outils de **lecture**).
    Claude Code devient un vrai agent : il lit lui-même console/métriques/fichiers, **croise**, puis
    rend le rapport. Les outils modifiants ne sont **pas** pré-autorisés → il propose sans exécuter.
  - **Un coup** : le Copilote pré-fournit le contexte (console + métriques) dans un seul prompt.
    Pour les CLI non-Claude ou si tu préfères.
- Prérequis : l'outil doit être **installé et authentifié** sur la machine (le `claude login`
  est interactif, à faire une fois). Ces diagnostics consomment ton quota d'abonnement.
- Windows : une commande « nue » (`claude`) est lancée via `cmd /c` pour résoudre les shims
  `.cmd`/npm et le PATH ; sinon, indique le chemin complet du binaire.
- **Robustesse** (`cli.rs`) : toutes les issues sont couvertes et remontées lisiblement dans le
  rapport 🩺 — binaire introuvable, échec d'écriture, **code de sortie non nul** (avec l'extrait
  stderr), **timeout** (le process est alors **tué et récupéré**, pas de fuite), **sortie vide**.
  stdout/stderr sont lus **en parallèle** (aucun interblocage de tube). En cas d'échec, le rapport
  suggère de vérifier l'installation et le `claude login`.

> **Claude Desktop**, lui, n'expose aucune interface programmable → il ne peut pas piloter le
> Copilote autonome. En revanche il s'utilise **en interactif** comme client MCP (voir `MCP.md`).

## 3. Déclencheurs

- **Crash** : arrêt inattendu détecté par le superviseur (réglable).
- **Seuil** : dépassement CPU/RAM/disque (réglable, désactivé par défaut).
- **Erreur / Avertissement console** : le superviseur scanne en continu la console des serveurs
  actifs (connexion *monitor*) et classe les lignes ; sur une ligne **ERROR** (ou **WARN**), le
  Copilote peut lancer un diagnostic. Chacun réglable séparément, **désactivés par défaut** (WARN
  peut être bruyant). Pas besoin d'ouvrir un onglet console : ça marche en fond, serveur en marche.
- **Sélection (clic droit)** : sélectionne du texte dans la **console** ou l'**éditeur de fichier**,
  clic droit → **🩺 Copilote** → analyse de l'extrait (explication + diagnostic + correctif).
- **Performance (Spark)** : bouton **« Analyser les performances »** (Aperçu) et/ou **auto sur
  surcharge CPU/RAM** (réglable). Voir §10.
- **Manuel** : bouton « Diagnostiquer » (Réglages → Copilote → Tester), utile pour valider la config.

Cooldowns séparés (5 min/serveur) pour les incidents (crash/seuil) et pour les logs (error/warn),
afin qu'ils ne se privent pas mutuellement d'un diagnostic. Les déclencheurs manuels (Tester,
clic droit) ignorent le gate « activé » — ils marchent même Copilote en veille.

## 4. Ce que fait l'agent

L'agent ne dispose **que d'outils de lecture** (jamais d'action modifiante directe) :
`list_servers`, `server_status`, `server_metrics`, `read_console`, `list_files`, `read_file`
— exactement la couche d'outils du MCP. Il lit la console, corrèle avec l'historique de
métriques, inspecte les fichiers de config, puis livre un **rapport structuré** :
résumé · cause probable · correctif détaillé · **0 à 3 actions concrètes** proposées, chacune
classée `safe` / `caution` / `danger`.

## 5. Niveaux d'autonomie (réglables)

| Niveau | Comportement |
|---|---|
| **Suggérer seulement** | N'exécute jamais. Propose ; tu appliques d'un clic si tu veux. |
| **Appliquer sur validation** | Boutons « Appliquer » sur chaque action, exécutés après ta confirmation. |
| **Auto-correctifs sûrs** | Applique **seul** les actions `safe` (ex. redémarrer un serveur crashé) ; le reste attend validation. |

Dans tous les cas, l'exécution d'une action passe par la **couche d'outils MCP** (mêmes garde-fous).

## 6. Sécurité

- **Agent en lecture seule** : il ne peut pas modifier ni supprimer par lui-même ; il *propose*.
- **Clés API dans le trousseau OS** (`llm-key-<fournisseur>`), **une par fournisseur** — jamais en clair,
  jamais transmises ailleurs qu'au fournisseur choisi.
- **Désactivé par défaut** : rien ne part vers un LLM tant que tu ne l'as pas activé et configuré.
- **Par serveur** : exclus n'importe quel serveur du diagnostic.
- **Auto-application** limitée aux seules actions `safe`, et seulement en mode « Auto-correctifs sûrs ».

## 7. Réglage (GUI)

Réglages → **Copilote** : activer, fournisseur + préréglage, modèle, URL de base (avancé),
clé API, niveau d'autonomie, déclencheurs, serveurs surveillés, et bouton **Tester**.
Les rapports apparaissent dans l'icône **🩺** de la barre supérieure (+ notification native).

## 8. Analyse de performance (Spark)

Diagnostic **performance** assisté par le profileur **Spark** (`crates/minestrator-core/src/perf.rs`).
Le cœur **orchestre** (pas l'agent, pour rester sûr et déterministe) :

1. **Collecte** : envoie `spark health` / `spark tps` / `spark gc` (console), attend, **lit la console**.
   La **détection se fait par la réponse console** (pas par le jar) → gère Spark **plugin** OU
   **intégré** (Paper/Purpur/Folia récents, sans jar). Si « unknown command » → note d'aide.
3. **Profiler** : `spark profiler start` → 30 s → `spark profiler stop` → extrait l'**URL** du rapport
   (`https://spark.lucko.me/…`), puis **télécharge le rapport brut** (`spark-usercontent.lucko.me/<clé>`,
   protobuf `x-spark-sampler`), le **parse** (`prost`, structs à la main) et calcule le **temps propre
   par méthode** → **top 15 des points chauds** (avec le plugin source si connu). Ces hotspots sont
   ajoutés au contexte : l'analyse est *réellement* profonde (« FooMobs = 41 % du tick »).
4. **Analyse** : tout ce contexte est passé au Copilote → il identifie la source du lag (plugin, GC,
   view-distance, entités, chunks…) et **propose des correctifs** (flags JVM, config via `write_file`…).

Déclenchement : bouton **« Analyser les performances »** (Aperçu, serveur en ligne) ; et/ou
**auto sur surcharge PROLONGÉE** (`perf_on_overload`) — le Copilote suit le flux de stats et ne
déclenche que si **CPU ou RAM restent ≥ seuil pendant N minutes en continu** (réglables :
`perf_overload_pct`, `perf_overload_minutes` ; défaut 85 % / 3 min), pas sur un simple pic. Le
résultat arrive dans **🩺** ; l'indicateur d'activité tourne pendant la
collecte (~35 s), avec des **étapes de progression** visibles (voir §11). Le `.hprof` binaire, lui,
reste inexploitable (non utilisé).

## 11. UI Copilote (indicateur + onglet dédié)

- **Icône 🩺** (barre supérieure) : vue **minimaliste** — analyses **en cours** avec **barre de
  progression** + étape courante, et les derniers rapports en compact. Un clic sur un rapport (ou
  « Ouvrir dans un onglet ») ouvre l'onglet Copilote.
- **Onglet Copilote** (`CopilotView`) : page complète — section **En cours** (barre + log des étapes,
  durée) et **Historique** (rapports dépliables : cause, correctif, **actions avec boutons Appliquer**,
  et le **log** des étapes). Étapes émises via `CoreEvent::CopilotProgress { id, phase }`.
- **Suivi live de l'agent (comme Claude Code)** : en mode **agent CLI**, on lance Claude Code avec
  `--output-format stream-json --verbose` et on lit sa sortie **ligne par ligne** (`cli::run_streaming`) :
  chaque appel d'outil / réflexion devient une étape en direct — « 📄 Lecture : plugins/config.yml »,
  « 📈 Analyse des métriques », « 💭 … ». Le mode **API** émet aussi une étape avant chaque outil.
  Ces étapes défilent dans le 🩺 (phase courante) et s'accumulent dans le **log** de l'onglet. C'est
  aussi le meilleur diagnostic en cas de lenteur/timeout : on voit ce que fait l'agent.

## 12. Assistant conversationnel (chat)

Un **chat multi-tours par serveur** (onglet **Assistant** 💬) : tu poses des questions en langage
naturel (« Optimise mon serveur pour plus de joueurs », « Mes plugins sont-ils traduits en
français ? », « Mon Skript est-il optimisé ? »). L'assistant **investigue** via les outils de
lecture (console, config, `/plugins`, métriques, `read_startup`) avant de répondre.

- **Deux modes** (interrupteur dans le chat) :
  - **Suggéré** (défaut) : il répond et **propose des actions** (boutons *Appliquer*) — rien n'est
    exécuté sans ton clic.
  - **Autonome** : il peut **exécuter directement** les actions (mêmes garde-fous de normalisation).
- **Multi-fournisseur** : HTTP (Anthropic/OpenAI) en multi-tours natif (transcript conservé) ;
  **Claude Code** via `--resume <session>` (le contexte est maintenu par Claude Code).
- **Streaming** : les étapes (📄 lecture, 📈 métriques…) défilent pendant qu'il réfléchit.
- Une conversation **par onglet** (`session_id` = id d'onglet) ; bouton « Nouvelle conversation ».

Implémentation : `copilot::chat_turn` (`chat_http` / `chat_cli`), `ChatSession`/`ChatReply`,
`Core::chat_send`/`chat_reset`, commande `chat_send`, vue `AssistantView.svelte`. Réutilise la
couche LLM, les outils, `run_streaming` et l'application d'actions.

## 9. Architecture

- `crates/minestrator-core/src/copilot.rs` — config, écoute des alertes, boucle agentique, rapport.
- `crates/minestrator-core/src/llm.rs` — couche multi-fournisseur (adaptateurs Anthropic / OpenAI-compatible).
- Réutilise `mcp::tool_list()` + `mcp::dispatch()` : **une seule** source de vérité pour les outils.
- Émet un `CoreEvent::Diagnosis` ; chaque frontend le relaie (l'app desktop → 🩺 + notification).

## 10. Limites / évolutions

- La surveillance console (ERROR/WARN) dépend du **monitoring du superviseur** (connexions monitor
  ouvertes pour les serveurs actifs) : superviseur désactivé → pas de déclenchement sur log.
- Classement des lignes = heuristique (`/ERROR]`, `SEVERE`, `FATAL`, `/WARN]`, `WARNING`…) ; robuste
  pour les logs Java/Minecraft, extensible si besoin.
- `resources`/`prompts` MCP et modèles de prompts spécialisés (« optimiser la RAM ») à venir.
- Coût : chaque diagnostic est un appel LLM facturé par ton fournisseur (le local/CLI est gratuit).

---

*Implémentation : `crates/minestrator-core/src/{copilot,llm}.rs`. Voir aussi [`MCP.md`](./MCP.md)
pour la couche d'outils partagée.*
