/**
 * Changement de compte côté UI mobile. Le core fait le travail (échange des clés, fermeture des
 * consoles et sessions SFTP, relance de la supervision) et émet `account://switched` ; ici on ne
 * fait que réaligner l'interface sur le compte désormais actif.
 */

import { api } from "../ipc";
import { auth } from "./auth.svelte";
import type { Account } from "../types";

/**
 * Aligne l'UI sur le compte actif (`null` : plus aucun → écran de connexion). Idempotent :
 * appelé par l'action elle-même ET par l'event `account://switched`.
 */
export async function applyAccountSwitch(current: Account | null): Promise<void> {
  if (!current) {
    auth.setUser(null);
    return;
  }
  if (auth.user?.id === current.id) return;
  try {
    auth.setUser(await api.getUser());
  } catch {
    // Clé refusée ou API injoignable : retour à l'écran de connexion.
    auth.setUser(null);
  }
}
