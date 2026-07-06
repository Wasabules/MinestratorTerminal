/**
 * Gestionnaire de transferts SFTP : suit les uploads/downloads/zip en cours (rune `$state`),
 * alimenté par l'event `sftp://progress`. Global (survit aux changements de vue et d'onglet).
 */

import type { SftpProgress } from '$lib/types';

export interface Transfer {
  id: string;
  name: string;
  direction: 'up' | 'down';
  done: number;
  total: number;
  status: 'active' | 'done' | 'error';
  error: string | null;
}

const MAX = 60;

const store = $state<{ items: Transfer[] }>({ items: [] });

/** Enregistre un transfert dès son lancement (avant le 1er event) pour un affichage immédiat. */
export function startTransfer(id: string, name: string, direction: 'up' | 'down'): void {
  store.items.unshift({ id, name, direction, done: 0, total: 0, status: 'active', error: null });
  if (store.items.length > MAX) store.items.length = MAX;
}

/** Met à jour depuis un event de progression (crée l'entrée si inconnue). */
export function applyProgress(p: SftpProgress): void {
  const t = store.items.find((x) => x.id === p.id);
  if (!t) {
    store.items.unshift({
      id: p.id,
      name: p.name,
      direction: p.direction,
      done: p.done,
      total: p.total,
      status: p.status,
      error: p.error,
    });
    return;
  }
  t.status = p.status;
  t.error = p.error;
  if (p.status === 'active') {
    t.done = p.done;
    t.total = p.total;
  } else if (p.status === 'done' && t.total > 0) {
    t.done = t.total; // barre pleine
  }
}

export function transferItems(): Transfer[] {
  return store.items;
}

export function activeCount(): number {
  return store.items.reduce((n, t) => n + (t.status === 'active' ? 1 : 0), 0);
}

export function clearFinished(): void {
  store.items = store.items.filter((t) => t.status === 'active');
}
