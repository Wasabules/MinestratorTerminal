# Minestrator Terminal

Client desktop léger et rapide pour piloter ses serveurs Minecraft via l'[API MineStrator](https://api.minestrator.com/openapi-public-fr.yaml).
Multi-plateformes (Windows / macOS / Linux), clé API stockée dans le trousseau natif de l'OS.

- **Stack** : Tauri 2 (Rust) + SvelteKit (Svelte 5) + TypeScript. Tout le réseau (HTTP + WebSocket) vit côté Rust.
- **Architecture** : voir [`ARCHITECTURE-V1.md`](./ARCHITECTURE-V1.md).
- **Charte graphique** : voir [`DESIGN.md`](./DESIGN.md).

## État actuel — Jalon 1 « Socle » ✅

- Stockage sécurisé de la clé API (trousseau OS via `keyring`).
- Écran d'onboarding : saisie + validation de la clé.
- Appel `GET /user` → affichage du pseudo, du solde de crédits et du nombre de MyBox.
- Client API Rust générique (auth `Bearer base64`, désenveloppage, erreurs typées).
- Garde de navigation (onboarding ↔ accueil), thème clair/sombre.

## Prérequis

| Outil | Statut sur cette machine |
|---|---|
| Node.js ≥ 18 + npm | ✅ installé |
| WebView2 (Windows) | ✅ présent |
| **Rust** (via [rustup](https://rustup.rs)) | ⛔ **à installer** |
| **Build Tools MSVC** (C++) | à vérifier (requis par Tauri sous Windows) |

> Le **frontend** se build déjà sans Rust (`npm run build`). Pour lancer l'application desktop
> complète, installe Rust + les Build Tools C++ de Visual Studio, puis `npm run tauri dev`.

## Commandes

```bash
npm install            # dépendances frontend
npm run check          # typecheck (svelte-check)
npm run build          # build du frontend seul (sortie: build/)
npm run tauri dev      # lance l'app desktop (nécessite Rust)
npm run tauri build    # build de l'app packagée (nécessite Rust)
```

## Structure

```
src/                       # frontend SvelteKit
  app.css                  # tokens de design (charte)
  lib/
    ipc.ts                 # couche IPC typée (seul point d'appel de invoke)
    types.ts               # miroirs TS des modèles Rust
    theme.ts               # thème clair/sombre
    stores/auth.svelte.ts  # état d'auth (rune $state)
  routes/
    +layout.svelte         # garde d'auth + shell
    +page.svelte           # accueil (profil)
    onboarding/+page.svelte # saisie de la clé

src-tauri/src/             # cœur Rust
  api/mod.rs               # client HTTP typé
  secrets.rs               # trousseau OS
  commands.rs              # commandes IPC
  models.rs                # structs (dé)sérialisées
  error.rs                 # AppError (sérialisable vers le front)
  config.rs                # constantes
```

## Prochaines étapes (voir ARCHITECTURE-V1.md)

2. Liste des serveurs · 3. Dashboard + power actions · 4. WebSocket console (protocole Wings déjà validé) · 5. Console live · 6. Polish.
