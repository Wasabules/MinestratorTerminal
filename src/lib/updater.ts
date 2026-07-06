/**
 * Auto-update (activé). L'app vérifie au démarrage la présence d'une release plus récente sur
 * GitHub (endpoint `latest.json` configuré dans `tauri.conf.json`). La signature de l'artefact est
 * vérifiée avec la clé publique intégrée — seule une release signée avec la clé privée est acceptée.
 * Voir `docs/AUTO-UPDATE.md`.
 */

import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

/**
 * Vérifie la disponibilité d'une mise à jour, SANS l'installer. Renvoie l'`Update` si disponible,
 * `null` si à jour, hors-ligne, ou si l'updater n'est pas joignable (échec silencieux : ne dérange
 * jamais l'utilisateur).
 */
export async function checkForUpdate(): Promise<Update | null> {
  try {
    return await check();
  } catch {
    return null;
  }
}

/** Télécharge et installe la mise à jour, puis relance l'application. */
export async function applyUpdate(update: Update): Promise<void> {
  await update.downloadAndInstall();
  await relaunch();
}
