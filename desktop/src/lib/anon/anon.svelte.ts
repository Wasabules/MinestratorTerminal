/**
 * Mode anonyme : masque à l'écran ce qui permet de retrouver le compte ou ses serveurs —
 * adresses IP (v4 et v6), adresses de connexion, hôte SFTP, e-mail — le temps d'une capture
 * d'écran ou d'un partage d'écran. Bouton œil de la barre du haut, Ctrl+Shift+H (⇧⌘H), palette.
 *
 * Distinct du mode démo (`fake.svelte.ts`), et cumulable avec lui :
 * - il **masque** (`•••.•••.•••.•••`) au lieu de **remplacer** par des valeurs plausibles ;
 * - il ne touche ni aux noms de serveurs ni aux pseudos ;
 * - il s'applique **au rendu**, pas aux réponses IPC : basculer masque ou démasque aussitôt tout
 *   ce qui est affiché, sans rien recharger.
 *
 * Purement visuel : les valeurs réelles restent en mémoire, les boutons « Copier » les copient
 * telles quelles et rien de masqué n'est envoyé au serveur.
 *
 * Rendu : `<Masked>` dans les gabarits, `anonText`/`anonAddress` pour un attribut ou une chaîne
 * composée, `hasSensitive` pour un champ de saisie (passé en `type="password"`).
 */

import { isMacPlatform } from '#lib/palette/registry.svelte.ts';
import { maskAddress, maskText, EMAIL_MASK } from './mask.ts';

const KEY = 'mnstr-anon';

const store = $state<{ on: boolean }>({ on: false });

let synced = false;

/** Charge la préférence au démarrage et suit les autres fenêtres (onglets détachés). */
export function initAnon(): void {
  try {
    store.on = localStorage.getItem(KEY) === '1';
  } catch {
    /* localStorage indisponible */
  }
  if (synced) return;
  synced = true;
  // Une capture peut inclure une fenêtre détachée : le mode la suit sans attendre un rechargement.
  window.addEventListener('storage', (e) => {
    if (e.key === KEY) store.on = e.newValue === '1';
  });
}

/** Le mode anonyme est-il actif ? (réactif) */
export function isAnon(): boolean {
  return store.on;
}

export function setAnon(on: boolean): void {
  store.on = on;
  try {
    localStorage.setItem(KEY, on ? '1' : '0');
  } catch {
    /* localStorage indisponible */
  }
}

export function toggleAnon(): void {
  setAnon(!store.on);
}

// --- Chaînes (attributs, textes composés) : masquées si le mode est actif ---

/** Texte libre : IP, e-mails et adresses de serveurs qu'il contient masqués. */
export function anonText(v: string): string {
  return store.on ? maskText(v) : v;
}

/** Adresse de connexion (`hôte[:port]`) ou IP seule. */
export function anonAddress(v: string): string {
  return store.on && v ? maskAddress(v) : v;
}

export function anonEmail(v: string): string {
  return store.on && v ? EMAIL_MASK : v;
}

// --- Raccourci clavier ---
// Ctrl+Shift+H (⇧⌘H sur macOS) : « H » pour *hide*. Libre dans l'app (palette Ctrl+K, copie
// Ctrl+C de la console, Ctrl+S de l'éditeur), dans CodeMirror et dans les raccourcis du webview.

/** L'événement est-il le raccourci du mode anonyme ? */
export function isAnonShortcut(e: KeyboardEvent): boolean {
  if (e.altKey || !e.shiftKey || e.isComposing) return false;
  if (e.key.toLowerCase() !== 'h') return false;
  return isMacPlatform() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

/** Libellé du raccourci (info-bulle, palette, Réglages). */
export function anonShortcutLabel(): string {
  return isMacPlatform() ? '⇧⌘H' : 'Ctrl+Shift+H';
}

/** Valeur `aria-keyshortcuts` du raccourci. */
export function anonShortcutAria(): string {
  return isMacPlatform() ? 'Meta+Shift+H' : 'Control+Shift+H';
}
