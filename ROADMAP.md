# Feuille de route — Minestrator Terminal

> Client desktop (Tauri 2 + SvelteKit) pour l'API MineStrator. **v0.2.1** · public · GPL-3.0.
>
> Le principe reste le même : ne pas refaire le panel web, mais exploiter ce qu'une app
> **persistante et locale** peut faire et que le site ne peut pas — historique, événements,
> automatisation, intégration OS. Cette feuille de route reflète l'état **réel** du projet.

## ✅ Livré

- **Superviseur + tray** — surveille tous les serveurs en tâche de fond, même fenêtre fermée.
- **Historique CPU/RAM/disque** — séries temporelles stockées en local (SQLite) + graphiques.
- **Alertes natives** — crash, seuils CPU/RAM/disque, expiration MyBox (`tend_days`).
- **Registre de snapshots** — l'app note ceux qu'elle crée, contournant l'absence de listing API.
- **Tableau expiration & crédits** — comptes à rebours MyBox, solde, rappels avant suspension.
- **Serveur MCP intégré** — expose l'app à Claude (statut, commandes, power, logs, métriques, SFTP).
- **Marketplace mods/plugins** — recherche Modrinth / CurseForge / Spigot → upload SFTP.
- **Mises à jour automatiques** — updater Tauri signé.

## 🚀 Livré au-delà du plan initial

- **Copilote IA multi-fournisseur** — Anthropic + tout endpoint OpenAI-compatible, plus des
  agents CLI locaux (Claude Code / OpenCode / Gemini).
- **MCP officiel MineStrator** — intégration du serveur MCP officiel, en plus du MCP maison.
- **Inspecteur NBT** — arbre typé (lecture seule), vue/copie SNBT, recherche ; **carte des régions** `.mca`.
- **Ouverture d'archives** — lecture directe de `.zip` / `.tar` / `.gz`.
- **Export vers services de paste** — logs et fichiers vers mclo.gs / pastes.dev.
- **Analyse de performance** — rapports Spark + docteur de démarrage.
- **i18n & thèmes** — français / anglais, clair / sombre.

## 🔧 À finir

- **Auto-restart avec backoff** — la détection de crash existe (simple alerte aujourd'hui) ;
  reste le redémarrage automatique borné + capture de la console autour du crash.
- **Suivi de disponibilité** — uptime %, journal des downtimes, style SLA.
- **Console intelligente** — surlignage regex, autocomplétion des commandes MC, clic-pseudo,
  marque-pages.
- **Éditeurs de config en formulaire** — `server.properties`, `ops.json`, `whitelist.json` via SFTP.
- **Finitions natives** — autostart au démarrage de session + raccourcis globaux.

## 🗺️ Backlog

- **Planificateur cron local** — redémarrages, annonces et snapshots programmés.
- **Moteur de règles** — event → action (« si CPU > 90 % 5 min → restart + notif »).
- **Macros de commandes** — séquences enregistrées, un clic = N commandes.
- **Actions bulk multi-serveurs** — redémarrer tout, diffuser un message partout.
- **Analytics joueurs** — playtime, first/last seen, pics de connexion.
- **Sync SFTP local ↔ distant** — surveillance de dossier + diff (workflow dev de plugins).
- **Palette de commandes (Ctrl+K)** — navigation et actions en recherche floue.
- **Console unifiée multi-serveurs** — vue « tour de contrôle » + recherche globale des logs.
- **Intégrations Discord / webhooks** — alertes poussées vers les communautés.
- **Page de statut / overlay OBS** — joueurs en ligne et uptime, partageables.
- **Multi-comptes** — plusieurs clés API, vue agrégée.
- **Journal d'audit local** — chaque action faite via l'app, exportable.
- **Création de serveurs par templates** — `POST /server` + presets d'eggs réutilisables.
