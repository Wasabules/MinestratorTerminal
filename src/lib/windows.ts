/**
 * Détachement d'onglets en fenêtres séparées.
 *
 * Chaque fenêtre est une instance indépendante de l'app (son propre gestionnaire
 * d'onglets). L'auth est partagée via le trousseau OS, donc une nouvelle fenêtre
 * s'authentifie seule. La spec de l'onglet à ouvrir est passée en paramètre d'URL.
 */

import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { ServerView } from './tabs/tabs.svelte';

export interface DetachSpec {
  serverId: number;
  serverName: string;
  view: ServerView;
}

let counter = 0;

/** Ouvre une nouvelle fenêtre affichant l'onglet décrit par `spec`. */
export async function detachTab(spec: DetachSpec): Promise<void> {
  const payload = encodeURIComponent(JSON.stringify(spec));
  const label = `detached-${spec.serverId}-${Date.now()}-${counter++}`;
  const win = new WebviewWindow(label, {
    url: `/?detach=${payload}`,
    title: `${spec.serverName} · ${spec.view}`,
    width: 1000,
    height: 700,
    minWidth: 720,
    minHeight: 480,
  });
  await new Promise<void>((resolve) => {
    void win.once('tauri://created', () => resolve());
    void win.once('tauri://error', () => resolve());
  });
}

/** Lit (et consomme) la spec de détachement depuis l'URL de la fenêtre courante. */
export function readDetachSpec(): DetachSpec | null {
  try {
    const raw = new URLSearchParams(location.search).get('detach');
    if (!raw) return null;
    const spec = JSON.parse(raw) as DetachSpec;
    history.replaceState({}, '', location.pathname); // nettoie l'URL
    return spec;
  } catch {
    return null;
  }
}

/** Le point écran (px CSS) est-il en dehors de la fenêtre courante ? */
export async function isPointerOutsideWindow(screenX: number, screenY: number): Promise<boolean> {
  try {
    const win = getCurrentWindow();
    const [pos, size, scale] = await Promise.all([
      win.outerPosition(),
      win.outerSize(),
      win.scaleFactor(),
    ]);
    const left = pos.x / scale;
    const top = pos.y / scale;
    const right = left + size.width / scale;
    const bottom = top + size.height / scale;
    return screenX < left || screenX > right || screenY < top || screenY > bottom;
  } catch {
    return false;
  }
}
