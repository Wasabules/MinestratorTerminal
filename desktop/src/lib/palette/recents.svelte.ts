/**
 * Entrées récentes de la palette, affichées en tête quand la requête est vide.
 *
 * Ne sont mémorisés que des **identifiants d'entrée** (`nav:settings`, `server:12:console`) :
 * jamais de libellé, de nom de serveur ni de texte saisi (une commande console peut contenir un
 * pseudo ou un mot de passe). Le libellé est recalculé à l'affichage, donc traduit et substitué
 * en mode démo ; un identifiant qui ne correspond plus à rien (serveur supprimé, autre compte)
 * est simplement ignoré. Persisté en localStorage, comme les autres préférences d'affichage.
 */

const KEY = 'mnstr-palette-recent';
const MAX = 8;

const store = $state<{ ids: string[] }>({ ids: [] });

/** Charge les récents enregistrés (à appeler au montage de la palette). */
export function initPaletteRecents(): void {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? '[]');
    if (Array.isArray(raw)) {
      store.ids = raw.filter((x): x is string => typeof x === 'string').slice(0, MAX);
    }
  } catch {
    /* localStorage illisible ou contenu corrompu : on repart d'une liste vide */
  }
}

/** Identifiants récents, du plus récent au plus ancien (réactif). */
export function recentIds(): readonly string[] {
  return store.ids;
}

/** Mémorise une entrée exécutée (en tête, sans doublon). */
export function pushRecent(id: string): void {
  store.ids = [id, ...store.ids.filter((x) => x !== id)].slice(0, MAX);
  try {
    localStorage.setItem(KEY, JSON.stringify(store.ids));
  } catch {
    /* localStorage indisponible */
  }
}

/** Oublie les récents (ex. changement de compte). */
export function clearRecents(): void {
  store.ids = [];
  try {
    localStorage.removeItem(KEY);
  } catch {
    /* localStorage indisponible */
  }
}
