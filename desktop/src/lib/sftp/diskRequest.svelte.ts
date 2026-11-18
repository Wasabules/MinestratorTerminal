/**
 * Demande d'ouverture de la vue « Espace disque » depuis une autre vue (l'aperçu du serveur) :
 * l'onglet Fichiers actif du serveur la consomme et ouvre l'analyse. Rune `$state` : l'onglet
 * déjà ouvert réagit sans être remonté.
 */

const pending = $state<{ ids: number[] }>({ ids: [] });

/** Demande l'analyse du disque pour ce serveur (à l'ouverture de son onglet Fichiers). */
export function requestDiskView(serverId: number): void {
  if (!pending.ids.includes(serverId)) pending.ids.push(serverId);
}

/** Consomme la demande en attente pour ce serveur : `true` s'il y en avait une. */
export function takeDiskRequest(serverId: number): boolean {
  const i = pending.ids.indexOf(serverId);
  if (i === -1) return false;
  pending.ids.splice(i, 1);
  return true;
}
