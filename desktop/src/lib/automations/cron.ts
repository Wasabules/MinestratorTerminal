/**
 * Expressions cron des tâches planifiées du panel MineStrator : description lisible et modèles de
 * saisie. Le cœur valide chaque champ avant envoi (`panel_schedules.rs`) ; ici, on ne fait que
 * présenter et proposer.
 */
import type { PanelCron } from '#lib/types.ts';

type T = (key: string, params?: Record<string, string | number>) => string;

const isNum = (v: string) => /^\d+$/.test(v);
const pad = (v: string) => v.padStart(2, '0');

/** Noms des jours (0 = dimanche, 7 = dimanche aussi) dans la langue de l'interface. */
function dayName(n: number, locale: string): string {
  const base = new Date(Date.UTC(2024, 0, 7 + (n % 7))); // 7 janvier 2024 = dimanche
  return new Intl.DateTimeFormat(locale, { weekday: 'long', timeZone: 'UTC' }).format(base);
}

/** Liste de jours `1,3,5` ou plage `1-5` → noms ; `null` si la forme n'est pas reconnue. */
function days(dow: string, locale: string): string | null {
  const out: number[] = [];
  for (const part of dow.split(',')) {
    const m = part.match(/^(\d)(?:-(\d))?$/);
    if (!m) return null;
    const a = Number(m[1]);
    const b = m[2] ? Number(m[2]) : a;
    for (let d = a; d <= b; d++) out.push(d);
  }
  return [...new Set(out.map((d) => d % 7))].map((d) => dayName(d, locale)).join(', ');
}

/** Description lisible d'une planification ; à défaut, l'expression cron brute. */
export function describeCron(c: PanelCron, t: T, locale: string): string {
  const raw = `${c.minute} ${c.hour} ${c.day_of_month} ${c.month} ${c.day_of_week}`;
  const everyDay = c.day_of_month === '*' && c.month === '*';
  if (raw === '* * * * *') return t('panelSchedules.cron.everyMinute');
  const step = c.minute.match(/^\*\/(\d+)$/);
  if (step && c.hour === '*' && everyDay && c.day_of_week === '*') {
    return t('panelSchedules.cron.everyNMinutes', { n: step[1] });
  }
  if (isNum(c.minute) && c.hour === '*' && everyDay && c.day_of_week === '*') {
    return t('panelSchedules.cron.hourly', { minute: pad(c.minute) });
  }
  if (isNum(c.minute) && isNum(c.hour) && everyDay) {
    const time = `${pad(c.hour)}:${pad(c.minute)}`;
    if (c.day_of_week === '*') return t('panelSchedules.cron.daily', { time });
    const d = days(c.day_of_week, locale);
    if (d) return t('panelSchedules.cron.weekly', { days: d, time });
  }
  return t('panelSchedules.cron.custom', { cron: raw });
}

/** Modèles proposés à la saisie. `custom` laisse éditer les cinq champs. */
export type CronPreset = 'daily' | 'weekly' | 'hourly' | 'custom';

/** Planification d'un modèle. `days` : jours ISO de la semaine (1 = lundi … 7 = dimanche). */
export function presetCron(preset: CronPreset, time: string, days: number[], minute: number): PanelCron | null {
  const [h, m] = time.split(':').map(Number);
  switch (preset) {
    case 'daily':
      return { minute: String(m), hour: String(h), day_of_month: '*', month: '*', day_of_week: '*' };
    case 'weekly':
      if (days.length === 0) return null;
      return {
        minute: String(m),
        hour: String(h),
        day_of_month: '*',
        month: '*',
        day_of_week: days.length === 7 ? '*' : [...days].sort((a, b) => a - b).map((d) => d % 7).join(','),
      };
    case 'hourly':
      return { minute: String(minute), hour: '*', day_of_month: '*', month: '*', day_of_week: '*' };
    default:
      return null;
  }
}

/** Modèle qui correspond à une planification existante (pour la rééditer). */
export function detectPreset(c: PanelCron): { preset: CronPreset; time: string; days: number[]; minute: number } {
  const everyDay = c.day_of_month === '*' && c.month === '*';
  if (isNum(c.minute) && c.hour === '*' && everyDay && c.day_of_week === '*') {
    return { preset: 'hourly', time: '04:00', days: [], minute: Number(c.minute) };
  }
  if (isNum(c.minute) && isNum(c.hour) && everyDay) {
    const time = `${pad(c.hour)}:${pad(c.minute)}`;
    if (c.day_of_week === '*') return { preset: 'daily', time, days: [], minute: 0 };
    if (/^[0-7](,[0-7])*$/.test(c.day_of_week)) {
      const ds = [...new Set(c.day_of_week.split(',').map((d) => (Number(d) % 7) || 7))];
      return { preset: 'weekly', time, days: ds, minute: 0 };
    }
  }
  return { preset: 'custom', time: '04:00', days: [], minute: 0 };
}
