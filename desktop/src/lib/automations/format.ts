/**
 * Mise en forme des automatisations (récurrences, actions, dates, durées) et brouillon d'édition
 * d'une tâche. Purement front : la validation qui fait foi est côté Rust (`normalize_task`), ce
 * module ne fait que guider la saisie.
 */

import { t, getLocale } from '#lib/i18n/index.ts';
import type { Schedule, ScheduledTask, TaskAction } from '#lib/types.ts';

function pad2(n: number): string {
  return String(n).padStart(2, '0');
}

function locale(): string {
  return getLocale() === 'fr' ? 'fr-FR' : 'en-GB';
}

/** `HH:MM` d'une heure et d'une minute. */
export function hhmm(hour: number, minute: number): string {
  return `${pad2(hour)}:${pad2(minute)}`;
}

/** Résumé lisible d'une récurrence (« Tous les jours à 04:00 », « Lun, Ven à 20:00 »…). */
export function scheduleSummary(s: Schedule): string {
  switch (s.kind) {
    case 'daily':
      return t('automations.summaryDaily', { time: hhmm(s.hour, s.minute) });
    case 'weekly':
      return t('automations.summaryWeekly', {
        days: s.days.map((d) => t(`automations.day.${d}`)).join(', '),
        time: hhmm(s.hour, s.minute),
      });
    case 'interval':
      return t('automations.summaryInterval', { n: s.every_minutes });
  }
}

/** Résumé lisible d'une action. */
export function actionSummary(a: TaskAction): string {
  switch (a.type) {
    case 'restart':
      return a.warnings.length > 0
        ? t('automations.summaryRestartWarn', { list: a.warnings.join(', ') })
        : t('automations.summaryRestart');
    case 'snapshot':
      return t('automations.summarySnapshot');
    case 'commands':
      return t('automations.summaryCommands', { n: a.commands.length });
  }
}

/** Icône (Icon.svelte) d'une action. */
export function actionIcon(type: TaskAction['type'] | 'auto_restart'): string {
  switch (type) {
    case 'restart':
    case 'auto_restart':
      return 'refresh-cw';
    case 'snapshot':
      return 'hard-drive';
    case 'commands':
      return 'terminal';
  }
}

/** Date et heure locales courtes d'un horodatage Unix (secondes). */
export function fmtDateTime(ts: number): string {
  return new Date(ts * 1000).toLocaleString(locale(), {
    weekday: 'short',
    day: '2-digit',
    month: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** Date courte (sans heure). */
export function fmtDate(ts: number): string {
  return new Date(ts * 1000).toLocaleDateString(locale(), { day: '2-digit', month: '2-digit', year: 'numeric' });
}

/** Durée compacte : « 45 s », « 12 min », « 3 h 05 », « 2 j 4 h ». */
export function fmtDuration(secs: number): string {
  const s = Math.max(0, Math.round(secs));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.round(s / 60)} min`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ${pad2(Math.floor((s % 3600) / 60))}`;
  const day = getLocale() === 'fr' ? 'j' : 'd';
  return `${Math.floor(s / 86400)} ${day} ${Math.floor((s % 86400) / 3600)} h`;
}

/** Brouillon d'édition d'une tâche (champs de formulaire). */
export interface TaskDraft {
  id: string;
  name: string;
  enabled: boolean;
  action: TaskAction['type'];
  warnings: string;
  message: string;
  snapshotName: string;
  commands: string;
  schedule: Schedule['kind'];
  time: string;
  days: number[];
  every: number;
}

/** Brouillon d'une nouvelle tâche : redémarrage quotidien à 04:00, annonces à 10, 5 et 1 min. */
export function newDraft(): TaskDraft {
  return {
    id: '',
    name: '',
    enabled: true,
    action: 'restart',
    warnings: '10, 5, 1',
    message: t('automations.defaultMessage'),
    snapshotName: '',
    commands: '',
    schedule: 'daily',
    time: '04:00',
    days: [1, 2, 3, 4, 5, 6, 7],
    every: 360,
  };
}

/** Tâche existante → brouillon. */
export function draftFrom(task: ScheduledTask): TaskDraft {
  const d = newDraft();
  d.id = task.id;
  d.name = task.name;
  d.enabled = task.enabled;
  d.action = task.action.type;
  if (task.action.type === 'restart') {
    d.warnings = task.action.warnings.join(', ');
    d.message = task.action.message;
  } else if (task.action.type === 'snapshot') {
    d.snapshotName = task.action.name;
  } else {
    d.commands = task.action.commands.join('\n');
  }
  d.schedule = task.schedule.kind;
  if (task.schedule.kind === 'interval') {
    d.every = task.schedule.every_minutes;
  } else {
    d.time = hhmm(task.schedule.hour, task.schedule.minute);
    if (task.schedule.kind === 'weekly') d.days = [...task.schedule.days];
  }
  return d;
}

/** Minutes d'annonce saisies (« 10, 5, 1 ») ; `null` si une valeur est invalide. */
export function parseWarnings(text: string): number[] | null {
  const parts = text.split(/[,;\s]+/).filter((p) => p !== '');
  const out: number[] = [];
  for (const p of parts) {
    const n = Number(p);
    if (!Number.isInteger(n) || n < 1 || n > 60) return null;
    out.push(n);
  }
  return [...new Set(out)].sort((a, b) => b - a);
}

/**
 * Brouillon → tâche à envoyer au cœur, ou clé i18n de l'erreur de saisie. `account_id` et
 * `updated_at` sont attribués par le cœur. `canAnnounce` : le jeu sait diffuser un message (sinon
 * aucune annonce n'est enregistrée — elle ne ferait que retarder le démarrage de la tâche).
 */
export function draftToTask(
  d: TaskDraft,
  serverId: number,
  canAnnounce: boolean,
): ScheduledTask | string {
  let action: TaskAction;
  if (d.action === 'restart') {
    const warnings = canAnnounce ? parseWarnings(d.warnings) : [];
    if (warnings === null) return 'automations.invalidWarnings';
    action = { type: 'restart', warnings, message: canAnnounce ? d.message.trim() : '' };
  } else if (d.action === 'snapshot') {
    action = { type: 'snapshot', name: d.snapshotName.trim() };
  } else {
    const commands = d.commands
      .split('\n')
      .map((c) => c.trim())
      .filter((c) => c !== '');
    if (commands.length === 0) return 'automations.invalidCommands';
    action = { type: 'commands', commands };
  }
  const [h, m] = d.time.split(':').map(Number);
  const hour = Number.isInteger(h) ? h : 0;
  const minute = Number.isInteger(m) ? m : 0;
  let schedule: Schedule;
  if (d.schedule === 'daily') {
    schedule = { kind: 'daily', hour, minute };
  } else if (d.schedule === 'weekly') {
    if (d.days.length === 0) return 'automations.invalidDays';
    schedule = { kind: 'weekly', days: [...d.days].sort((a, b) => a - b), hour, minute };
  } else {
    if (!Number.isInteger(d.every) || d.every < 5) return 'automations.invalidInterval';
    schedule = { kind: 'interval', every_minutes: d.every };
  }
  return {
    id: d.id,
    account_id: 0,
    server_id: serverId,
    name: d.name.trim(),
    enabled: d.enabled,
    action,
    schedule,
    updated_at: 0,
  };
}
