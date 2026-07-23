/** Centre de diagnostics : accumule les rapports du Copilote + suit les analyses en cours. */

import type { CopilotProgress, CopilotStarted, Diagnosis } from '$lib/types';

export interface DiagnosisItem extends Diagnosis {
  read: boolean;
  /** Étapes traversées pendant l'analyse (log/détail). */
  log: string[];
}

export interface ActiveRun {
  id: string;
  server_name: string;
  trigger: string;
  /** Étape courante. */
  phase: string;
  /** Historique des étapes. */
  log: string[];
  /** Horodatage de départ (ms, horloge locale). */
  started: number;
}

const MAX = 50;

const store = $state<{ items: DiagnosisItem[]; active: ActiveRun[] }>({ items: [], active: [] });

/** Une analyse démarre. */
export function startRun(s: CopilotStarted): void {
  // Évite les doublons si l'event arrive deux fois.
  if (store.active.some((a) => a.id === s.id)) return;
  store.active.push({
    id: s.id,
    server_name: s.server_name,
    trigger: s.trigger,
    phase: '',
    log: [],
    started: Date.now(),
  });
}

/** Étape d'avancement d'une analyse. */
export function progressRun(p: CopilotProgress): void {
  const run = store.active.find((a) => a.id === p.id);
  if (!run) return;
  run.phase = p.phase;
  // Évite les doublons consécutifs (Claude Code émet des étapes répétées).
  if (run.log[run.log.length - 1] !== p.phase) run.log.push(p.phase);
}

/** Le rapport final clôt l'analyse en cours correspondante (et récupère son log). */
export function addDiagnosis(d: Diagnosis): void {
  const idx = store.active.findIndex((a) => a.id === d.id);
  const log = idx >= 0 ? store.active[idx].log : [];
  if (idx >= 0) store.active.splice(idx, 1);
  store.items.unshift({ ...d, read: false, log });
  if (store.items.length > MAX) store.items.length = MAX;
}

export function diagnosisItems(): DiagnosisItem[] {
  return store.items;
}

export function activeRuns(): ActiveRun[] {
  return store.active;
}

export function activeCount(): number {
  return store.active.length;
}

export function unreadDiagnoses(): number {
  return store.items.reduce((n, i) => n + (i.read ? 0 : 1), 0);
}

export function markDiagnosesRead(): void {
  for (const i of store.items) i.read = true;
}

export function clearDiagnoses(): void {
  store.items = [];
}
