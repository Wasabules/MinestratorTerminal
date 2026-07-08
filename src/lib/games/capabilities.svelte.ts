/**
 * Capacités par jeu, côté front : store réactif keyé par `serverId`, alimenté depuis
 * `ServerListItem.capabilities` (calculé en Rust, voir `crate::games`). Sert à gater les
 * vues/fonctions exposées. Miroir de `src/lib/servers/runtime.svelte.ts`.
 */

import { api } from '$lib/ipc';
import { VIEWS } from '$lib/tabs/tabs.svelte';
import type { GameCapabilities, ServerListItem } from '$lib/types';

/** Capacités par défaut pour un jeu inconnu / non encore chargé (défaut sûr). */
export const GENERIC_CAPS: GameCapabilities = {
  family: 'generic',
  console_autocomplete: 'none',
  log_format: 'generic',
  players: false,
  mods: 'none',
  backups: true,
};

const store = $state<{ map: Record<number, GameCapabilities> }>({ map: {} });

/** Mémorise les capacités d'une liste de serveurs (à appeler après `api.listServers()`). */
export function rememberCaps(servers: ServerListItem[]): void {
  for (const s of servers) store.map[s.id] = s.capabilities;
}

/** Capacités connues d'un serveur (réactif ; `undefined` si jamais chargé). */
export function serverCaps(id: number): GameCapabilities | undefined {
  return store.map[id];
}

/**
 * Garantit la présence des capacités d'un serveur : si absentes (onglet « froid » ou fenêtre
 * détachée ouverte avant le chargement de l'accueil), interroge l'API une fois. Retombe sur
 * `GENERIC_CAPS` si la liste est indisponible.
 */
export async function ensureCaps(id: number): Promise<GameCapabilities> {
  const known = store.map[id];
  if (known) return known;
  try {
    const { servers } = await api.listServers();
    rememberCaps(servers);
  } catch {
    /* liste indisponible : on retombe sur le défaut */
  }
  return store.map[id] ?? GENERIC_CAPS;
}

/** Filtre la liste des vues selon les capacités (Joueurs/Mods conditionnels). */
export function availableViews(caps: GameCapabilities | undefined): typeof VIEWS {
  if (!caps) return VIEWS; // pas encore chargé : permissif (ne masque rien à tort)
  return VIEWS.filter((v) => {
    if (v.id === 'players') return caps.players;
    if (v.id === 'mods') return caps.mods !== 'none';
    return true;
  });
}
