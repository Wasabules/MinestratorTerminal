/** Gestion du thème clair/sombre, persistée en localStorage. Dark-first. */

const STORAGE_KEY = 'mnstr-theme';

export type Theme = 'dark' | 'light';

function current(): Theme {
  const attr = document.documentElement.getAttribute('data-theme');
  return attr === 'light' ? 'light' : 'dark';
}

/** Réapplique le thème enregistré (le script inline de app.html l'a déjà posé au boot). */
export function applyInitialTheme(): void {
  const saved = (localStorage.getItem(STORAGE_KEY) as Theme | null) ?? 'dark';
  document.documentElement.setAttribute('data-theme', saved);
}

/** Bascule le thème et renvoie le nouveau. */
export function toggleTheme(): Theme {
  const next: Theme = current() === 'dark' ? 'light' : 'dark';
  document.documentElement.setAttribute('data-theme', next);
  localStorage.setItem(STORAGE_KEY, next);
  return next;
}
