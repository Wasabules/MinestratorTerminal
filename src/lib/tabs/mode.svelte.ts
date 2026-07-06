/**
 * Mode d'affichage des onglets : **compact** (icônes de vue seules ; le nom du serveur reste visible
 * via le chip de groupe ou l'onglet solo) vs normal. Persisté en localStorage. Réactif (rune `$state`),
 * miroir de `servers/colors.svelte.ts`.
 */

const KEY = 'mnstr-tabs-compact';

const store = $state<{ compact: boolean }>({ compact: false });

export function initTabMode(): void {
  try {
    store.compact = localStorage.getItem(KEY) === '1';
  } catch {
    /* localStorage illisible : on reste en mode normal */
  }
}

export function isCompactTabs(): boolean {
  return store.compact;
}

export function setCompactTabs(on: boolean): void {
  store.compact = on;
  try {
    localStorage.setItem(KEY, on ? '1' : '0');
  } catch {
    /* localStorage indisponible */
  }
}
