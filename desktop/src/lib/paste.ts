/**
 * Export de logs / fichiers texte vers un service de paste public. L'upload (réseau) et
 * l'anonymisation sont faits côté cœur Rust ; ici on ne fait qu'appeler, ouvrir l'URL et la copier.
 */

import { openUrl } from '@tauri-apps/plugin-opener';
import { api } from './ipc';

/** Services proposés (MineStrator en premier : instance de l'hébergeur). */
export const PASTE_SERVICES: { id: string; label: string }[] = [
  { id: 'minestrator', label: 'MineStrator' },
  { id: 'mclogs', label: 'mclo.gs' },
  { id: 'pastesdev', label: 'pastes.dev' },
];

/** Publie `content`, ouvre l'URL dans le navigateur et la copie dans le presse-papier. Renvoie l'URL. */
export async function pasteExport(service: string, content: string): Promise<string> {
  const url = await api.pasteUpload(service, content);
  await openUrl(url).catch(() => {
    /* openUrl indisponible : l'URL reste copiée */
  });
  await navigator.clipboard.writeText(url).catch(() => {
    /* presse-papier indisponible */
  });
  return url;
}
