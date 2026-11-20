# Captures d'écran du README

Visuels affichés dans le [`README`](../../README.md) principal. Garde les mêmes noms de fichiers
pour les remplacer.

| Fichier | Écran |
|---|---|
| `desktop-home.png` | Desktop — accueil, serveurs de chaque MyBox avec leur biome |
| `desktop-overview.png` | Desktop — tableau de bord d'un serveur |
| `desktop-console.png` | Desktop — console temps réel |
| `desktop-sftp.png` | Desktop — fichiers SFTP et éditeur intégré |
| `desktop-marketplace.png` | Desktop — marketplace et mises à jour des plugins |
| `desktop-mybox.png` | Desktop — gestion des MyBox |
| `mobile-servers.png` | Mobile — serveurs groupés par MyBox |
| `mobile-overview.png` | Mobile — aperçu d'un serveur |
| `mobile-console.png` | Mobile — console |
| `mobile-files.png` | Mobile — fichiers |
| `mobile-mods.png` | Mobile — mods et plugins |
| `mobile-settings.png` | Mobile — paramètres du serveur |

## Refaire les captures

- **Toujours en mode « données de démo »** (Réglages → Debug sur ordinateur, Réglages sur téléphone) : noms de serveurs et de MyBox,
  adresses, IP et pseudos sont remplacés à l'affichage par des valeurs fictives. Ce mode ne
  touche pas au **contenu** de la console ni des fichiers : choisis un serveur dont le journal ne
  contient ni domaine, ni lien, ni identifiant personnel, et un fichier sans secret.
- **Desktop** : fenêtre de 1280×800 en thème sombre, capture au facteur 1,5 (1920×1200).
- **Mobile** : 412×892 au facteur 2 (824×1784), en portrait. Depuis un appareil :
  ```bash
  adb exec-out screencap -p > docs/screenshots/mobile-console.png
  ```
- Le badge « Données de démo » peut être masqué pour la capture ; la mention sous la galerie du
  README signale que les données sont fictives.
