# Journal des modifications

Toutes les versions publiées de **Minestrator Terminal**, de la plus récente à la plus ancienne.

Le contenu de chaque version est repris tel quel dans la release GitHub correspondante
(cf. [`.github/workflows/release.yml`](.github/workflows/release.yml)) : ce fichier est la source
unique des notes de version. Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/).

---

## v0.7.0

### Nouvelle apparence « Biome »

- **L'application change de visage, sur ordinateur comme sur téléphone.** Chaque serveur a
  désormais un biome — forêt, océan, désert, neige, Nether ou End — qui colore ses onglets et
  l'en-tête de son tableau de bord, où s'affiche un paysage en pixels propre à ce serveur. Le
  biome se choisit d'un clic droit sur l'onglet (ordinateur) ou depuis la vue du serveur
  (téléphone) ; les couleurs déjà attribuées à vos serveurs sont converties vers le biome le plus
  proche.
- Les onglets d'un même serveur sont réunis en un seul bloc à sa couleur, et l'onglet actif se
  fond dans la page.
- Le tableau de bord d'un serveur occupe toute la largeur de la fenêtre : état, adresse et boutons
  d'alimentation en tête, raccourcis vers chaque vue, puis historique, ressources, joueurs et
  disponibilité.
- Les polices de l'application sont intégrées : l'affichage est le même quel que soit le système.

### Mode anonyme pour les captures d'écran

- **Un bouton en forme d'œil masque instantanément les adresses IP, adresses de connexion, hôte
  SFTP et e-mail du compte**, partout dans l'application, console comprise. Raccourci
  Ctrl+Maj+H sur ordinateur. Les noms de serveurs et les pseudos restent visibles ; les boutons
  « Copier » copient toujours la vraie valeur.

### Joueurs

- **La vue Joueurs liste tous les joueurs déjà venus**, pas seulement ceux connectés : temps de
  jeu, dernière venue, opérateur, liste blanche ou banni, avec recherche, tri et statistiques
  (actifs sur 7 et 30 jours, meilleurs temps de jeu).
- Chaque joueur a sa fiche : santé, morts, distances parcourues et inventaire illustré. Sur
  téléphone, la liste et la fiche sont consultables.

### Historique et disponibilité

- Les graphiques remontent jusqu'à 30 jours et montrent les joueurs connectés. Quand votre
  ordinateur était éteint, les statistiques de l'hébergeur prennent le relais (en pointillés).
- La disponibilité de chaque serveur est suivie sur 24 heures, 7 jours et 30 jours, avec la
  liste des coupures.

### Mods, plugins et fichiers

- **Les mises à jour des mods, plugins et datapacks sont signalées** sur l'aperçu et dans le
  Marketplace : mise à jour d'un élément ou de tous, puis proposition de redémarrer. Une version
  prévue pour une autre plateforme (par exemple un plugin pour proxy proposé à un serveur Paper)
  n'est jamais proposée.
- Compresser et extraire des archives se fait désormais directement sur le serveur, et
  télécharger un dossier en .zip est bien plus rapide.
- Une vue « Espace disque » montre ce qui occupe le plus de place, dossier par dossier.
- Le mot de passe SFTP est de nouveau récupéré automatiquement : plus besoin de le saisir.
- L'application vérifie l'identité du serveur SFTP à chaque connexion et prévient si elle change.

### MyBox

- **Un onglet MyBox permet de gérer vos ressources et vos serveurs** : créer un serveur
  (catalogue des jeux, processeur et mémoire limités à ce qui reste disponible, suggestion de
  MyBox), le redimensionner, le renommer, réordonner, mettre en pause ou supprimer — avec une
  sauvegarde proposée avant suppression et le nom à retaper. Sur téléphone, les ressources sont
  consultables et les noms modifiables.

### Automatisations et alertes

- **Redémarrage automatique en cas de crash**, tâches planifiées exécutées par l'application et
  historique de leurs exécutions.
- Les tâches planifiées du panel MineStrator, exécutées par l'hébergeur même ordinateur éteint,
  sont visibles et modifiables depuis l'application et le téléphone.
- Les alertes peuvent être envoyées sur Discord ou vers n'importe quel webhook.

### Navigation et comptes

- **Palette de commandes** (Ctrl+K) : aller à un serveur, ouvrir une vue, lancer une action.
- **Plusieurs comptes MineStrator** : ajoutez-les et passez de l'un à l'autre sans ressaisir de
  clé.
- Sur téléphone : vues Paramètres du serveur, Mods et plugins, centre d'alertes, historique des
  ressources et tâches planifiées.

### Copilote et serveur MCP

- Le serveur MCP intégré passe à 77 outils : recherche dans les journaux (y compris archivés),
  regroupement des erreurs par cause, audit de configuration et de sécurité, contrôle des
  dépendances des plugins et mods, informations sur le monde, outils sur plusieurs serveurs
  (copie, comparaison, recherche d'un joueur, annonce groupée), gestion des MyBox et des tâches
  planifiées.
- Il peut aussi relayer, sur option, les outils du MCP officiel de MineStrator : une seule
  connexion pour votre assistant, et la clé API reste dans le trousseau de l'ordinateur. Toute
  suppression ou restauration relayée exige de confirmer le nom de la ressource.
- **Le Copilote est mieux cloisonné** : en mode autonome, il ne peut plus supprimer, restaurer,
  réinstaller ni gérer l'infrastructure, n'agit que sur le serveur de la conversation et ne
  touche plus à la commande de démarrage. Les assistants en ligne de commande n'ont plus accès à
  la clé API.

### Corrections

- Certains secrets des fichiers de configuration (comme `management-server-secret`) et les
  identifiants des joueurs pouvaient être transmis en clair à l'IA : ils sont désormais masqués.
- Le journal d'activité de l'aperçu affichait des libellés vides : chaque action est maintenant
  décrite en toutes lettres.
- Dans la console, les filtres d'erreurs, d'avertissements et d'informations ne s'appliquaient
  pas aux lignes déjà affichées.
- Le mode « données de démo » laissait apparaître l'hôte SFTP réel dans la confirmation
  d'identité du serveur.

---

## v0.6.1

### Fichiers (SFTP)

- **L'accès aux fichiers des serveurs fonctionne à nouveau.** MineStrator ne transmet plus le mot
  de passe SFTP à l'application : l'explorateur de fichiers, les transferts et les outils fichiers
  du Copilote échouaient tous. L'application le demande désormais une fois par serveur — il se
  copie depuis l'onglet « Accès » du panel —, le vérifie auprès du serveur, puis s'en souvient.
- Le bouton clé de l'onglet Fichiers permet de changer ou d'oublier ce mot de passe ; s'il est
  refusé, par exemple après un changement dans le panel, l'application le redemande.
- Les mots de passe SFTP sont effacés à la déconnexion du compte.

### Sécurité

- **Android : les identifiants sont désormais chiffrés par le Keystore du téléphone** — clé API,
  clés des fournisseurs d'IA, mots de passe SFTP. Ils étaient jusqu'ici rangés dans un fichier
  privé de l'application, sans chiffrement propre ; ce fichier est repris puis supprimé au premier
  lancement, sans rien avoir à ressaisir.
- Windows : la clé API conservée par le service d'arrière-plan est chiffrée avec la clé du compte
  du service ; une copie du fichier est inexploitable ailleurs.

---

## v0.6.0

### Supervision en arrière-plan

- **Un service système peut surveiller vos serveurs même application fermée** ou session
  déconnectée (Windows, Linux, macOS) : crash, seuils de consommation, échéance des MyBox. Il
  s'active dans Réglages → Service d'arrière-plan ; l'application met alors sa propre surveillance
  en veille pour éviter les doublons.
- Les alertes détectées pendant que l'application était fermée apparaissent dans le centre
  d'alertes à la réouverture.
- **Windows : l'installation ne demande plus de droits administrateur.** L'application s'installe
  pour votre compte ; seule la supervision en arrière-plan, proposée en fin d'installation, en
  demande — à la première connexion dans l'application. L'installeur est disponible en français.
- Après une mise à jour de l'application, les Réglages signalent que le service tourne une autre
  version et proposent de le mettre à jour.

### Paramètres et état des serveurs

- **Nouvelle vue « Paramètres » (Minecraft)** : les réglages de `server.properties` se lisent et se
  modifient directement, sans passer par les fichiers, avec proposition de redémarrage pour les
  appliquer.
- **Aperçu** : indique quand le serveur puise dans le CPU partagé de l'offre (FlexCore), affiche
  le trafic réseau, et la durée de fonctionnement même sans console ouverte.
- **Accueil** : renouvellement automatique de chaque MyBox, CPU et RAM encore disponibles, et
  compte à rebours avant la suppression d'une MyBox expirée.
- Plus d'alerte d'expiration pour une MyBox en renouvellement automatique ; une alerte critique
  prévient quand une MyBox expirée va être supprimée.

### Copilote et intégration IA

- **Les agents en ligne de commande (Claude Code, OpenCode, Gemini CLI) n'ont plus accès au shell
  ni aux fichiers de l'ordinateur.** Ils lisent la console, où écrivent les joueurs : un message
  piégé pouvait jusqu'ici leur faire exécuter une commande sur votre machine. Ils ne disposent plus
  que des outils de gestion des serveurs.
- La clé API transmise à un agent n'est plus laissée sur le disque après son passage.
- Le Copilote garde l'historique de consommation et les mods Satisfactory quand le serveur MCP
  officiel MineStrator est activé.
- Nouveaux outils pour l'IA : lire et modifier les paramètres du serveur (mots de passe masqués) et
  consulter le détail d'une MyBox ; l'état temps réel d'un serveur s'obtient plus vite.

### Diagnostic

- **Journaux de l'application** conservés sept jours. Réglages → Debug (desktop) ou Réglages
  (mobile) : copie anonymisée — adresses IP, e-mails et mots de passe masqués — à joindre à un
  rapport de bug ; ouverture du dossier des journaux sur desktop.

### Sécurité

- Toutes les dépendances sont à jour, dont le client SSH utilisé pour le SFTP, la lecture des
  archives, SQLite et le chiffrement des connexions : plus aucune vulnérabilité connue.
- Les secrets enregistrés hors du trousseau du système ne sont lisibles que par leur propriétaire.

### Application mobile

- Les textes restés en français dans l'interface anglaise sont traduits.
- Copie des journaux anonymisés depuis les Réglages.

### Corrections

- Arrêter un serveur depuis le panel web ou l'API ne déclenche plus de fausse alerte « crash ».
- Le Copilote ne se bloque plus après une réponse vide du modèle : la conversation échouait à
  chaque message suivant.
- La liste des snapshots ne disparaît plus pendant la création d'un snapshot, et leur taille n'est
  plus affichée 1 024 fois trop petite.
- Après une longue session stable, la console se reconnecte aussitôt au lieu d'attendre jusqu'à
  30 secondes.
- Le lien du rapport de performance Spark n'est plus corrompu par la coloration de la console.
- Un refus des identifiants SFTP n'est plus présenté comme une clé API invalide.
- La configuration de démarrage d'un serveur sans fichier JAR (Bedrock…) se lit à nouveau.
- Les réglages du superviseur ne sont plus remis à zéro par une mise à jour qui en ajoute.
- Mobile : le contenu des archives s'affiche avec le nom de chaque fichier.

---

## v0.5.2

### Fiabilité des transferts et du SFTP

- **Les connexions SFTP survivent à la mise en veille.** Une sonde périodique (keepalive SSH)
  empêche les sessions de mourir en silence, et une opération dont la session est tombée est
  automatiquement rejouée sur une connexion neuve. Auparavant, la première action après une veille
  du téléphone ou un passage en arrière-plan échouait systématiquement.
- **Les envois interrompus reprennent où ils s'étaient arrêtés.** Sur mobile, changer d'application
  pendant un envoi coupe le réseau : le transfert redémarre désormais à l'octet près au lieu de
  tout perdre — décisif pour les gros fichiers.
- **Un envoi en cours peut être annulé.**
- Une erreur ordinaire (fichier absent, droits insuffisants) ne ferme plus la connexion : elle
  n'imposait qu'une reconnexion inutile à l'opération suivante.

### Application mobile

- **Envoi et téléchargement de fichiers** depuis le téléphone, avec progression réelle, **vitesse
  en Ko/s ou Mo/s**, et notification Android suivant l'avancement même écran éteint.
- **Centre de transferts** : liste des envois et téléchargements, avec leur état, leur taille et
  leurs erreurs ; le bouton clignote tant qu'un transfert est en cours.
- **Fichiers** : aperçu des images, ouverture des archives `.zip`/`.tar.gz`/`.jar` en lecture, et
  recherche récursive dans le dossier courant.
- **Aperçu réorganisé** : état et jauges regroupés, boutons d'alimentation contextuels (trois au
  lieu de quatre) qui restent accessibles au défilement, message du serveur repliable, et partage
  de l'adresse de connexion.
- **Notifications de l'hébergeur** dans un écran dédié, avec recherche et filtres.
- La notification de surveillance en arrière-plan affiche l'état des serveurs surveillés.
- Raccourci d'appui long sur l'icône de l'application.

### Application desktop

- **Journal d'activité** dans l'Aperçu : qui a fait quoi, quand et depuis quelle adresse — y
  compris les actions faites hors de l'application (panel web, staff de l'hébergeur).
- **Notifications de l'hébergeur** : dans le centre d'alertes et dans un onglet dédié.
- Téléchargement du dernier backup et référence support accessibles depuis l'Aperçu.

### Confidentialité

- **Mode « Données de démo »** : remplace pseudos, adresses, IP et références support par des
  valeurs fictives et cohérentes, pour produire des captures d'écran publiables. Purement
  visuel — les commandes envoyées restent inchangées.

### Corrections

- Les fichiers envoyés depuis Android conservent leur vrai nom et leur extension.
- Un téléchargement incomplet n'est plus présenté comme réussi.
- L'état d'un serveur s'affiche correctement dans l'Aperçu mobile (il indiquait « Inconnu »).
- Un serveur suspendu ne déclenche plus de tentatives de reconnexion sans fin.
- La console affiche la sortie d'installation d'un serveur en cours de déploiement.

---

## v0.5.1

- **Application mobile** : portage des nouveautés de la 0.5.0 — écran des notifications de
  l'hébergeur, activité récente du serveur, téléchargement du dernier backup, avertissements avant
  arrêt ou redémarrage, et états de console explicites pour un serveur suspendu ou en hibernation.

---

## v0.5.0

- **Notifications de l'hébergeur** (incidents, crédits, changements d'offre) dans l'application.
- **Journal d'audit** par serveur : actions, auteur et origine.
- **Icônes de jeu** réelles sur les listes de serveurs, desktop et mobile.
- **Joueurs en ligne** affichés directement dans la liste des serveurs.
- Reconnaissance fiable du jeu de chaque serveur ; les bases de données ne sont plus traitées
  comme des serveurs de jeu.
- Avertissements de l'hébergeur avant un arrêt ou un redémarrage, quand ils sont définis.
