# Journal des modifications

Toutes les versions publiées de **Minestrator Terminal**, de la plus récente à la plus ancienne.

Le contenu de chaque version est repris tel quel dans la release GitHub correspondante
(cf. [`.github/workflows/release.yml`](.github/workflows/release.yml)) : ce fichier est la source
unique des notes de version. Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/).

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
