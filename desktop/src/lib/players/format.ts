/**
 * Mise en forme des joueurs déjà venus (vue Joueurs et fiche) : durées de jeu, dernière venue,
 * tête du joueur, erreurs de lecture des données joueur. Les données elles-mêmes (tri, filtres,
 * statistiques, décodage de l'inventaire) sont calculées côté Rust (`players.rs`).
 */

import { t, getLocale } from '#lib/i18n/index.ts';
import { isAppError, humanizeError } from '#lib/ipc.ts';

function locale(): string {
  return getLocale() === 'fr' ? 'fr-FR' : 'en-GB';
}

/** Temps de jeu compact : « 45 min », « 12 h 05 », « 1 234 h ». */
export function fmtPlayTime(secs: number | null): string {
  if (secs === null) return '—';
  const minutes = Math.floor(secs / 60);
  if (minutes < 60) return `${minutes} min`;
  const h = Math.floor(minutes / 60);
  if (h >= 100) return `${h.toLocaleString(locale())} h`;
  return `${h} h ${String(minutes % 60).padStart(2, '0')}`;
}

/** Dernière venue relative : « en ligne », « aujourd'hui », « il y a 3 j », « il y a 2 mois ». */
export function fmtLastSeen(ts: number | null, online = false): string {
  if (online) return t('players.badge.online');
  if (ts === null) return '—';
  const days = Math.floor((Date.now() / 1000 - ts) / 86_400);
  if (days <= 0) return t('players.seenToday');
  if (days < 60) return t('players.seenDaysAgo', { n: days });
  return t('players.seenMonthsAgo', { n: Math.floor(days / 30) });
}

/** Date et heure locales d'un horodatage Unix (secondes). */
export function fmtDateTime(ts: number | null): string {
  if (ts === null) return '—';
  return new Date(ts * 1000).toLocaleString(locale(), {
    day: '2-digit',
    month: '2-digit',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}

/** Nombre entier localisé. */
export function fmtInt(n: number): string {
  return n.toLocaleString(locale());
}

/** Distance en blocs, en kilomètres au-delà de 1 000. */
export function fmtDistance(blocks: number): string {
  if (blocks >= 1000) {
    return t('players.sheet.km', {
      n: (blocks / 1000).toLocaleString(locale(), { maximumFractionDigits: 1 }),
    });
  }
  return t('players.sheet.blocks', { n: Math.round(blocks).toLocaleString(locale()) });
}

/**
 * Tête du joueur (service d'images de MineStrator, celui du panel). Construite depuis le pseudo
 * AFFICHÉ : en mode démo, c'est celle du pseudo substitué.
 */
export function headUrl(name: string): string {
  return `https://head.sttr.io/face/${encodeURIComponent(name)}`;
}

/** Codes d'erreur de lecture des fichiers joueur (`API_PLAYERDATA_*`) traduits. */
const DATA_ERRORS = ['NOT_FOUND', 'VOLUME_UNREACHABLE', 'SCRIPT_UNAVAILABLE', 'UNSUPPORTED', 'TOO_LARGE'];

/** Message d'erreur d'une lecture de fiche ou d'inventaire. */
export function playerDataError(e: unknown): string {
  if (isAppError(e) && e.kind === 'api') {
    const code = DATA_ERRORS.find((c) => e.message.includes(`PLAYERDATA_${c}`));
    if (code) return t(`players.dataErrors.${code}`);
  }
  return humanizeError(e);
}
