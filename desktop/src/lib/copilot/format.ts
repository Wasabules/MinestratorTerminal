/** Helpers d'affichage partagés par les surfaces Copilote (icône, onglet, chat) et alertes. */

import { t } from '$lib/i18n';

/** Couleur d'un niveau de risque d'action (`safe`/`caution`/`danger`). */
export function riskColor(risk: string): string {
  if (risk === 'safe') return 'var(--state-ok, #3fb950)';
  if (risk === 'danger') return 'var(--state-danger)';
  return 'var(--state-pending)';
}

const TRIGGER_ICONS: Record<string, string> = {
  crash: 'skull',
  cpu: 'cpu',
  ram: 'memory-stick',
  disk: 'hard-drive',
  error: 'circle-alert',
  warn: 'triangle-alert',
  manual: 'play',
  selection: 'search',
  performance: 'zap',
};

/** Nom d'icône (voir Icon.svelte) d'un déclencheur de diagnostic. */
export function trigIcon(trigger: string): string {
  return TRIGGER_ICONS[trigger] ?? 'activity';
}

/** Libellé lisible d'un déclencheur (i18n). */
export function trigLabel(trigger: string): string {
  const map: Record<string, string> = {
    crash: t('copilot.trgCrash'),
    cpu: 'CPU',
    ram: 'RAM',
    disk: t('overview.disk'),
    error: 'ERROR',
    warn: 'WARN',
    manual: t('copilot.trgManual'),
    selection: t('copilot.trgSelection'),
    performance: t('copilot.trgPerformance'),
  };
  return map[trigger] ?? trigger;
}

/** « il y a … » relatif à `nowMs` (passé en paramètre pour rester réactif). */
export function ago(nowMs: number, ts: number): string {
  const d = Math.max(0, Math.floor(nowMs / 1000) - ts);
  if (d < 60) return t('common.justNow');
  if (d < 3600) return `${Math.floor(d / 60)} min`;
  if (d < 86400) return `${Math.floor(d / 3600)} h`;
  return `${Math.floor(d / 86400)} j`;
}

/** Durée écoulée depuis `startedMs` (format court). */
export function elapsed(nowMs: number, startedMs: number): string {
  const s = Math.max(0, Math.floor((nowMs - startedMs) / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}

/** Formatage d'octets en unités binaires (IEC : o/Kio/Mio/Gio, base 1024). */
export function fmtBytes(n: number): string {
  if (n <= 0) return '0';
  if (n >= 1024 ** 3) return `${(n / 1024 ** 3).toFixed(1)} Gio`;
  if (n >= 1024 ** 2) return `${(n / 1024 ** 2).toFixed(0)} Mio`;
  if (n >= 1024) return `${(n / 1024).toFixed(0)} Kio`;
  return `${n} o`;
}
