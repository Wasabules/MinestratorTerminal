# Journal des modifications

Toutes les versions publiées de **Minestrator Terminal**, de la plus récente à la plus ancienne.

Le contenu de chaque version est repris tel quel dans la release GitHub correspondante
(cf. [`.github/workflows/release.yml`](.github/workflows/release.yml)) : ce fichier est la source
unique des notes de version. Format inspiré de [Keep a Changelog](https://keepachangelog.com/fr/).

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
