/**
 * Mode « debug » développeur : quand actif, l'app affiche des infos techniques sur les vues
 * (egg brut, capacités calculées, source de mods, id de serveur…). Préférence purement UI,
 * persistée en localStorage. Réglable dans Paramètres → Debug.
 */

const KEY = 'mnstr-debug';

const store = $state<{ on: boolean }>({ on: false });

/** Charge la préférence au démarrage. */
export function initDebug(): void {
  try {
    store.on = localStorage.getItem(KEY) === '1';
  } catch {
    /* localStorage indisponible */
  }
}

/** Le mode debug est-il actif ? (réactif) */
export function isDebug(): boolean {
  return store.on;
}

/** Active/désactive le mode debug (persisté). */
export function setDebug(on: boolean): void {
  store.on = on;
  try {
    localStorage.setItem(KEY, on ? '1' : '0');
  } catch {
    /* localStorage indisponible */
  }
}
