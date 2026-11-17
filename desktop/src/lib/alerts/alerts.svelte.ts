/** Centre d'alertes : accumule les alertes reçues du superviseur (rune `$state`). */

import type { Alert } from '$lib/types';
import { uid } from '$lib/util/id';

export interface AlertItem extends Alert {
  id: string;
  read: boolean;
}

const MAX = 100;

const store = $state<{ items: AlertItem[] }>({ items: [] });

/** Même alerte = même serveur, même type, même horodatage (relais du service + arriéré). */
function sameAlert(x: Alert, y: Alert): boolean {
  return x.server_id === y.server_id && x.kind === y.kind && x.ts === y.ts;
}

export function addAlert(a: Alert): void {
  if (store.items.some((i) => sameAlert(i, a))) return;
  store.items.unshift({ ...a, id: uid(), read: false });
  if (store.items.length > MAX) store.items.length = MAX;
}

/**
 * Intègre l'arriéré d'alertes du service d'arrière-plan (survenues app fermée) : rangées par
 * date, sans doublon, et sans notification (elles sont passées).
 */
export function addBacklog(list: Alert[]): void {
  const fresh = list.filter((a) => !store.items.some((i) => sameAlert(i, a)));
  if (fresh.length === 0) return;
  store.items = [...store.items, ...fresh.map((a) => ({ ...a, id: uid(), read: false }))]
    .sort((x, y) => y.ts - x.ts)
    .slice(0, MAX);
}

export function alertItems(): AlertItem[] {
  return store.items;
}

export function unreadCount(): number {
  return store.items.reduce((n, i) => n + (i.read ? 0 : 1), 0);
}

export function markAllRead(): void {
  for (const i of store.items) i.read = true;
}

export function clearAlerts(): void {
  store.items = [];
}
