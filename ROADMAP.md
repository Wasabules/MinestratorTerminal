# Au-delà du panel — analyse & pistes à forte valeur

> Objectif : ne pas refaire le panel web, mais exploiter ce qu'une **app desktop** peut faire
> et que le site ne peut pas. Analyse ancrée dans les capacités **et les limites réelles** de
> l'API MineStrator (relevées pendant le dev).

## 1. La thèse

Le panel web est **sans état** : requête → réponse, et il ne « vit » que tant que l'onglet est ouvert.
Une app desktop peut être un **agent persistant et local**. Quatre avantages structurels :

| Avantage | Ce que ça débloque |
|---|---|
| **Persistance** (tray, 24/7) | surveiller même fenêtre fermée, réagir aux événements |
| **Stockage local** | garder ce que l'API ne garde pas : historique, analytics, journaux |
| **Intégration OS** | notifications natives, hotkeys globaux, fichiers, autostart |
| **Hôte d'automatisation** | planificateur, moteur de règles, **serveur MCP** pour Claude |

**L'avantage décisif** : l'app comble les 3 gros trous de l'API. Là où le panel montre l'instant T,
l'app **raconte l'histoire** et **agit** en continu.

## 2. Limites API qui cadrent le réalisable (déjà vérifiées)

- ✅ **Instantané** : stats temps réel (WS), console (WS), `live/light` (joueurs/version/limites),
  power actions, commandes, actions joueurs, SFTP (creds complets), liste serveurs (flags : expiration,
  suspension, hibernation), catalogue (locations/eggs), ressources MyBox, solde crédits.
- ❌ **Absent** : historique de métriques · suivi de `job_id` · **listing** (snapshots, backups, bans,
  ops, whitelist) · events *push* (le WS n'existe que serveur allumé).

➡️ **Conséquence stratégique** : beaucoup des features à plus forte valeur consistent précisément à
**reconstruire localement ce que l'API ne fournit pas**. C'est là que le desktop gagne.

---

## 3. Le catalogue de features (valeur ★ / effort S-M-L / faisabilité)

### A. Supervision & monitoring — **LE** différenciateur

| # | Feature | Valeur | Effort | Note de faisabilité |
|---|---|---|---|---|
| A1 | **Superviseur en arrière-plan** (icône tray, surveille tous les serveurs même fenêtre fermée) | ★★★★★ | M | Cœur : maintient un poll `live/light` + WS léger par serveur. Base de tout le reste. |
| A2 | **Historique & graphiques de métriques** (CPU/RAM/disque/joueurs sur heures/jours/semaines) | ★★★★★ | M | **L'API n'a AUCUN historique** → on stocke localement (SQLite). Impossible sur le panel. |
| A3 | **Alertes & notifications natives** (crash, seuils CPU/RAM/disque, 0 joueur, pic de joueurs, **expiration MyBox** via `tend_days`, suspension, sortie d'hibernation) | ★★★★★ | S/M | Notifs OS + centre d'alertes in-app. Actionnable (« Redémarrer », « Ouvrir la console »). |
| A4 | **Détection de crash + auto-récupération** (running→offline inattendu → redémarrage auto avec backoff/limite, capture de la console autour du crash) | ★★★★★ | M | On détecte via le `status` WS ; on relance via REST. Le « crash report » local est unique. |
| A5 | **Suivi de disponibilité** (uptime %, journal des downtimes, style SLA) | ★★★★ | S | Dérivé du superviseur. Très parlant pour une commu. |
| A6 | **Registre de snapshots local** (l'app note les snapshots qu'elle crée → permet restore/delete malgré l'absence de listing API) | ★★★★ | S | Contournement malin de la limite « pas de listing ». |
| A7 | **Tableau de bord expiration & crédits** (comptes à rebours MyBox, solde, rappels avant expiration/suspension) | ★★★★ | S | `tend_days` + `money` déjà dispo. Évite de perdre un serveur. |

### B. Automatisation

| # | Feature | Valeur | Effort | Note |
|---|---|---|---|---|
| B1 | **Serveur MCP** (expose l'app à Claude : lister, statut, commandes, power, kick/ban, lire logs, métriques, SFTP) | ★★★★★ | M | Toute la plomberie existe déjà. Transforme Claude en « mains » sur tes serveurs (cf. §4). |
| B2 | **Planificateur (cron local)** (redémarrages programmés, annonces récurrentes, snapshots planifiés, « redémarrage nocturne ») | ★★★★★ | M | Le desktop persistant est l'endroit idéal pour ça. |
| B3 | **Moteur de règles (event → action)** (« si console matche regex → commande », « si 0 joueur 30 min → stop », « si CPU>90% 5 min → restart + notif ») | ★★★★★ | M/L | La brique la plus « intelligente ». Composable avec A/B. |
| B4 | **Macros de commandes** (séquences enregistrées, un clic = N commandes, avec variables — ex. « lancer event ») | ★★★ | S | Petit effort, gros confort admin. |
| B5 | **Actions multi-serveurs** (redémarrer tout, diffuser un message partout, appliquer un réglage en masse) | ★★★★ | S | Le panel force le 1-par-1 ; l'app fait le bulk. |

### C. Outils métier Minecraft (là où on devient irremplaçable)

| # | Feature | Valeur | Effort | Note |
|---|---|---|---|---|
| C1 | **Gestionnaire de plugins/mods** (recherche **Modrinth/Spigot** intégrée → téléchargement → **upload SFTP** dans `plugins/`, suivi des versions, mises à jour) | ★★★★★ | M/L | API Modrinth publique + notre SFTP. **Tueur** pour les admins MC. |
| C2 | **Analytics joueurs** (playtime, pics de connexion, first/last seen, classement) construits en parsant la console + `players.list` | ★★★★ | M | L'API ne donne que l'instant ; l'app bâtit l'historique. |
| C3 | **Console intelligente** (règles de surlignage regex, **autocomplétion des commandes MC**, clic sur un pseudo → actions, timestamps, marque-pages, export) | ★★★★ | M | Prolonge la console déjà solide. |
| C4 | **Éditeurs de config en formulaire** via SFTP (`server.properties` en UI, `ops.json`/`whitelist.json`/`banned-*.json` en listes éditables) | ★★★★ | M | Compense l'absence de listing bans/ops : on **lit les JSON via SFTP** ! Puissant. |
| C5 | **Sync SFTP local↔distant** (surveiller un dossier local, auto-upload à la sauvegarde — dev de plugins ; diff ; comparer `server.properties` entre serveurs) | ★★★★ | M | Workflow dev que le panel ne peut pas offrir. |

### D. Productivité & UX (nativement desktop)

| # | Feature | Valeur | Effort | Note |
|---|---|---|---|---|
| D1 | **Palette de commandes (Ctrl+K)** (aller à un serveur/vue, lancer une action, recherche floue) | ★★★★ | S | Rapidité « pro ». Peu d'effort. |
| D2 | **Console unifiée multi-serveurs** + recherche globale dans les logs | ★★★★ | M | Vue « tour de contrôle ». |
| D3 | **Hotkeys globaux, autostart au démarrage, mises à jour auto** (updater Tauri) | ★★★ | S/M | Finitions natives qui font « vrai logiciel ». |

### E. Intégrations sortantes

| # | Feature | Valeur | Effort | Note |
|---|---|---|---|---|
| E1 | **Discord / webhooks** (alertes crash/joueurs/expiration poussées sur Discord ; statut) | ★★★★ | S/M | Les communautés MC vivent sur Discord. Fort effet de levier. |
| E2 | **Page de statut / overlay OBS** (joueurs en ligne, uptime — partageable/embarquable pour streamers) | ★★★ | M | Niche mais différenciant. |

### F. Plateforme

| # | Feature | Valeur | Effort | Note |
|---|---|---|---|---|
| F1 | **Multi-comptes** (plusieurs clés API, vue agrégée) | ★★★ | S | Revendeurs / gros admins. |
| F2 | **Journal d'audit local** (chaque action faite via l'app, exportable) | ★★★ | S | Traçabilité, debug, conformité. |
| F3 | **Création de serveurs par templates** (MyBox `POST /server` + catalogue eggs, presets réutilisables) | ★★★ | M | Industrialise le déploiement. |

---

## 4. Zoom sur tes deux intuitions

### Le **superviseur** (A1-A7) — la colonne vertébrale
Un service en tâche de fond qui, pour chaque serveur : maintient l'état (poll + WS), **enregistre une
série temporelle locale** (SQLite), applique des **seuils/règles**, **notifie**, et peut **agir**
(auto-restart, snapshot avant expiration). C'est ce qui fait passer l'app de « télécommande » à
« gardien ». Tout le reste (graphes, alertes, analytics, uptime) se branche dessus.

### Le **serveur MCP** (B1) — l'app comme « mains » de Claude
On expose des *tools* MCP : `list_servers`, `server_status`, `send_command`, `power_action`,
`player_action`, `read_console`, `metrics_history`, `sftp_read/write`… Bénéfices :
- **Ops en langage naturel** : « redémarre survie avec un compte à rebours de 5 min ».
- **Diagnostic** : Claude **lit la console/les crash reports** et explique la cause.
- **Bulk NL** : « bannis griefer123 sur tous mes serveurs ».
- **Boucles agentiques** : « surveille, et si ça crash 3× en 1h, préviens-moi avec les logs ».

La plomberie (client API, WS, SFTP) est **déjà écrite** → l'exposer en MCP est incrémental. C'est
probablement le meilleur ratio valeur/effort × originalité de toute la liste.

---

## 5. Feuille de route priorisée

- **Phase 1 — « l'app qui veille »** : A1 superviseur · A3 notifications · A2 historique+graphes · A4
  crash/auto-restart · A7 expiration. → transforme immédiatement la nature de l'app.
- **Phase 2 — « l'app qui agit »** : B1 MCP · B2 planificateur · B3 règles · B5 multi-serveurs.
- **Phase 3 — « l'app de l'admin MC »** : C1 plugins (Modrinth) · C2 analytics joueurs · C3 console
  intelligente · C4 éditeurs de config.
- **Phase 4 — « l'app de la commu »** : E1 Discord · D1 palette · D2 console unifiée · F1 multi-comptes.

## 6. Ma reco — le trio signature (si tu ne fais que 3 choses)

1. **Superviseur tray + notifications + historique/graphes** (Phase 1) — le plus gros saut de valeur,
   et impossible à répliquer côté web.
2. **Serveur MCP** (Phase 2) — rend l'app pilotable et diagnosticable par Claude ; effort faible car la
   base existe.
3. **Gestionnaire de plugins Modrinth → SFTP** (Phase 3) — la feature « waouh » pour les admins MC.

> Fil rouge : **exploiter la persistance + le stockage local** pour battre l'API sur son propre terrain
> (historique, events, listing) — c'est ça, dépasser le panel.
