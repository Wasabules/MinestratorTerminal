/**
 * Sources intégrées de la palette de commandes : serveurs (onglet, vues, alimentation, commande
 * console), navigation, sections des Réglages, préférences, et le mode `>` (commande console).
 *
 * Elles passent par le registre public (`registry.svelte.ts`) exactement comme le ferait une autre
 * fonctionnalité : ce fichier sert aussi d'exemple d'enregistrement.
 */

import { t, getLocale, setLocale, LOCALES } from '#lib/i18n/index.ts';
import { api } from '#lib/ipc.ts';
import { tabs } from '#lib/tabs/tabs.svelte.ts';
import { isCompactTabs, setCompactTabs } from '#lib/tabs/mode.svelte.ts';
import {
  availableViews,
  knownServers,
  rememberCaps,
  serverCaps,
} from '#lib/games/capabilities.svelte.ts';
import { serverRuntime } from '#lib/servers/runtime.svelte.ts';
import {
  availablePowerActions,
  isDisruptivePower,
  powerBlocked,
  powerWarning,
} from '#lib/servers/power.ts';
import { runtimeMeta, statusMeta } from '#lib/status.ts';
import { toggleTheme } from '#lib/theme.ts';
import { openSettingsSection } from '#lib/settings/nav.svelte.ts';
import { setAlertCenterOpen } from '#lib/alerts/alerts.svelte.ts';
import type { PowerAction } from '#lib/types.ts';
import {
  registerPaletteMode,
  registerPaletteSource,
  registerServerEntries,
  serverEntryProviders,
  type PaletteEntry,
  type PalettePage,
  type PaletteServer,
} from '#lib/palette/registry.svelte.ts';

// --- Serveurs ---

/** Serveurs connus (liste déjà chargée par l'accueil), triés par nom. */
export function paletteServers(): PaletteServer[] {
  return knownServers()
    .map((item) => ({
      id: item.id,
      name: item.name,
      item,
      caps: serverCaps(item.id),
      runtime: serverRuntime(item.id),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

function findServer(id: number): PaletteServer | undefined {
  return paletteServers().find((s) => s.id === id);
}

let loaded = false;
let loading = false;

/**
 * Palette ouverte avant que l'accueil ait chargé la liste : la charger une fois. Sinon, aucune
 * requête — la liste de l'accueil (et son bouton Rafraîchir) fait foi.
 */
export function ensurePaletteServers(): void {
  if (loaded || loading || knownServers().length > 0) return;
  loading = true;
  api
    .listServers()
    .then((o) => {
      rememberCaps(o.servers);
      loaded = true;
    })
    .catch(() => {
      /* liste indisponible : la palette garde ses autres sections */
    })
    .finally(() => (loading = false));
}

/** Pastille + libellé d'état : statut d'infrastructure s'il bloque, sinon état live. */
function serverState(s: PaletteServer): { dot: string; label: string } {
  const meta =
    powerBlocked(s.item.status) || !s.runtime ? statusMeta(s.item.status) : runtimeMeta(s.runtime);
  return { dot: meta.color, label: t(`status.${meta.key}`) };
}

/** Focus un onglet existant du serveur (l'actif s'il en est un), sinon ouvre son Aperçu. */
function openServer(s: PaletteServer): void {
  const active = tabs.tabs.find((x) => x.id === tabs.activeId);
  if (active?.kind === 'server' && active.serverId === s.id) return;
  const existing = tabs.tabs.find((x) => x.kind === 'server' && x.serverId === s.id);
  if (existing) tabs.activate(existing.id);
  else tabs.focusOrOpen(s.id, s.name, 'overview');
}

/** Entrées de toutes les sources par serveur (vues, commande, alimentation, contributions). */
function scopedEntries(s: PaletteServer): PaletteEntry[] {
  return serverEntryProviders().flatMap((p) => p.entries(s));
}

function serverPage(id: number, title: string): PalettePage {
  return {
    id: `server:${id}`,
    title,
    entries: () => {
      const s = findServer(id);
      return s ? scopedEntries(s) : [];
    },
  };
}

function serverEntry(s: PaletteServer): PaletteEntry {
  const state = serverState(s);
  return {
    id: `server:${s.id}`,
    title: s.name,
    subtitle: s.item.egg_name,
    // L'adresse aide à départager deux serveurs homonymes ; non affichée (substituée en démo).
    keywords: [s.item.address],
    image: s.item.egg_icon_url,
    icon: 'gauge',
    dot: state.dot,
    hint: state.label,
    boost: 2,
    run: () => openServer(s),
    page: () => serverPage(s.id, s.name),
  };
}

const POWER_ICONS: Record<PowerAction, string> = {
  start: 'play',
  stop: 'square',
  stop10: 'square',
  restart: 'refresh-cw',
  restart10: 'refresh-cw',
  kill: 'skull',
};

function powerEntry(s: PaletteServer, action: PowerAction): PaletteEntry {
  const blocked = powerBlocked(s.item.status);
  const label = t(`power.${action}`);
  return {
    id: `server:${s.id}:power:${action}`,
    title: label,
    icon: POWER_ICONS[action],
    keywords: [t('palette.kw.power')],
    danger: action === 'kill',
    disabled: blocked,
    hint: blocked ? serverState(s).label : undefined,
    // Mêmes avertissements de l'hébergeur que les boutons des vues serveur (`servers/power.ts`),
    // et confirmation systématique : une palette se valide trop vite au clavier.
    confirm: isDisruptivePower(action)
      ? {
          title: t('palette.confirmPower', { action: label, server: s.name }),
          warning: powerWarning(action, {
            restartWarning: s.item.restart_warning,
            stopWarning: s.item.stop_warning,
          }),
          detail: t(action === 'kill' ? 'palette.confirmKillDetail' : 'palette.confirmPowerDetail'),
          confirmLabel: label,
        }
      : undefined,
    run: () => api.powerAction(s.id, action),
  };
}

/** Page d'envoi de commandes à un serveur : la requête EST la commande (pas de filtrage). */
function commandPage(id: number, name: string): PalettePage {
  return {
    id: `command:${id}`,
    title: name,
    placeholder: t('palette.commandPlaceholder', { server: name }),
    filter: false,
    entries: (query) => {
      const s = findServer(id);
      if (!s) return [];
      const cmd = query.trim();
      const blocked = powerBlocked(s.item.status);
      return [
        {
          id: `command:${id}:send`,
          title: cmd ? t('palette.sendNamed', { cmd }) : t('palette.typeCommand'),
          icon: 'terminal',
          disabled: !cmd || blocked,
          hint: blocked ? serverState(s).label : undefined,
          noRecent: true, // le texte d'une commande n'est jamais mémorisé
          run: async () => {
            await api.sendCommand(id, cmd);
            // Palette laissée ouverte, champ vidé : on peut enchaîner les commandes.
            return {
              keepOpen: true,
              query: '',
              message: t('palette.commandSent', { cmd, server: s.name }),
            };
          },
        },
        {
          id: `server:${id}:console`,
          title: t('palette.openConsole'),
          icon: 'terminal',
          run: () => tabs.focusOrOpen(id, s.name, 'console'),
        },
      ];
    },
  };
}

function commandEntry(s: PaletteServer, id: string, title: string): PaletteEntry {
  const blocked = powerBlocked(s.item.status);
  return {
    id,
    title,
    icon: 'terminal',
    disabled: blocked,
    hint: blocked ? serverState(s).label : undefined,
    page: () => commandPage(s.id, s.name),
  };
}

// --- Navigation, Réglages, préférences ---

/** Sections de `SettingsView` (id → clé i18n du libellé). Une nouvelle section s'ajoute ici. */
const SETTINGS_SECTIONS: { id: string; label: string }[] = [
  { id: 'general', label: 'settings.general' },
  { id: 'games', label: 'settings.games' },
  { id: 'supervisor', label: 'settings.supervisor' },
  { id: 'service', label: 'service.nav' },
  { id: 'mcp', label: 'settings.mcp' },
  { id: 'copilot', label: 'settings.copilot' },
  { id: 'privacy', label: 'settings.privacy' },
  { id: 'debug', label: 'settings.debug' },
];

function settingsEntries(withContext: boolean): PaletteEntry[] {
  return SETTINGS_SECTIONS.map((s) => ({
    id: `settings:${s.id}`,
    title: t(s.label),
    subtitle: withContext ? t('settings.title') : undefined,
    keywords: [t('palette.kw.settings')],
    icon: 'settings',
    searchOnly: true,
    run: () => openSettingsSection(s.id),
  }));
}

/** Synonymes de recherche d'une vue (`palette.kw.views.<id>`), s'il y en a. */
function viewKeywords(view: string): string[] | undefined {
  const key = `palette.kw.views.${view}`;
  const kw = t(key);
  return kw === key ? undefined : [kw];
}

function currentTheme(): 'dark' | 'light' {
  return document.documentElement.getAttribute('data-theme') === 'light' ? 'light' : 'dark';
}

let installed = false;

/** Enregistre les sources intégrées (idempotent ; appelé par le composant de la palette). */
export function installBuiltinPaletteSources(): void {
  if (installed) return;
  installed = true;

  // Entrées par serveur. Les vues suivent `availableViews` (capacités du jeu) : une vue ajoutée à
  // `VIEWS` (tabs.svelte.ts) apparaît ici sans autre changement, libellée par `view.<id>`.
  registerServerEntries({
    id: 'views',
    order: 10,
    entries: (s) =>
      availableViews(s.caps)
        .filter((v) => v.ready)
        .map((v) => ({
          id: `server:${s.id}:${v.id}`,
          title: t(`view.${v.id}`),
          keywords: viewKeywords(v.id),
          icon: v.icon,
          run: () => tabs.focusOrOpen(s.id, s.name, v.id),
        })),
  });
  registerServerEntries({
    id: 'command',
    order: 20,
    entries: (s) => [commandEntry(s, `server:${s.id}:command`, t('palette.sendCommand'))],
  });
  registerServerEntries({
    id: 'power',
    order: 30,
    entries: (s) => availablePowerActions(s.runtime).map((a) => powerEntry(s, a)),
  });

  registerPaletteSource({
    id: 'servers',
    label: () => t('palette.groups.servers'),
    order: 10,
    entries: () =>
      paletteServers().flatMap((s) => [
        serverEntry(s),
        // En recherche seulement : « survie console », « redémarrer survie »…
        ...scopedEntries(s).map((e) => ({ ...e, subtitle: e.subtitle ?? s.name, searchOnly: true })),
      ]),
  });

  registerPaletteSource({
    id: 'navigation',
    label: () => t('palette.groups.navigation'),
    order: 20,
    entries: () => [
      { id: 'nav:home', title: t('common.home'), icon: 'home', run: () => tabs.focusHome() },
      {
        id: 'nav:notifications',
        title: t('notifications.title'),
        icon: 'bell',
        run: () => tabs.openNotifications(),
      },
      { id: 'nav:copilot', title: t('copilot.title'), icon: 'activity', run: () => tabs.openCopilot() },
      {
        id: 'nav:alerts',
        title: t('palette.alerts'),
        icon: 'triangle-alert',
        keywords: [t('palette.kw.alerts')],
        run: () => setAlertCenterOpen(true),
      },
      {
        id: 'nav:settings',
        title: t('settings.title'),
        icon: 'settings',
        keywords: [t('palette.kw.settings')],
        run: () => tabs.openSettings(),
        page: () => ({
          id: 'settings',
          title: t('settings.title'),
          entries: () => settingsEntries(false),
        }),
      },
      {
        id: 'nav:close-tab',
        title: t('palette.closeTab'),
        icon: 'x',
        searchOnly: true,
        disabled: tabs.tabs.find((x) => x.id === tabs.activeId)?.kind === 'home',
        noRecent: true,
        run: () => tabs.close(tabs.activeId),
      },
    ],
  });

  registerPaletteSource({
    id: 'settings',
    label: () => t('palette.groups.settings'),
    order: 30,
    entries: () => settingsEntries(true),
  });

  registerPaletteSource({
    id: 'preferences',
    label: () => t('palette.groups.preferences'),
    order: 40,
    entries: () => [
      {
        id: 'pref:theme',
        title: currentTheme() === 'dark' ? t('palette.themeLight') : t('palette.themeDark'),
        icon: 'contrast',
        keywords: [t('common.theme')],
        run: () => void toggleTheme(),
      },
      ...LOCALES.filter((l) => l.code !== getLocale()).map((l) => ({
        id: `pref:locale:${l.code}`,
        title: t('palette.language', { lang: l.label }),
        icon: 'globe',
        keywords: [t('common.language'), 'language', 'langue'],
        run: () => setLocale(l.code),
      })),
      {
        id: 'pref:compact-tabs',
        title: isCompactTabs() ? t('palette.compactOff') : t('palette.compactOn'),
        icon: isCompactTabs() ? 'check-square' : 'square',
        keywords: [t('settings.compactTabs')],
        run: () => setCompactTabs(!isCompactTabs()),
      },
      {
        id: 'pref:check-updates',
        title: t('palette.checkUpdates'),
        icon: 'download',
        keywords: [t('palette.kw.updates')],
        run: () => openSettingsSection('general', { checkUpdates: true }),
      },
    ],
  });

  // Mode `>` : choisir le serveur (recherche floue sur les noms), puis taper la commande.
  registerPaletteMode({
    prefix: '>',
    label: () => t('palette.commandMode'),
    page: () => ({
      id: 'mode:command',
      title: t('palette.commandMode'),
      placeholder: t('palette.commandPick'),
      entries: () =>
        paletteServers().map((s) => ({
          ...commandEntry(s, `command-pick:${s.id}`, s.name),
          subtitle: s.item.egg_name,
          keywords: [s.item.address],
          image: s.item.egg_icon_url,
          dot: serverState(s).dot,
          hint: serverState(s).label,
          noRecent: true,
        })),
    }),
  });
}
