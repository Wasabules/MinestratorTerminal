/**
 * Auto-update — PRÉPARÉ, NON BRANCHÉ.
 *
 * Aucun code n'appelle ces fonctions pour l'instant : l'updater est inerte tant que la config
 * `plugins.updater` (endpoints + pubkey) n'est pas renseignée dans `src-tauri/tauri.conf.json` et
 * que `bundle.createUpdaterArtifacts` n'est pas activé.
 *
 * Pour activer, voir `docs/AUTO-UPDATE.md` : générer la clé de signature, renseigner la pubkey,
 * activer les artefacts, ajouter le secret CI, puis appeler `checkForUpdate()` au démarrage
 * (p. ex. depuis `+layout.svelte`, après l'init).
 */

import { check, type Update } from '@tauri-apps/plugin-updater';

/**
 * Vérifie la disponibilité d'une mise à jour, SANS l'installer.
 * Renvoie l'`Update` si disponible, `null` si à jour — ou si l'updater n'est pas encore configuré
 * (pubkey/endpoint absents) ou hors-ligne (échec silencieux : ne dérange pas l'utilisateur).
 */
export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch {
    return null;
  }
}

/**
 * Télécharge et installe une mise à jour déjà obtenue via {@link checkForUpdate}.
 * Selon la plateforme, l'app peut devoir être relancée ensuite (voir `@tauri-apps/plugin-process`).
 */
export async function applyUpdate(update: Update): Promise<void> {
  await update.downloadAndInstall();
}
