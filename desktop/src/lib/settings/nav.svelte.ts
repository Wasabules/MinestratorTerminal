/**
 * Navigation vers une section des Réglages depuis l'extérieur de la vue (palette de commandes,
 * liens d'autres panneaux). La section affichée reste un état local de `SettingsView` : ce module
 * ne fait que transmettre l'intention, que la vue consomme puis efface.
 */

import { tabs } from '#lib/tabs/tabs.svelte.ts';

export interface SettingsRequest {
  /** Identifiant de section de `SettingsView` (`general`, `mcp`, `privacy`…). */
  section: string;
  /** Lancer aussi la recherche de mise à jour (section Général). */
  checkUpdates?: boolean;
}

const store = $state<{ request: SettingsRequest | null }>({ request: null });

/** Ouvre (ou focus) l'onglet Réglages sur `section`. */
export function openSettingsSection(section: string, opts: { checkUpdates?: boolean } = {}): void {
  store.request = { section, checkUpdates: opts.checkUpdates };
  tabs.openSettings();
}

/** Demande en attente (réactif), `null` sinon. */
export function settingsRequest(): SettingsRequest | null {
  return store.request;
}

/** Marque la demande comme traitée (appelé par `SettingsView`). */
export function clearSettingsRequest(): void {
  store.request = null;
}
