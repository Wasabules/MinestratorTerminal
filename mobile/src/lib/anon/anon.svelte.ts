/**
 * Mode anonyme : masque à l'écran ce qui permet de retrouver le compte ou ses serveurs —
 * adresses IP (v4 et v6), adresses de connexion, hôte SFTP, e-mail — le temps d'une capture
 * d'écran ou d'un partage d'écran. Bouton œil de l'accueil et de la vue serveur, ligne dans les
 * Réglages.
 *
 * Distinct du mode démo (`fake.svelte.ts`), et cumulable avec lui :
 * - il **masque** (`•••.•••.•••.•••`) au lieu de **remplacer** par des valeurs plausibles ;
 * - il ne touche ni aux noms de serveurs ni aux pseudos ;
 * - il s'applique **au rendu**, pas aux réponses IPC : basculer masque ou démasque aussitôt tout
 *   ce qui est affiché, sans rien recharger.
 *
 * Purement visuel : les valeurs réelles restent en mémoire, les boutons « Copier » et « Partager »
 * les transmettent telles quelles et rien de masqué n'est envoyé au serveur.
 *
 * Rendu : `<Masked>` dans les gabarits, `anonText`/`anonAddress` pour un attribut ou une chaîne
 * composée, `hasSensitive` (mask.ts) pour un champ de saisie (passé en `type="password"`).
 *
 * Même clé de stockage que le desktop ; chaque appareil garde sa préférence.
 */

import { EMAIL_MASK, maskAddress, maskText } from "./mask.ts";

const KEY = "mnstr-anon";

function load(): boolean {
  try {
    return localStorage.getItem(KEY) === "1";
  } catch {
    return false; /* localStorage indisponible */
  }
}

const store = $state<{ on: boolean }>({ on: load() });

/** Le mode anonyme est-il actif ? (réactif) */
export function isAnon(): boolean {
  return store.on;
}

export function setAnon(on: boolean): void {
  store.on = on;
  try {
    localStorage.setItem(KEY, on ? "1" : "0");
  } catch {
    /* localStorage indisponible : le mode vaut pour la session */
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
