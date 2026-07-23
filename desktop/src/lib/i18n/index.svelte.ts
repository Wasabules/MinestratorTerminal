/**
 * i18n natif, minimal et réactif.
 *
 * `t('a.b.c', { name })` lit la locale courante (rune `$state`) → tout appel de `t`
 * dans un composant se réévalue automatiquement au changement de langue.
 * Repli : locale courante → français → la clé brute.
 */

import { fr, type Dict } from './fr';
import { en } from './en';

export type Locale = 'fr' | 'en';

export const LOCALES: { code: Locale; label: string }[] = [
  { code: 'fr', label: 'Français' },
  { code: 'en', label: 'English' },
];

const DICTS: Record<Locale, Dict> = { fr, en };
const STORAGE_KEY = 'mnstr-locale';

const state = $state<{ locale: Locale }>({ locale: 'fr' });

export function initLocale(): void {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved === 'fr' || saved === 'en') {
    state.locale = saved;
  } else {
    state.locale = navigator.language?.toLowerCase().startsWith('en') ? 'en' : 'fr';
  }
}

export function getLocale(): Locale {
  return state.locale;
}

export function setLocale(locale: Locale): void {
  state.locale = locale;
  localStorage.setItem(STORAGE_KEY, locale);
}

function lookup(dict: Dict, path: string): string | undefined {
  let cur: unknown = dict;
  for (const part of path.split('.')) {
    if (cur && typeof cur === 'object' && part in (cur as Record<string, unknown>)) {
      cur = (cur as Record<string, unknown>)[part];
    } else {
      return undefined;
    }
  }
  return typeof cur === 'string' ? cur : undefined;
}

export function t(key: string, params?: Record<string, string | number>): string {
  const dict = DICTS[state.locale]; // lecture réactive
  let value = lookup(dict, key) ?? lookup(fr, key) ?? key;
  if (params) {
    for (const [name, val] of Object.entries(params)) {
      value = value.split(`{${name}}`).join(String(val));
    }
  }
  return value;
}
