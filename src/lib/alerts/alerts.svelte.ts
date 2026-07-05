/** Centre d'alertes : accumule les alertes reçues du superviseur (rune `$state`). */

import type { Alert } from '$lib/types';
import { uid } from '$lib/util/id';

export interface AlertItem extends Alert {
  id: string;
  read: boolean;
}

const MAX = 100;

const store = $state<{ items: AlertItem[] }>({ items: [] });

export function addAlert(a: Alert): void {
  store.items.unshift({ ...a, id: uid(), read: false });
  if (store.items.length > MAX) store.items.length = MAX;
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
