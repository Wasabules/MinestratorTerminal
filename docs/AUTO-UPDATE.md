# Auto-update — ACTIVÉ

Les mises à jour automatiques sont **actives**. Chaque app installée vérifie au démarrage la
présence d'une release plus récente sur GitHub, propose de l'installer (bandeau non intrusif),
vérifie la **signature** de l'artefact, puis installe et relance.

## Comment ça marche

| Élément | Fichier |
|---|---|
| Vérification au démarrage + bandeau | `desktop/src/lib/components/UpdateBanner.svelte` (monté dans `+layout.svelte`) |
| Helpers `checkForUpdate()` / `applyUpdate()` | `desktop/src/lib/updater.ts` |
| Plugin updater + process (relance) | `desktop/src-tauri/src/lib.rs`, `desktop/src-tauri/Cargo.toml`, `desktop/package.json` |
| Permissions `updater` + `process` | `desktop/src-tauri/capabilities/default.json` |
| Endpoint + clé publique | `desktop/src-tauri/tauri.conf.json` → `plugins.updater` |
| Artefacts signés | `bundle.createUpdaterArtifacts: true` |
| Signature en CI | `.github/workflows/release.yml` (secret `TAURI_SIGNING_PRIVATE_KEY`) |

Le check n'est fait que dans la **fenêtre principale** (`label === "main"`) pour éviter les doublons
dans les fenêtres détachées. En cas d'échec (hors-ligne, endpoint injoignable), rien ne s'affiche.

## Clé de signature

- Clé **publique** : dans `tauri.conf.json` (`plugins.updater.pubkey`) — publique, versionnée.
- Clé **privée** : fichier `updater-private.key` à la racine (**gitignoré**, `*.key`) **et** secret
  GitHub `TAURI_SIGNING_PRIVATE_KEY` (utilisé par la CI). Mot de passe **vide**.
- ⚠️ **Sauvegarde impérative** de `updater-private.key` (coffre / gestionnaire de mots de passe) :
  sans elle, plus aucune release ne peut être signée et l'auto-update casse. Si elle est perdue,
  il faut générer une nouvelle paire, republier `pubkey`, et les clients devront réinstaller à la main.

## Publier une mise à jour

1. **Bumper la version** dans les 3 fichiers (garder les 3 identiques) :
   - `desktop/src-tauri/tauri.conf.json` → `"version"`
   - `desktop/package.json` → `"version"`
   - `Cargo.toml` (racine) → `[workspace.package] version`
2. Committer, puis **taguer et pousser** :
   ```
   git tag v0.2.0 && git push origin v0.2.0
   ```
3. La CI build Windows/macOS/Linux, **signe** les artefacts, génère `latest.json` et crée une release
   **draft**.
4. **Revoir la release draft** sur GitHub → **Publish** (elle devient « latest »).
5. Les apps installées détectent la nouvelle version au prochain lancement → bandeau → un clic.

> L'endpoint `releases/latest/download/latest.json` ne résout que vers une release **publiée**
> (pas draft). Tant que la draft n'est pas publiée, personne ne voit la mise à jour.
