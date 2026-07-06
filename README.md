# Minestrator Terminal

Client desktop **léger, rapide et cross-platform** pour piloter ses serveurs Minecraft via l'**API MineStrator** — console temps réel, SFTP natif complet, inspecteur NBT, marketplace, sauvegardes, et un **Copilote IA** capable de diagnostiquer et réparer.

![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555)
![License](https://img.shields.io/badge/license-GPLv3-blue)

> Windows · macOS · Linux — la clé API est stockée dans le **trousseau natif** de l'OS, aucun secret en clair. **Mise à jour automatique** signée intégrée.

---

## ✨ Fonctionnalités

### 🎮 Pilotage & console
- **Console temps réel** (xterm.js) via WebSocket (protocole Pterodactyl Wings) : logs live, **filtres par niveau** (ERROR/WARN/INFO), envoi de commandes avec historique, copie de la sélection.
- **Export du log** en un clic droit vers un service de paste : **mclo.gs**, l'instance **MineStrator**, ou **pastes.dev** (contenu anonymisé avant envoi).
- **Alimentation** : démarrer · redémarrer · arrêter · kill.
- **Joueurs** : kick, ban/unban, op, whitelist — y compris sur des pseudos hors-ligne.
- **Vue d'ensemble** : statut d'exécution live, CPU/RAM/disque, **graphique d'historique** avec survol des valeurs. Pastille d'état d'exécution **sur chaque onglet**.

### 📁 SFTP natif intégré (russh) — un vrai gestionnaire de fichiers
- Explorateur avec colonnes triables, **glisser-déposer** pour l'upload, **sélection multiple**.
- **Éditeur intégré** (CodeMirror) : coloration JSON/YAML/TOML/Java/Properties/Shell/…, **recherche dans le fichier** (Ctrl+F), formatage/minification JSON.
- **Téléchargement** d'un fichier, ou d'un dossier / d'une sélection **en `.zip`** (récursif côté client).
- **Gestionnaire de transferts** façon navigateur : progression, upload/download suivis en parallèle.
- **Ouverture d'archives** en lecture seule (`.zip`/`.tar`/`.tar.gz`/`.gz`) : parcourir, lire une entrée, en extraire une.
- **Aperçu d'images** (`.png`/`.jpg`/`.gif`/`.webp`/`.svg`…), **suppression multiple**.
- **Export** des fichiers texte vers un service de paste.

### 🧊 Inspecteur NBT & mondes (façon NBTExplorer)
- `.dat` / `.nbt` / `.schem` → **arbre typé repliable** qui **préserve le type NBT** exact, **recherche** dans l'arbre, **hints parlants** (UUID, dates, coordonnées, booléens), **vue et copie en SNBT** (`/data`).
- `.mca` → **carte 32×32 des chunks** (présence / taille / corruption en heatmap), **aller à une coordonnée**, ouverture de l'arbre NBT d'un chunk, **rapport de corruption** + réparation.

### 📦 Contenu & sauvegardes
- **Marketplace** mods & plugins : recherche Modrinth / CurseForge / SpigotMC, choix de version, installation en un clic.
- **Sauvegardes** : backups quotidiens automatiques (restaurables) + **snapshots** à la demande — le filet avant une intervention risquée.

### 🩺 Supervision & IA
- **Superviseur** en tâche de fond : historique CPU/RAM/disque (SQLite), alertes (crash, seuils, expiration de MyBox) → **notifications natives**.
- **Copilote IA** :
  - Diagnostic **automatique** (crash, surcharge prolongée) et **assistant conversationnel**.
  - **Multi-fournisseur** : Anthropic (Claude) et tout service compatible OpenAI (GPT, Gemini, Mistral, Groq, DeepSeek, OpenRouter, Ollama / LM Studio locaux…).
  - **Agents CLI** : Claude Code, OpenCode, Gemini CLI (utilisent l'abonnement déjà configuré sur la machine, **sans clé API**), avec détection automatique de leur présence.
  - Outils de réparation : **docteur démarrage** (crash-loop), **maps corrompues** (`.mca`), **analyse de performance** (Spark).
- **Serveur MCP intégré** : expose la gestion via le **Model Context Protocol** → utilisable par n'importe quel client MCP (Claude Desktop, Claude Code, Cline…). Intégration du **MCP officiel MineStrator**. Voir [`MCP.md`](./MCP.md).

### ⚙️ Confort & sécurité
- Multi-onglets (dont **fenêtres détachées**), **tray** (fermer la fenêtre la masque, le superviseur reste actif), thème clair/sombre, **i18n fr/en**.
- **Mise à jour automatique** signée : bandeau au démarrage, réglable (Réglages → Général).
- **Confidentialité** : la clé API vit dans le trousseau natif ; anonymisation (mots de passe, IPv4, e-mails, secrets) avant tout envoi à une IA **ou** publication vers un paste ; serveur MCP en **deny-by-default** (les écritures se débloquent explicitement).

---

## 📦 Installation

Télécharge le dernier installeur depuis la page **[Releases](../../releases/latest)** :

| OS | Fichier |
|---|---|
| Windows | `.msi` ou `.exe` (NSIS) |
| macOS | `.dmg` (universel Intel + Apple Silicon) |
| Linux | `.AppImage`, `.deb` ou `.rpm` |

Au premier démarrage, saisis ta **clé API MineStrator** (Panel → Compte → Clés API) — elle est validée puis stockée dans le trousseau de l'OS.

Une fois installée, l'app **se met à jour toute seule** : elle propose les nouvelles versions au démarrage (désactivable dans les Réglages).

> Les binaires ne sont **pas signés au niveau OS** : Windows SmartScreen / macOS Gatekeeper peuvent afficher un avertissement au premier lancement. Les **artefacts de mise à jour**, eux, sont signés et vérifiés (voir [`docs/AUTO-UPDATE.md`](./docs/AUTO-UPDATE.md)).

---

## 🏗️ Architecture

Monorepo **Cargo workspace** qui **sépare strictement la logique métier de l'UI** : le cœur est portable vers un futur daemon Linux ou une CLI/TUI.

```
crates/
  minestrator-core/   Logique métier UI-agnostique : client API, WebSocket, SFTP,
                      NBT/.mca, superviseur, Copilote (LLM + agents CLI), serveur MCP.
  minestrator-mcp/    Serveur MCP autonome (headless), bâti sur le core.
src-tauri/            App desktop Tauri : commandes IPC, tray, pont d'events, notifications.
src/                  Frontend SvelteKit / Svelte 5 (runes) + TypeScript.
docs/                 Documentation additionnelle.
```

- **Stack** : Tauri 2 · Rust · SvelteKit · Svelte 5 · TypeScript.
- **Tout le réseau** (HTTP, WebSocket, SFTP, uploads paste) vit **côté Rust** ; le front ne parle qu'à une couche IPC typée (`src/lib/ipc.ts`).

---

## 🚀 Développement

### Prérequis
- [Rust](https://rustup.rs) (stable) + les [prérequis Tauri](https://v2.tauri.app/start/prerequisites/) de ta plateforme (Windows : MSVC Build Tools + WebView2 ; Linux : `webkit2gtk`, `librsvg`, etc.).
- [Node.js](https://nodejs.org) 20+ et npm.

### Lancer en développement
```bash
npm install          # dépendances frontend
npm run tauri dev    # app desktop en dev (hot-reload du front)
```

### Autres commandes
```bash
npm run check                             # typecheck (svelte-check)
npm run build                             # build du frontend seul → build/
npm run tauri build                       # app packagée (installeurs)
cargo test -p minestrator-core            # tests du cœur métier
cargo clippy --workspace --all-targets    # lint
cargo build -p minestrator-mcp --release  # binaire serveur MCP autonome
```

### Publier une release
Incrémente la version dans les **3 fichiers** (`src-tauri/tauri.conf.json`, `package.json`, `Cargo.toml`), puis pousse un tag :
```bash
git tag v0.3.0 && git push origin v0.3.0
```
GitHub Actions ([`release.yml`](./.github/workflows/release.yml)) build les trois OS, **signe** les artefacts, génère `latest.json` et crée une release *draft* à publier. Détails : [`docs/AUTO-UPDATE.md`](./docs/AUTO-UPDATE.md).

---

## 📚 Documentation

| Doc | Contenu |
|---|---|
| [`ARCHITECTURE.md`](./ARCHITECTURE.md) | Architecture actuelle : workspace 3 crates, modules, IPC, events |
| [`DESIGN.md`](./DESIGN.md) | Charte graphique / design system |
| [`COPILOT.md`](./COPILOT.md) | Fonctionnement du Copilote IA |
| [`MCP.md`](./MCP.md) | Serveur MCP : catalogue d'outils, sécurité, connexion |
| [`docs/AUTO-UPDATE.md`](./docs/AUTO-UPDATE.md) | Mise à jour automatique : signature, endpoint, publication |
| [`ROADMAP.md`](./ROADMAP.md) | Ce qui est livré, à finir, et le backlog |
| [`docs/history/ARCHITECTURE-V1.md`](./docs/history/ARCHITECTURE-V1.md) | Spec fondatrice V1 (historique) |

---

## 📄 Licence

[GNU GPL v3.0 ou ultérieure](./LICENSE) © Geoffrey Lecoq

Logiciel libre : vous pouvez l'utiliser, l'étudier, le modifier et le redistribuer, à condition que
tout dérivé distribué reste sous licence GPL (copyleft).

> Non affilié à MineStrator — client tiers utilisant leur API publique.
