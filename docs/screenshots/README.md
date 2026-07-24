# Captures d'écran du README

Dépose ici les visuels affichés dans le [`README`](../../README.md) principal.
**Garde les mêmes noms de fichiers** : ils apparaissent alors automatiquement.

| Fichier | Écran | État |
|---|---|---|
| `desktop-overview.png` | Desktop — vue d'ensemble (jauges + graphes) | ⬜ placeholder |
| `desktop-console.png` | Desktop — console temps réel | ⬜ placeholder |
| `desktop-sftp.png` | Desktop — SFTP / éditeur | ⬜ placeholder |
| `desktop-marketplace.png` | Desktop — marketplace de mods | ⬜ placeholder |
| `mobile-onboarding.png` | Mobile — connexion | ✅ réelle |
| `mobile-servers.png` | Mobile — serveurs (groupés par MyBox) | ✅ réelle |
| `mobile-overview.png` | Mobile — aperçu (jauges live) | ⬜ placeholder |
| `mobile-console.png` | Mobile — console | ⬜ placeholder |
| `mobile-files.png` | Mobile — fichiers / éditeur | ⬜ placeholder |
| `mobile-settings.png` | Mobile — réglages | ⬜ placeholder |

## Conseils

- **Desktop** : paysage, idéalement ≥ 1280×800, format PNG. Un thème sombre rend mieux (charte dark-first).
- **Mobile** : capture directe depuis l'appareil (portrait) :
  ```bash
  adb exec-out screencap -p > docs/screenshots/mobile-console.png
  ```
- Les placeholders gris (charte `#2c3038`) sont là juste pour réserver la place ; écrase-les.
