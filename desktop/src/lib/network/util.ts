/**
 * Aides de la vue Réseau (pures) : libellés, ordre des gravités, serveurs proposés aux
 * assistants. Les règles du réseau elles-mêmes (rapprochement, plans) vivent dans le cœur.
 */

import { t } from '#lib/i18n/index.ts';
import type {
  NetworkPlatform,
  NetworkProxy,
  NetworkServer,
  NetworkSeverity,
  NetworkStep,
  NetworkTopology,
} from '#lib/types.ts';

/** Gravités, de la plus forte à la plus faible. */
export const SEVERITY_ORDER: NetworkSeverity[] = ['critical', 'important', 'info'];

export function platformLabel(p: NetworkPlatform | null | undefined): string {
  return t(`network.platform.${p ?? 'unknown'}`);
}

/** Proxy repéré (ou plateforme de proxy annoncée par l'hébergeur). */
export function isProxyServer(s: NetworkServer): boolean {
  return s.proxy || s.platform === 'velocity' || s.platform === 'bungeecord';
}

/**
 * Serveurs de jeu possibles pour un proxy : Minecraft Java, hors proxys. Ceux de la MyBox du proxy
 * d'abord, puis par nom.
 */
export function backendCandidates(topo: NetworkTopology, proxyId: number): NetworkServer[] {
  const box = topo.servers.find((s) => s.id === proxyId)?.mybox_id ?? null;
  return topo.servers
    .filter((s) => s.id !== proxyId && !isProxyServer(s))
    .sort(
      (a, b) =>
        Number(b.mybox_id === box) - Number(a.mybox_id === box) ||
        a.name.localeCompare(b.name, undefined, { sensitivity: 'base' }),
    );
}

/** MyBox d'un serveur du compte (`null` : inconnue). */
export function myboxOf(topo: NetworkTopology, id: number): number | null {
  return topo.servers.find((s) => s.id === id)?.mybox_id ?? null;
}

/**
 * Serveurs de jeu déjà reliés à un proxy (entrées reconnues sur le compte), ceux de la liste de
 * repli d'abord, dans son ordre : relancer la liaison sur eux la rend conforme.
 */
export function linkedBackends(topo: NetworkTopology, proxy: NetworkProxy): number[] {
  const rank = (name: string) => {
    const i = proxy.try_order.findIndex((x) => x.toLowerCase() === name.toLowerCase());
    return i < 0 ? Number.MAX_SAFE_INTEGER : i;
  };
  const ids: number[] = [];
  const entries = proxy.entries
    .filter((e) => e.target && !e.target.self_proxy && !e.local)
    .sort((a, b) => rank(a.name) - rank(b.name));
  for (const e of entries) {
    const id = e.target!.server_id;
    const s = topo.servers.find((x) => x.id === id);
    if (s && !isProxyServer(s) && !ids.includes(id)) ids.push(id);
  }
  return ids;
}

/** Serveurs de la liste de repli actuelle d'un proxy (identifiants). */
export function fallbackBackends(proxy: NetworkProxy): number[] {
  return proxy.entries
    .filter((e) => e.target && proxy.try_order.some((x) => x.toLowerCase() === e.name.toLowerCase()))
    .map((e) => e.target!.server_id);
}

/** Étapes regroupées par serveur, dans l'ordre du plan (proxy d'abord). */
export function groupSteps(steps: NetworkStep[]): { serverId: number; server: string; steps: NetworkStep[] }[] {
  const out: { serverId: number; server: string; steps: NetworkStep[] }[] = [];
  for (const s of steps) {
    const g = out.find((x) => x.serverId === s.server_id);
    if (g) g.steps.push(s);
    else out.push({ serverId: s.server_id, server: s.server, steps: [s] });
  }
  return out;
}

/** Serveurs que l'application va modifier (étapes `change`). */
export function changedServers(steps: NetworkStep[]): number {
  return new Set(steps.filter((s) => s.status === 'change').map((s) => s.server_id)).size;
}

/** Classe d'une ligne de diff unifié. */
export function diffClass(line: string): string {
  if (line.startsWith('+++') || line.startsWith('---')) return 'meta';
  if (line.startsWith('@@')) return 'hunk';
  if (line.startsWith('+')) return 'add';
  if (line.startsWith('-')) return 'del';
  return '';
}

/** Nom d'entrée accepté par Velocity et BungeeCord (comme le cœur) ; vide : automatique. */
export function validEntryName(n: string): boolean {
  const v = n.trim();
  return v === '' || (/^[A-Za-z0-9_-]{1,32}$/.test(v) && v.toLowerCase() !== 'try');
}
