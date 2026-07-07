/**
 * Préférence « à la fermeture de la fenêtre » : quand l'utilisateur clique sur la croix, faut-il
 * demander, réduire dans le tray (l'app continue en fond) ou quitter ? Choix purement UI, persisté
 * en localStorage. Lu au moment de la fermeture par CloseDialog.svelte.
 */
export type CloseBehavior = 'ask' | 'minimize' | 'quit';

const KEY = 'mnstr-close-behavior';

const store = $state<{ value: CloseBehavior }>({ value: 'ask' });

/** Charge la préférence depuis le localStorage (au démarrage). */
export function initCloseBehavior(): void {
  try {
    const v = localStorage.getItem(KEY);
    if (v === 'ask' || v === 'minimize' || v === 'quit') store.value = v;
  } catch {
    /* localStorage indisponible */
  }
}

/** Comportement courant (réactif). */
export function closeBehavior(): CloseBehavior {
  return store.value;
}

/** Met à jour et persiste la préférence. */
export function setCloseBehavior(b: CloseBehavior): void {
  store.value = b;
  try {
    localStorage.setItem(KEY, b);
  } catch {
    /* localStorage indisponible */
  }
}
