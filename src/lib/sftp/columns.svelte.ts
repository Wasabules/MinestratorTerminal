/**
 * État de l'explorateur SFTP : colonnes visibles + tri. Persisté en localStorage.
 * La colonne « Nom » est toujours affichée ; les autres sont optionnelles.
 */

import type { SftpEntry } from '$lib/types';

export type SftpColumnId = 'size' | 'type' | 'modified';
export type SftpSortKey = 'name' | 'size' | 'type' | 'modified';
export type SortDir = 'asc' | 'desc';

export const OPTIONAL_COLUMNS: SftpColumnId[] = ['size', 'type', 'modified'];

export const COLUMN_LABEL: Record<SftpColumnId, string> = {
  size: 'sftp.colSize',
  type: 'sftp.colType',
  modified: 'sftp.colModified',
};

const STORAGE_KEY = 'mnstr-sftp-view';

const state = $state<{
  visible: Record<SftpColumnId, boolean>;
  sortKey: SftpSortKey;
  sortDir: SortDir;
}>({
  visible: { size: true, type: false, modified: true },
  sortKey: 'name',
  sortDir: 'asc',
});

export function initSftpView(): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as Partial<typeof state>;
    if (parsed.visible) state.visible = { ...state.visible, ...parsed.visible };
    if (parsed.sortKey) state.sortKey = parsed.sortKey;
    if (parsed.sortDir) state.sortDir = parsed.sortDir;
  } catch {
    /* localStorage illisible : valeurs par défaut */
  }
}

function persist(): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

export function isColVisible(id: SftpColumnId): boolean {
  return state.visible[id];
}
export function toggleCol(id: SftpColumnId): void {
  state.visible[id] = !state.visible[id];
  persist();
}
export function sortKey(): SftpSortKey {
  return state.sortKey;
}
export function sortDir(): SortDir {
  return state.sortDir;
}
export function setSort(key: SftpSortKey): void {
  if (state.sortKey === key) {
    state.sortDir = state.sortDir === 'asc' ? 'desc' : 'asc';
  } else {
    state.sortKey = key;
    state.sortDir = 'asc';
  }
  persist();
}

function ext(e: SftpEntry): string {
  const i = e.name.lastIndexOf('.');
  return i > 0 ? e.name.slice(i + 1).toLowerCase() : '';
}

/** Tri : dossiers toujours en premier, puis selon la colonne et la direction. */
export function sortEntries(entries: SftpEntry[], key: SftpSortKey, dir: SortDir): SftpEntry[] {
  const factor = dir === 'asc' ? 1 : -1;
  const byName = (a: SftpEntry, b: SftpEntry) =>
    a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });

  return [...entries].sort((a, b) => {
    if (a.is_dir !== b.is_dir) return a.is_dir ? -1 : 1;
    let cmp = 0;
    if (key === 'name') cmp = byName(a, b);
    else if (key === 'size') cmp = a.size - b.size;
    else if (key === 'modified') cmp = (a.modified ?? 0) - (b.modified ?? 0);
    else if (key === 'type') cmp = ext(a).localeCompare(ext(b)) || byName(a, b);
    return cmp * factor;
  });
}
