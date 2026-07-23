/**
 * Couleur associée à un serveur, pour le différencier dans les onglets et cartes.
 * Persistée en localStorage. Réactif (rune `$state`).
 */

export const SERVER_COLORS: { key: string; hex: string }[] = [
  { key: 'emerald', hex: '#009b72' },
  { key: 'coral', hex: '#ff715b' },
  { key: 'violet', hex: '#7828c8' },
  { key: 'blue', hex: '#3b82f6' },
  { key: 'amber', hex: '#f59e0b' },
  { key: 'pink', hex: '#ec4899' },
  { key: 'teal', hex: '#06beb6' },
  { key: 'slate', hex: '#64748b' },
];

const STORAGE_KEY = 'mnstr-server-colors';

const store = $state<{ map: Record<number, string> }>({ map: {} });

export function initColors(): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) store.map = JSON.parse(raw) as Record<number, string>;
  } catch {
    /* localStorage illisible : on repart d'une map vide */
  }
}

export function serverColor(id: number): string | undefined {
  return store.map[id];
}

export function setServerColor(id: number, hex: string | null): void {
  if (hex) store.map[id] = hex;
  else delete store.map[id];
  localStorage.setItem(STORAGE_KEY, JSON.stringify(store.map));
}
