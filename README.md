# Minestrator Terminal

Client desktop **léger, rapide et cross-platform** pour piloter ses serveurs Minecraft via l'**API MineStrator** — console temps réel, SFTP natif, gestion des joueurs, marketplace de mods/plugins, sauvegardes, et un **Copilote IA** capable de diagnostiquer et réparer.

![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white)
![Svelte](https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white)
![Rust](https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white)
![Platforms](https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-555)
![License](https://img.shields.io/badge/license-MIT-green)

> Windows · macOS · Linux — la clé API est stockée dans le **trousseau natif** de l'OS, aucun secret en clair.

---

## ✨ Fonctionnalités

**Pilotage**
- **Console temps réel** (xterm.js) via WebSocket (protocole Pterodactyl Wings) : logs live, filtres par niveau, envoi de commandes, copie de la sélection.
- **Alimentation** : démarrer · redémarrer · arrêter · kill.
- **Joueurs** : kick, ban/unban, op, whitelist — y compris sur des pseudos hors-ligne.
- **SFTP natif intégré** (russh) : explorateur de fichiers + éditeur de config (CodeMirror, coloration JSON/YAML/Java/…), glisser-déposer pour l'upload.

**Contenu & sauvegardes**
- **Marketplace** mods & plugins : recherche Modrinth / CurseForge / SpigotMC, choix de version, installation en un clic.
- **Sauvegardes** : backups quotidiens automatiques (restaurables) + **snapshots** à la demande — le filet avant une intervention risquée.

**Supervision & IA**
- **Superviseur** en tâche de fond : historique CPU/RAM/disque (SQLite), alertes (crash, seuils, expiration de MyBox) → **notifications natives**.
- **Copilote IA** :
  - Diagnostic **automatique** (crash, surcharge prolongée) et **assistant conversationnel**.
  - **Multi-fournisseur** : Anthropic (Claude) et tout service compatible OpenAI (GPT, Gemini, Mistral, Groq, xAI, DeepSeek, OpenRouter, Ollama / LM Studio locaux…).
  - **Agents CLI** : Claude Code, OpenCode, Gemini CLI (utilisent l'abonnement déjà configuré sur la machine, sans clé API) — avec détection automatique de leur présence.
  - Outils de réparation : **docteur démarrage** (crash-loop), **maps corrompues** (fichiers `.mca`), **analyse de performance** (Spark).
- **Serveur MCP intégré** : expose la gestion via le **Model Context Protocol** → utilisable par n'importe quel client MCP (Claude Desktop, Claude Code, Cline…). Voir [`MCP.md`](./MCP.md).

**Confort**
- Multi-onglets (dont fenêtres détachées), **tray** (fermer la fenêtre la masque, le superviseur reste actif), thème clair/sombre, **i18n fr/en**.
- **Confidentialité** : anonymisation (mots de passe, IPv4, e-mails, secrets) avant tout envoi à une IA.

---

## 📦 Installation

Télécharge le dernier installeur depuis la page **[Releases](../../releases)** :

| OS | Fichier |
|---|---|
| Windows | `.msi` ou `.exe` (NSIS) |
| macOS | `.dmg` (universel Intel + Apple Silicon) |
| Linux | `.AppImage`, `.deb` ou `.rpm` |

> Les binaires ne sont pas encore signés : Windows SmartScreen / macOS Gatekeeper peuvent afficher un avertissement au premier lancement.

Au premier démarrage, saisis ta **clé API MineStrator** — elle est validée puis stockée dans le trousseau de l'OS.

---

## 🏗️ Architecture

Monorepo **Cargo workspace** qui **sépare strictement la logique métier de l'UI** : le cœur est portable vers un futur daemon Linux ou une CLI/TUI.

```
crates/
  minestrator-core/   Logique métier UI-agnostique : client API, WebSocket, SFTP,
                      superviseur, Copilote (LLM + agents CLI), serveur MCP.
  minestrator-mcp/    Serveur MCP autonome (headless), bâti sur le core.
src-tauri/            App desktop Tauri : commandes IPC, tray, pont d'events, notifications.
src/                  Frontend SvelteKit / Svelte 5 (runes) + TypeScript.
docs/                 Documentation additionnelle.
```

- **Stack** : Tauri 2 · Rust · SvelteKit · Svelte 5 · TypeScript.
- **Tout le réseau** (HTTP, WebSocket, SFTP) vit **côté Rust** ; le front ne parle qu'à une couche IPC typée (`src/lib/ipc.ts`).

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
cargo build -p minestrator-mcp --release  # binaire serveur MCP autonome
```

### Publier une release
Les installeurs (Windows/macOS/Linux) sont produits par GitHub Actions au **push d'un tag** :
```bash
git tag v0.1.0 && git push origin v0.1.0
```
Le workflow [`release.yml`](./.github/workflows/release.yml) build les trois OS et publie une release avec les binaires en pièces jointes.

---

## 📚 Documentation

| Doc | Contenu |
|---|---|
| [`ARCHITECTURE-V1.md`](./ARCHITECTURE-V1.md) | Architecture, choix techniques, faits API validés |
| [`DESIGN.md`](./DESIGN.md) | Charte graphique / design system |
| [`COPILOT.md`](./COPILOT.md) | Fonctionnement du Copilote IA |
| [`MCP.md`](./MCP.md) | Serveur MCP : catalogue d'outils, sécurité, connexion |
| [`docs/AUTO-UPDATE.md`](./docs/AUTO-UPDATE.md) | Mise à jour automatique (préparée, non activée) |
| [`ROADMAP.md`](./ROADMAP.md) | Suite prévue |

---

## 📄 Licence

[MIT](./LICENSE) © Geoffrey Lecoq
