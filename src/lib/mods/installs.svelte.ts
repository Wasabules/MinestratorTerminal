/**
 * Installations de mods Satisfactory : deux notions GLOBALES (survivent aux changements d'onglet).
 *  - le **panier** par serveur : mods choisis (avec leur version) pas encore lancés ;
 *  - les **installations** (en cours / terminées) : alimentées par l'event `mods://install-progress`.
 * Calqué sur `src/lib/transfers/transfers.svelte.ts` (store rune + abonnement central dans Workspace).
 */

import type { ModInstallProgress } from '$lib/types';

/** Un mod ajouté au panier d'un serveur (référence + version choisie + nom lisible). */
export interface CartItem {
  reference: string;
  versionId: string;
  name: string;
}

/** Une installation (un lot de 1+ mods) suivie de bout en bout. */
export interface InstallRun {
  id: string;
  serverId: number;
  serverName: string;
  label: string;
  /** resolving | downloading | stopping | uploading | restarting | done | error */
  phase: string;
  done: number;
  total: number;
  status: 'active' | 'done' | 'error';
  error: string | null;
}

const MAX_RUNS = 40;

const store = $state<{ cart: Record<number, CartItem[]>; runs: InstallRun[] }>({
  cart: {},
  runs: [],
});

// --- Panier -------------------------------------------------------------

/** Ajoute (ou met à jour la version d'un) mod au panier du serveur. */
export function addToCart(serverId: number, item: CartItem): void {
  const list = store.cart[serverId] ?? [];
  const existing = list.find((c) => c.reference === item.reference);
  if (existing) existing.versionId = item.versionId;
  else list.push(item);
  store.cart[serverId] = list;
}

export function removeFromCart(serverId: number, reference: string): void {
  store.cart[serverId] = (store.cart[serverId] ?? []).filter((c) => c.reference !== reference);
}

export function clearCart(serverId: number): void {
  store.cart[serverId] = [];
}

/** Réactif : contenu du panier d'un serveur. */
export function cartItems(serverId: number): CartItem[] {
  return store.cart[serverId] ?? [];
}

export function inCart(serverId: number, reference: string): boolean {
  return (store.cart[serverId] ?? []).some((c) => c.reference === reference);
}

// --- Installations (runs) ----------------------------------------------

/** Enregistre une installation dès son lancement (avant le 1er event) — affichage immédiat. */
export function startRun(
  id: string,
  serverId: number,
  serverName: string,
  label: string,
  total: number
): void {
  store.runs.unshift({
    id,
    serverId,
    serverName,
    label,
    phase: 'resolving',
    done: 0,
    total,
    status: 'active',
    error: null,
  });
  if (store.runs.length > MAX_RUNS) store.runs.length = MAX_RUNS;
}

/** Met à jour une installation depuis un event de progression (crée l'entrée si inconnue). */
export function applyInstallProgress(p: ModInstallProgress): void {
  const r = store.runs.find((x) => x.id === p.id);
  if (!r) {
    store.runs.unshift({
      id: p.id,
      serverId: 0,
      serverName: '',
      label: p.mod_name,
      phase: p.phase,
      done: p.done,
      total: p.total,
      status: p.status as InstallRun['status'],
      error: p.error,
    });
    return;
  }
  r.phase = p.phase;
  r.status = p.status as InstallRun['status'];
  r.error = p.error;
  if (p.total > 0) r.total = p.total;
  r.done = p.status === 'done' && r.total > 0 ? r.total : p.done;
}

/** Réactif : toutes les installations (récentes d'abord). */
export function runItems(): InstallRun[] {
  return store.runs;
}

export function activeRunCount(): number {
  return store.runs.reduce((n, r) => n + (r.status === 'active' ? 1 : 0), 0);
}

/** Une installation est-elle en cours sur ce serveur ? (bloque une 2e install concurrente). */
export function hasActiveRun(serverId: number): boolean {
  return store.runs.some((r) => r.serverId === serverId && r.status === 'active');
}

export function clearFinishedRuns(): void {
  store.runs = store.runs.filter((r) => r.status === 'active');
}
