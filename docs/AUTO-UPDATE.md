# Auto-update — état & activation

L'infrastructure de mise à jour automatique est **préparée mais NON activée**. Rien ne vérifie
de mise à jour au démarrage ; l'app se comporte comme si l'updater n'existait pas.

## Ce qui est déjà en place

| Élément | Fichier | État |
|---|---|---|
| Plugin Rust `tauri-plugin-updater` | `src-tauri/Cargo.toml` | ajouté |
| Enregistrement du plugin (desktop) | `src-tauri/src/lib.rs` (`setup`) | actif mais inerte |
| Plugin JS `@tauri-apps/plugin-updater` | `package.json` | ajouté |
| Permission `updater:default` | `src-tauri/capabilities/default.json` | ajoutée |
| Config `plugins.updater` | `src-tauri/tauri.conf.json` | **placeholder** (`pubkey: ""`) |
| Helpers front `checkForUpdate()` / `applyUpdate()` | `src/lib/updater.ts` | prêts, **non appelés** |

Tant que `pubkey` est vide et qu'on n'appelle jamais `checkForUpdate()`, l'updater ne fait rien.

## Activer (checklist)

1. **Générer la clé de signature** (une fois) :
   ```
   npm run tauri signer generate -- -w ~/.tauri/minestrator-terminal.key
   ```
   Garde **la clé privée hors du dépôt** (elle est déjà couverte par `.gitignore` si placée hors du
   projet). Note le mot de passe choisi.

2. **Renseigner la clé publique** dans `src-tauri/tauri.conf.json` → `plugins.updater.pubkey`
   (la valeur affichée par la commande ci-dessus).

3. **Activer la génération des artefacts signés** dans `src-tauri/tauri.conf.json` :
   ```json
   "bundle": { "createUpdaterArtifacts": true, ... }
   ```

4. **Ajouter les secrets CI** (repo → Settings → Secrets → Actions) puis les passer à
   `tauri-apps/tauri-action` dans `.github/workflows/release.yml` :
   ```yaml
   env:
     TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
     TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
   ```
   La CI produira alors, en plus des installeurs, les fichiers `.sig` et un `latest.json`.

5. **Vérifier l'endpoint** (`plugins.updater.endpoints`). Le placeholder pointe vers
   `releases/latest/download/latest.json`.
   ⚠️ **Repo privé** : l'endpoint GitHub exige une authentification — soit publier les binaires sur
   un hébergement public/CDN, soit servir `latest.json` derrière une URL autorisée, soit rendre le
   dépôt (ou juste les releases) public. À trancher au moment de l'activation.

6. **Brancher le check au démarrage** — p. ex. dans `src/routes/+layout.svelte`, après l'init :
   ```ts
   import { checkForUpdate, applyUpdate } from '$lib/updater';
   // ... dans onMount, sans bloquer :
   const update = await checkForUpdate();
   if (update) {
     // proposer la MàJ à l'utilisateur, puis :
     await applyUpdate(update);
     // relancer via @tauri-apps/plugin-process si nécessaire.
   }
   ```

## Publier une mise à jour

Une fois activé : incrémenter la version (`tauri.conf.json`, `package.json`, `Cargo.toml`), taguer
`vX.Y.Z` et pousser le tag. La CI build, signe et publie `latest.json` — les clients installés le
détecteront au prochain `checkForUpdate()`.
