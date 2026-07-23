/**
 * État d'exécution live (running/starting/stopping/offline) par serveur, pour la pastille
 * des onglets. Alimenté par les events console du superviseur (`mon:{id}`, actif par défaut)
 * et des vues ouvertes. Réactif (rune `$state`), miroir de `colors.svelte.ts`.
 */

const store = $state<{ map: Record<number, string> }>({ map: {} });

/** Enregistre le dernier état d'exécution connu d'un serveur. */
export function setServerRuntime(id: number, state: string): void {
  store.map[id] = state;
}

/** État d'exécution connu d'un serveur (`undefined` si jamais reçu). */
export function serverRuntime(id: number): string | undefined {
  return store.map[id];
}
