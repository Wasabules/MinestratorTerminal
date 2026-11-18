/**
 * Navigation des Réglages hors de la vue — source unique :
 * - la section ouverte vit ici, pas dans `SettingsView` : elle survit au remontage du workspace
 *   (changement de compte) ;
 * - toute ouverture directe d'une section depuis ailleurs (palette de commandes, « Gérer les
 *   comptes » du menu utilisateur…) passe par `openSettingsSection`.
 */

import { tabs } from '#lib/tabs/tabs.svelte.ts';

/**
 * État partagé avec `SettingsView` : `section` est synchronisée dans les deux sens ;
 * `checkUpdates` est une demande ponctuelle, consommée puis remise à `false` par la vue.
 */
export const settingsNav = $state<{ section: string; checkUpdates: boolean }>({
  section: 'general',
  checkUpdates: false,
});

/** Ouvre (ou focus) l'onglet Réglages sur `section` (`general`, `accounts`, `mcp`…). */
export function openSettingsSection(section: string, opts: { checkUpdates?: boolean } = {}): void {
  settingsNav.section = section;
  if (opts.checkUpdates) settingsNav.checkUpdates = true;
  tabs.openSettings();
}
