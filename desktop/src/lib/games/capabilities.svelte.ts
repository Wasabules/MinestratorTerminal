/**
 * Capacités par jeu, côté front : store réactif keyé par `serverId`, alimenté depuis
 * `ServerListItem.capabilities` (calculé en Rust, voir `crate::games`). Sert à gater les
 * vues/fonctions exposées. Miroir de `src/lib/servers/runtime.svelte.ts`.
 *
 * Le même store retient l'**entrée de liste** complète : elle porte des métadonnées que seule
 * la liste fournit (avertissements de power action, note, dernier backup, réf. support), et les
 * vues serveur n'ont sinon aucun moyen de les obtenir sans refaire un appel.
 */

import { api } from '#lib/ipc.ts';
import { VIEWS } from '#lib/tabs/tabs.svelte.ts';
import type { GameCapabilities, ServerListItem } from '#lib/types.ts';

/** Capacités par défaut pour un jeu inconnu / non encore chargé (défaut sûr). */
export const GENERIC_CAPS: GameCapabilities = {
  family: 'generic',
  console_autocomplete: 'none',
  log_format: 'generic',
  players: false,
  mods: 'none',
  backups: true,
  properties: false,
  broadcast: null,
};

const store = $state<{
  map: Record<number, GameCapabilities>;
  items: Record<number, ServerListItem>;
}>({ map: {}, items: {} });

/** Mémorise les capacités d'une liste de serveurs (à appeler après `api.listServers()`). */
export function rememberCaps(servers: ServerListItem[]): void {
  for (const s of servers) {
    store.map[s.id] = s.capabilities;
    store.items[s.id] = s;
  }
}

/** Capacités connues d'un serveur (réactif ; `undefined` si jamais chargé). */
export function serverCaps(id: number): GameCapabilities | undefined {
  return store.map[id];
}

/** Entrée de liste connue d'un serveur (réactif ; `undefined` si jamais chargée). */
export function serverItem(id: number): ServerListItem | undefined {
  return store.items[id];
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
    if (v.id === 'settings') return caps.properties;
    return true;
  });
}
