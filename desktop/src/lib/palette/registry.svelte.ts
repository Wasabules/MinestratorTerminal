/**
 * Registre de la palette de commandes (Ctrl+K / ⌘K) : la palette n'a aucune entrée en dur, elle
 * affiche ce que les fonctionnalités y enregistrent. Le composant (`CommandPalette.svelte`) ne
 * change pas quand une fonctionnalité ajoute ses actions.
 *
 * Trois points d'extension :
 * - `registerPaletteSource` — une section de la racine (« Navigation », « Comptes »…) ;
 * - `registerServerEntries` — des entrées propres à chaque serveur : elles apparaissent dans la
 *   sous-page du serveur (Tab sur un serveur) et, en recherche, à la racine avec le nom du serveur ;
 * - `registerPaletteMode` — un mode préfixé (`>` : envoyer une commande console).
 *
 * Exemple (fonctionnalité « comptes ») :
 *
 * ```ts
 * import { registerPaletteSource } from '#lib/palette/registry.svelte.ts';
 *
 * registerPaletteSource({
 *   id: 'accounts',
 *   label: () => t('accounts.title'),
 *   order: 40,
 *   entries: () =>
 *     accounts().map((a) => ({
 *       id: `account:${a.id}`,
 *       title: t('accounts.switchTo', { name: a.label }),
 *       icon: 'users',
 *       searchOnly: true,
 *       run: () => switchAccount(a.id),
 *     })),
 * });
 * ```
 *
 * Règles :
 * - `entries()` est rappelée à chaque frappe : elle peut lire des stores rune et `t()` (la palette
 *   suit la langue et l'état), mais ne doit ni écrire d'état ni appeler le réseau.
 * - `id` d'entrée **stable et sans donnée sensible** (pas de nom, de pseudo, d'IP ni de texte
 *   saisi) : il est mémorisé tel quel dans les récents (localStorage). Un identifiant numérique
 *   de serveur convient.
 * - Tout libellé identifiant (nom de serveur, de compte…) doit venir des données déjà passées par
 *   la façade `ipc.ts`, pour rester substitué en mode démo.
 * - Une action destructrice porte `confirm` (et `danger` si elle est irréversible) : la palette
 *   demande confirmation et ne la mémorise jamais dans les récents.
 * - Enregistrer sous un `id` déjà pris remplace la source précédente ; la fonction renvoyée
 *   désinscrit (à appeler dans le `onDestroy` d'un composant qui s'enregistre au montage).
 */

import type { GameCapabilities, ServerListItem } from '#lib/types.ts';

/** Résultat optionnel d'une exécution. Sans résultat, la palette se ferme. */
export interface PaletteOutcome {
  /** Garder la palette ouverte (ex. envoi de commandes en série). */
  keepOpen?: boolean;
  /** Message de statut affiché dans la palette (et annoncé aux lecteurs d'écran). */
  message?: string;
  /** Nouvelle requête à afficher (ex. vider la commande envoyée). */
  query?: string;
}

/** Confirmation demandée avant d'exécuter une entrée. */
export interface PaletteConfirm {
  /** Question posée (« Arrêter « Survie » ? »). */
  title: string;
  /** Avertissement à lire avant de confirmer (ex. message de l'hébergeur), affiché en évidence. */
  warning?: string | null;
  /** Explication complémentaire, plus discrète. */
  detail?: string | null;
  /** Libellé du bouton de confirmation. */
  confirmLabel: string;
}

export interface PaletteEntry {
  /** Identifiant stable et unique, préfixé par la source (`server:12:console`). Voir les règles. */
  id: string;
  /** Libellé principal (déjà traduit). */
  title: string;
  /** Contexte affiché en second (ex. le serveur visé) ; pris en compte par la recherche. */
  subtitle?: string;
  /** Termes de recherche supplémentaires, non affichés (synonymes, autre langue…). */
  keywords?: string[];
  /** Icône : nom dans `Icon.svelte`. */
  icon?: string;
  /** Vignette (URL d'image, ex. icône du jeu) ; repli sur `icon` si elle ne charge pas. */
  image?: string | null;
  /** Pastille d'état (couleur CSS, de préférence un token `var(--state-…)`). */
  dot?: string;
  /** Indication alignée à droite (statut, raccourci…). */
  hint?: string;
  /** N'apparaît qu'en recherche (pas dans la liste par défaut quand la requête est vide). */
  searchOnly?: boolean;
  /** Visible mais non exécutable ; `hint` dit pourquoi. */
  disabled?: boolean;
  /** Action à risque : mise en forme d'alerte, jamais mémorisée dans les récents. */
  danger?: boolean;
  /** Confirmation préalable (actions destructrices). */
  confirm?: PaletteConfirm;
  /** Ne pas mémoriser dans les récents (déjà implicite pour `danger` et `confirm`). */
  noRecent?: boolean;
  /** Bonus de classement (petit entier) pour départager des entrées proches. */
  boost?: number;
  /** Exécution. Un rejet est affiché dans la palette (via `humanizeError`), qui reste ouverte. */
  run?: () => void | PaletteOutcome | Promise<void | PaletteOutcome>;
  /** Sous-page : Tab ou → l'ouvre ; Entrée aussi quand l'entrée n'a pas de `run`. */
  page?: () => PalettePage;
}

export interface PalettePage {
  id: string;
  /** Titre affiché dans le fil d'Ariane du champ de recherche. */
  title: string;
  placeholder?: string;
  /** Entrées de la page ; `query` est fourni aux pages qui l'interprètent (`filter: false`). */
  entries: (query: string) => PaletteEntry[];
  /** `false` : pas de filtrage flou, la page interprète la requête elle-même (ex. commande). */
  filter?: boolean;
}

export interface PaletteSource {
  /** Identifiant unique ; réenregistrer le même id remplace la source. */
  id: string;
  /** Titre de section (lu à chaque rendu : suit la langue). */
  label: () => string;
  /** Rang de la section quand la requête est vide (croissant, défaut 100). */
  order?: number;
  entries: () => PaletteEntry[];
}

/** Serveur tel que vu par les fournisseurs d'entrées par serveur. */
export interface PaletteServer {
  id: number;
  /** Nom affichable (déjà substitué en mode démo). */
  name: string;
  item: ServerListItem;
  /** Capacités du jeu (`undefined` tant qu'inconnues). */
  caps: GameCapabilities | undefined;
  /** État d'exécution live (`running`, `offline`…), `undefined` si aucun event reçu. */
  runtime: string | undefined;
}

export interface ServerEntryProvider {
  id: string;
  /** Rang dans la sous-page du serveur (croissant, défaut 100 ; vues 10, commande 20, alimentation 30). */
  order?: number;
  entries: (server: PaletteServer) => PaletteEntry[];
}

export interface PaletteMode {
  /** Préfixe tapé en tête de requête (`>`). */
  prefix: string;
  /** Libellé du mode, affiché dans le fil d'Ariane et l'aide du pied de palette. */
  label: () => string;
  /** Page du mode ; reçoit la requête **sans** le préfixe. */
  page: () => PalettePage;
}

// --- Registres (réactifs : une inscription pendant que la palette est ouverte s'affiche) ---
// `$state.raw` : les sources portent des fonctions, inutile (et coûteux) de les envelopper dans
// des proxys profonds ; seule la réaffectation des listes doit être suivie.

let sources = $state.raw<PaletteSource[]>([]);
let serverProviders = $state.raw<ServerEntryProvider[]>([]);
let modes = $state.raw<PaletteMode[]>([]);

function byOrder<T extends { order?: number }>(a: T, b: T): number {
  return (a.order ?? 100) - (b.order ?? 100);
}

/** Enregistre une section de la racine. Renvoie la fonction de désinscription. */
export function registerPaletteSource(source: PaletteSource): () => void {
  sources = [...sources.filter((s) => s.id !== source.id), source].sort(byOrder);
  return () => {
    sources = sources.filter((s) => s !== source);
  };
}

/** Enregistre des entrées propres à chaque serveur. Renvoie la fonction de désinscription. */
export function registerServerEntries(provider: ServerEntryProvider): () => void {
  serverProviders = [...serverProviders.filter((p) => p.id !== provider.id), provider].sort(byOrder);
  return () => {
    serverProviders = serverProviders.filter((p) => p !== provider);
  };
}

/** Enregistre un mode préfixé. Renvoie la fonction de désinscription. */
export function registerPaletteMode(mode: PaletteMode): () => void {
  // Le préfixe le plus long d'abord : `>>` ne doit pas être capté par `>`.
  modes = [...modes.filter((m) => m.prefix !== mode.prefix), mode].sort(
    (a, b) => b.prefix.length - a.prefix.length
  );
  return () => {
    modes = modes.filter((m) => m !== mode);
  };
}

/** Sources de la racine, triées par `order`. */
export function paletteSources(): readonly PaletteSource[] {
  return sources;
}

/** Fournisseurs d'entrées par serveur, triés par `order`. */
export function serverEntryProviders(): readonly ServerEntryProvider[] {
  return serverProviders;
}

/** Modes préfixés enregistrés. */
export function paletteModes(): readonly PaletteMode[] {
  return modes;
}

/** Mode dont le préfixe ouvre la requête, ou `undefined`. */
export function modeFor(query: string): PaletteMode | undefined {
  return modes.find((m) => query.startsWith(m.prefix));
}

// --- Ouverture ---

const ui = $state<{ open: boolean; query: string; seq: number }>({ open: false, query: '', seq: 0 });

/** Ouvre la palette ; `query` la préremplit (ex. `'>'` pour le mode commande). */
export function openPalette(opts: { query?: string } = {}): void {
  ui.query = opts.query ?? '';
  ui.seq += 1;
  ui.open = true;
}

export function closePalette(): void {
  ui.open = false;
}

export function togglePalette(): void {
  if (ui.open) closePalette();
  else openPalette();
}

/** État d'ouverture (réactif). `seq` change à chaque ouverture, `query` est la requête initiale. */
export function paletteUi(): { readonly open: boolean; readonly query: string; readonly seq: number } {
  return ui;
}

// --- Raccourci clavier ---

/** macOS : ⌘K ; ailleurs : Ctrl+K. */
export function isMacPlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /Mac|iPhone|iPad/i.test(navigator.platform || navigator.userAgent);
}

/** L'événement est-il le raccourci d'ouverture de la palette ? */
export function isPaletteShortcut(e: KeyboardEvent): boolean {
  if (e.altKey || e.shiftKey || e.isComposing) return false;
  if (e.key.toLowerCase() !== 'k') return false;
  return isMacPlatform() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

/** Libellé du raccourci, pour l'afficher (bouton, info-bulle, Réglages). */
export function paletteShortcutLabel(): string {
  return isMacPlatform() ? '⌘K' : 'Ctrl+K';
}

/** Valeur `aria-keyshortcuts` du raccourci. */
export function paletteShortcutAria(): string {
  return isMacPlatform() ? 'Meta+K' : 'Control+K';
}
