/**
 * Section ouverte de l'onglet Réglages, hors du composant : elle survit au remontage du workspace
 * (changement de compte) et permet d'ouvrir les Réglages directement sur une section (« Gérer les
 * comptes » du menu utilisateur).
 */

import { tabs } from '#lib/tabs/tabs.svelte.ts';

export const settingsNav = $state<{ section: string }>({ section: 'general' });

/** Ouvre (ou focus) l'onglet Réglages sur `section`. */
export function openSettingsAt(section: string): void {
  settingsNav.section = section;
  tabs.openSettings();
}
