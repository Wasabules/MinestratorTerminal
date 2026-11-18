/**
 * Gestionnaire de transferts SFTP : suit les uploads/downloads/zip en cours (rune `$state`),
 * ainsi que les opérations de fichiers faites sur le serveur (compression, extraction),
 * alimenté par l'event `sftp://progress`. Global (survit aux changements de vue et d'onglet).
 */

import type { SftpProgress } from '#lib/types.ts';

/**
 * Un transfert suivi. `direction` (`up` | `down` | `server`) et `status` (`active` | `done` |
 * `error`) sont des chaînes côté Rust (`SftpProgress`) : on garde leur type généré plutôt qu'une
 * union qui promettrait plus que le core ne garantit.
 */
export interface Transfer {
  id: string;
  name: string;
  direction: SftpProgress['direction'];
  done: number;
  total: number;
  status: SftpProgress['status'];
  error: string | null;
  /** Étape d'une opération en plusieurs temps (compression sur le serveur, téléchargement…). */
  phase: string | null;
  /** Fichiers déjà traités par le serveur (compression). */
  files: number;
  /** Fichiers ignorés par le serveur (archive ou extraction partielle), connu à la fin. */
  skipped: number;
}

const MAX = 60;

const store = $state<{ items: Transfer[] }>({ items: [] });

/** Enregistre un transfert dès son lancement (avant le 1er event) pour un affichage immédiat. */
export function startTransfer(id: string, name: string, direction: 'up' | 'down' | 'server'): void {
  store.items.unshift({
    id,
    name,
    direction,
    done: 0,
    total: 0,
    status: 'active',
    error: null,
    phase: null,
    files: 0,
    skipped: 0,
  });
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
      phase: p.phase,
      files: p.files,
      skipped: p.skipped,
    });
    return;
  }
  // Une entrée annulée côté interface reste annulée : les derniers events du core (fin du
  // bloc en cours) ne doivent pas la ranimer.
  if (t.status !== 'active') return;
  t.status = p.status;
  t.error = p.error;
  if (p.status === 'active') {
    t.done = p.done;
    t.total = p.total;
    t.phase = p.phase;
    t.files = p.files;
  } else {
    t.skipped = p.skipped;
    // Fin : seule une phase finale explicite (ex. `still_running`) est gardée.
    t.phase = p.phase;
    if (p.status === 'done' && t.total > 0) t.done = t.total; // barre pleine
  }
}

/**
 * Marque un transfert comme annulé dans l'interface. L'arrêt côté Rust est coopératif : il
 * intervient au bloc suivant (ou, pour une compression, seul le suivi s'arrête).
 */
export function markCancelled(id: string, message: string): void {
  const t = store.items.find((x) => x.id === id);
  if (!t || t.status !== 'active') return;
  t.status = 'error';
  t.error = message;
  t.phase = null;
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
