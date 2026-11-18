/**
 * Mises à jour des mods, plugins et datapacks, par serveur : une vérification partagée par
 * l'Aperçu (compteur) et la Marketplace (badges, application). Le core garde déjà le résultat
 * 15 minutes ; ce store évite en plus les appels simultanés de deux vues et republie le résultat
 * à toutes celles qui l'affichent.
 */

import { api, humanizeError } from '#lib/ipc.ts';
import type {
  ContentKind,
  ContentUpdate,
  ContentUpdates,
  UpdateReport,
  UpdateSelection,
} from '#lib/types.ts';

export interface ServerUpdates {
  data: ContentUpdates | null;
  loading: boolean;
  error: string | null;
}

/** Durée pendant laquelle une vérification affichée est réutilisée sans rappeler le core. */
const FRESH_SECS = 15 * 60;

const EMPTY: ServerUpdates = { data: null, loading: false, error: null };

const store = $state<{ map: Record<number, ServerUpdates> }>({ map: {} });
const inflight = new Map<number, Promise<void>>();

/** Réactif : état de la vérification d'un serveur. */
export function serverUpdates(serverId: number): ServerUpdates {
  return store.map[serverId] ?? EMPTY;
}

/** Réactif : nombre de mises à jour applicables d'un serveur (0 tant que rien n'est vérifié). */
export function updateCount(serverId: number): number {
  return store.map[serverId]?.data?.updates.length ?? 0;
}

/** Réactif : mise à jour d'un élément installé (clé = nom de fichier, ou nom d'un datapack). */
export function updateFor(serverId: number, kind: ContentKind, key: string): ContentUpdate | undefined {
  return store.map[serverId]?.data?.updates.find((u) => u.kind === kind && u.key === key);
}

/**
 * Vérifie les mises à jour d'un serveur. Sans `force`, un résultat récent est conservé tel quel ;
 * deux appels simultanés n'en font qu'un.
 */
export function loadUpdates(serverId: number, force = false): Promise<void> {
  const current = store.map[serverId];
  const age = current?.data ? Date.now() / 1000 - current.data.checked_at : Infinity;
  if (!force && age < FRESH_SECS) return Promise.resolve();
  const pending = inflight.get(serverId);
  if (pending) return pending;
  store.map[serverId] = { data: current?.data ?? null, loading: true, error: null };
  const run = api
    .contentUpdates(serverId, force)
    .then((data) => {
      store.map[serverId] = { data, loading: false, error: null };
    })
    .catch((e) => {
      store.map[serverId] = { data: current?.data ?? null, loading: false, error: humanizeError(e) };
    })
    .finally(() => inflight.delete(serverId));
  inflight.set(serverId, run);
  return run;
}

/** Applique des mises à jour puis revérifie le serveur (son contenu a changé). */
export async function applyUpdates(serverId: number, items: UpdateSelection[]): Promise<UpdateReport> {
  try {
    return await api.applyContentUpdates(serverId, items);
  } finally {
    void loadUpdates(serverId, true);
  }
}
