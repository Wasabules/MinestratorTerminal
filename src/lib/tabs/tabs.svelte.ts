/**
 * Gestionnaire d'onglets du workspace.
 *
 * Modèle : un onglet « Home » permanent (liste des serveurs) + N onglets serveur.
 * Un onglet serveur cible un couple (serveur, vue) — Console, SFTP, Aperçu…
 * Plusieurs onglets peuvent viser le **même** serveur et la **même** vue (doublons
 * autorisés), chacun avec son propre `id` d'instance.
 *
 * Important : les panneaux inactifs restent montés (voir Workspace.svelte). Le store
 * ne gère donc que l'identité et l'ordre des onglets, pas leur cycle de vie interne
 * (une console garde sa connexion WS quand on change d'onglet).
 */

import { uid } from '$lib/util/id';

export type ServerView =
  | 'overview'
  | 'console'
  | 'sftp'
  | 'players'
  | 'mods'
  | 'assistant'
  | 'backups'
  | 'settings';

export interface HomeTab {
  id: string;
  kind: 'home';
}

export interface SettingsTab {
  id: string;
  kind: 'settings';
}

export interface CopilotTab {
  id: string;
  kind: 'copilot';
}

export interface ServerTab {
  id: string;
  kind: 'server';
  serverId: number;
  serverName: string;
  view: ServerView;
}

export type Tab = HomeTab | SettingsTab | CopilotTab | ServerTab;

/** Métadonnées d'affichage par vue (ordre = ordre des actions dans l'UI). `icon` = nom dans Icon.svelte. */
export const VIEWS: { id: ServerView; icon: string; ready: boolean }[] = [
  { id: 'overview', icon: 'gauge', ready: true },
  { id: 'console', icon: 'terminal', ready: true },
  { id: 'sftp', icon: 'folder', ready: true },
  { id: 'players', icon: 'users', ready: true },
  { id: 'mods', icon: 'package', ready: true },
  { id: 'assistant', icon: 'message-circle', ready: true },
  { id: 'backups', icon: 'hard-drive', ready: true },
  { id: 'settings', icon: 'settings', ready: false },
];

export function viewMeta(view: ServerView) {
  return VIEWS.find((v) => v.id === view) ?? VIEWS[0];
}

const HOME_ID = 'home';

class TabManager {
  tabs = $state<Tab[]>([{ id: HOME_ID, kind: 'home' }]);
  activeId = $state<string>(HOME_ID);

  activate(id: string): void {
    if (this.tabs.some((t) => t.id === id)) this.activeId = id;
  }

  focusHome(): void {
    this.activeId = HOME_ID;
  }

  /** Ouvre (ou focus) l'onglet Réglages — unique. */
  openSettings(): void {
    const existing = this.tabs.find((t) => t.kind === 'settings');
    if (existing) {
      this.activeId = existing.id;
      return;
    }
    const tab: SettingsTab = { id: uid(), kind: 'settings' };
    this.tabs.push(tab);
    this.activeId = tab.id;
  }

  /** Ouvre (ou focus) l'onglet Copilote — unique. */
  openCopilot(): void {
    const existing = this.tabs.find((t) => t.kind === 'copilot');
    if (existing) {
      this.activeId = existing.id;
      return;
    }
    const tab: CopilotTab = { id: uid(), kind: 'copilot' };
    this.tabs.push(tab);
    this.activeId = tab.id;
  }

  /** Focus un onglet (serveur, vue) existant s'il y en a un, sinon en ouvre un. */
  focusOrOpen(serverId: number, serverName: string, view: ServerView): void {
    const existing = this.tabs.find(
      (t): t is ServerTab => t.kind === 'server' && t.serverId === serverId && t.view === view
    );
    if (existing) {
      this.activeId = existing.id;
      return;
    }
    this.openNew(serverId, serverName, view);
  }

  /** Ouvre toujours un nouvel onglet (autorise les doublons). */
  openNew(serverId: number, serverName: string, view: ServerView): void {
    const tab: ServerTab = { id: uid(), kind: 'server', serverId, serverName, view };
    this.tabs.push(tab);
    this.activeId = tab.id;
  }

  close(id: string): void {
    if (id === HOME_ID) return; // Home n'est pas fermable.
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    this.tabs.splice(idx, 1);
    if (this.activeId === id) {
      const next = this.tabs[idx] ?? this.tabs[idx - 1] ?? this.tabs[0];
      this.activeId = next.id;
    }
  }

  /** Ferme tous les onglets serveur sauf `keepId` (et Home). */
  closeOthers(keepId: string): void {
    this.tabs = this.tabs.filter((t) => t.kind === 'home' || t.id === keepId);
    this.ensureActiveValid(keepId);
  }

  /** Ferme les onglets serveur situés à droite de `id`. */
  closeRight(id: string): void {
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    this.tabs = this.tabs.filter((t, i) => t.kind === 'home' || i <= idx);
    this.ensureActiveValid(id);
  }

  /** Ferme les onglets serveur situés à gauche de `id` (Home reste). */
  closeLeft(id: string): void {
    const idx = this.tabs.findIndex((t) => t.id === id);
    if (idx === -1) return;
    this.tabs = this.tabs.filter((t, i) => t.kind === 'home' || i >= idx);
    this.ensureActiveValid(id);
  }

  /** Déplace un onglet serveur à la position `targetIndex` du tableau courant. */
  moveTo(id: string, targetIndex: number): void {
    const from = this.tabs.findIndex((t) => t.id === id);
    if (from === -1 || this.tabs[from].kind === 'home') return;
    const arr = [...this.tabs];
    const [moved] = arr.splice(from, 1);
    let idx = from < targetIndex ? targetIndex - 1 : targetIndex;
    idx = Math.max(1, Math.min(idx, arr.length)); // jamais avant Home
    arr.splice(idx, 0, moved);
    this.tabs = arr;
  }

  private ensureActiveValid(fallbackId: string): void {
    if (!this.tabs.some((t) => t.id === this.activeId)) {
      const exists = this.tabs.some((t) => t.id === fallbackId);
      this.activeId = exists ? fallbackId : HOME_ID;
    }
  }

  /** Ferme tous les onglets serveur (ex. à la déconnexion). */
  reset(): void {
    this.tabs = [{ id: HOME_ID, kind: 'home' }];
    this.activeId = HOME_ID;
  }
}

export const tabs = new TabManager();
