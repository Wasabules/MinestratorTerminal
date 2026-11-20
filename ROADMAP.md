# Feuille de route — Minestrator Terminal

> Client **desktop** (Windows, macOS, Linux) et **Android** pour l'API MineStrator. Dernière version
> publiée : **v0.6.1** · public · GPL-3.0. Les fonctionnalités marquées *(main)* sont fusionnées et
> testées en CI, et partiront avec la prochaine version.
>
> Le principe reste le même : ne pas refaire le panel web, mais exploiter ce qu'une app
> **persistante et locale** peut faire et que le site ne peut pas — historique, événements,
> automatisation, intégration OS. Cette feuille de route reflète l'état **réel** du projet.

## ✅ Livré

### Supervision et automatisation

- **Superviseur + tray** — surveille tous les serveurs, fenêtre fermée.
- **Service d'arrière-plan** (Windows, Linux, macOS) — supervision même application fermée ou
  session déconnectée ; l'app s'efface quand il surveille le même compte.
- **Historique CPU/RAM/disque** — séries temporelles locales (SQLite) et graphiques.
- **Alertes natives** — crash, seuils, échéance et suppression des MyBox (sauf renouvellement auto).
- **Redémarrage automatique après crash** *(main)* — par serveur, délais croissants, plafond
  horaire, dernières lignes de console jointes à l'alerte.
- **Automatisations planifiées** *(main)* — redémarrage avec annonces aux joueurs, snapshot,
  commandes ; exécutées par l'app ou le service, distinctes des tâches du panel web.
- **Alertes Discord et webhooks** *(main)* — par événement, anonymisées, regroupées.
- **Suivi de disponibilité** *(main)* — 24 h / 7 j / 30 j et journal des coupures, sans compter
  les périodes non surveillées.

### Serveurs

- **Console live** (WebSocket Wings), autocomplétion et coloration adaptées au jeu.
- **Fichiers SFTP complets** — éditeur, archives, inspecteur NBT, carte des régions `.mca`,
  transferts reprenables, recherche.
- **Paramètres `server.properties`** via l'API, valeurs secrètes masquées.
- **Snapshots et backups**, journal d'activité, notifications de l'hébergeur.
- **Joueurs** — kick, ban, op, whitelist ; tous les joueurs déjà venus (Minecraft Java) avec temps
  de jeu, dernière venue, recherche et tri, fiche (activité, état, combats) et inventaire en
  lecture ; statistiques (actifs 24 h / 7 j / 30 j, plus gros temps de jeu).
- **Marketplaces** — Modrinth / CurseForge / Spigot, mods Satisfactory (ficsit), Factorio,
  Thunderstore, uMod.
- **Adaptation par jeu** — vues, outils et commandes selon le jeu détecté.
- **Vue Réseau** *(main)* — réseaux proxy Velocity / BungeeCord d'une MyBox en schéma (proxy →
  serveurs, état, ping, réglages et problèmes en badges), « Pinger tout », assistant « Relier des
  serveurs » (repli, routage par domaine, nettoyage) et préréglages LuckPerms / Geyser : plan
  simulé avec diffs, application après confirmation, annulable. Écritures réelles encore à
  valider sur un réseau de test.

### Comptes et sécurité

- **Plusieurs comptes MineStrator** et changement de compte *(main)*.
- **Secrets en stockage sécurisé** — trousseau de l'OS, Keystore Android, fichier chiffré DPAPI
  pour le service Windows ; aucun identifiant en clair.
- **Mot de passe SFTP** obtenu automatiquement (`/server/{id}/sftp`), saisie manuelle en secours.
- **Vérification de l'identité du serveur SFTP** *(main)* — empreinte mémorisée, connexion bloquée
  avant l'envoi du mot de passe si elle change.
- **Confidentialité** — anonymisation avant tout envoi à une IA, un paste ou un webhook ;
  journaux anonymisés ; mode « données de démo » pour les captures.

### IA

- **Copilote multi-fournisseur** — Anthropic, tout endpoint OpenAI-compatible, agents CLI locaux
  (Claude Code, OpenCode, Gemini CLI) privés de shell et de disque.
- **Serveur MCP intégré** (deny-by-default) et **MCP officiel MineStrator**.
- **Analyse de performance** (rapports Spark) et docteur de démarrage.

### Confort

- **Palette de commandes Ctrl+K / ⌘K** *(main)* — serveurs, vues, actions, commandes console,
  réglages, comptes.
- Onglets persistants, fenêtres détachées, français / anglais, thèmes clair / sombre.
- **Mises à jour automatiques signées** (desktop et APK Android).

### Application Android

- Aperçu (jauges, historique 1 h / 6 h / 24 h *(main)*, alertes récentes *(main)*), console,
  joueurs, fichiers, sauvegardes, notifications de l'hébergeur.
- **Paramètres du serveur, centre d'alertes, mods et plugins** *(main)*.
- Comptes *(main)*, surveillance en arrière-plan, transferts avec notification.

### Outillage

- CI sur les 3 OS, audit de sécurité hebdomadaire, types TypeScript générés depuis Rust.
- **Surveillance quotidienne de l'API MineStrator** *(main)* — contrat des réponses et structure de
  la documentation OpenAPI ; un écart ouvre une issue.
- **Contrôle des traductions** fr/en en CI *(main)*.

## 🔧 À finir

- **Valider en conditions réelles** ce qui n'a été testé qu'en CI et en simulation : installeur
  Windows et activation du service, service installé, automatisations, multi-comptes, palette.
- **Endpoints non documentés** — recensés dans `docs/api/REFERENCE.md` (presque tous accessibles
  avec une clé API) ; à exploiter : API fichiers REST (sans SFTP), état groupé des serveurs,
  tâches planifiées du panel, statistiques, sous-utilisateurs, bases de données.
- **Console intelligente** — surlignage personnalisé, clic sur un pseudo, marque-pages.
- **Éditeurs en formulaire** — `ops.json`, `whitelist.json`.
- **Finitions natives** — lancement à l'ouverture de session, raccourcis globaux.
- **Mobile** — disponibilité et automatisations en lecture, Copilote.
- **Service Windows** — compte de service virtuel dédié (`NT SERVICE\…`) au lieu de `LocalService`.
- **Signature de code** Windows et macOS (avertissements SmartScreen et Gatekeeper).
- **TypeScript 7**, dès que SvelteKit le prendra en charge.

## 🗺️ Backlog

- **Moteur de règles** — généraliser le redémarrage auto : événement → action
  (« si CPU > 90 % pendant 5 min → redémarrage + notification »).
- **Macros de commandes** — séquences enregistrées, un clic = N commandes.
- **Actions groupées multi-serveurs** — tout redémarrer, diffuser un message partout.
- **Statistiques joueurs** — reste : temps de jeu **par période** (l'API ne donne que le cumul :
  il faudrait des relevés locaux datés) et pics de fréquentation.
- **Synchronisation SFTP local ↔ distant** — surveillance de dossier et diff (développement de
  plugins).
- **Console unifiée multi-serveurs** — vue « tour de contrôle » et recherche globale des logs.
- **Page de statut / overlay OBS** — joueurs en ligne et disponibilité, partageables.
- **Journal d'audit local** — chaque action faite via l'app, exportable.
- **Création de serveurs par modèles** — `POST /mybox/{id_mybox}/server` + catalogues d'eggs et de
  localisations, préréglages réutilisables.
- **iOS**.
