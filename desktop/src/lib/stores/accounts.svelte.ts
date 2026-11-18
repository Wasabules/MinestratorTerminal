/**
 * Comptes MineStrator enregistrés et changement de compte, côté UI.
 *
 * Le cœur Rust fait tout le travail (échange des clés, fermeture des consoles et sessions SFTP,
 * relance de la supervision) et émet `account://switched` à toutes les fenêtres. Ici, on ne fait
 * que réaligner l'interface : onglets serveurs de l'ancien compte fermés, profil rechargé — le
 * `{#key}` de `+page.svelte` remonte alors le workspace, donc toutes les listes se rechargent.
 */

import { api } from '#lib/ipc.ts';
import { authStore, setAuth } from '#lib/stores/auth.svelte.ts';
import { tabs } from '#lib/tabs/tabs.svelte.ts';
import type { Account } from '#lib/types.ts';

/**
 * Liste partagée (menu utilisateur, Réglages → Comptes) et ouverture de la boîte « Ajouter un
 * compte », rendue une seule fois par le menu utilisateur.
 */
export const accountsStore = $state<{ list: Account[]; adding: boolean }>({
  list: [],
  adding: false,
});

/** Ouvre la boîte « Ajouter un compte » (menu utilisateur, Réglages, palette de commandes…). */
export function openAddAccount(): void {
  accountsStore.adding = true;
}

/** Recharge la liste des comptes (sans effet en cas d'échec : la liste précédente reste). */
export async function refreshAccounts(): Promise<Account[]> {
  try {
    accountsStore.list = await api.accountsList();
  } catch {
    /* liste indisponible : on garde la précédente */
  }
  return accountsStore.list;
}

/**
 * Aligne l'UI sur le compte désormais actif (`null` : plus aucun → écran de connexion).
 * Idempotent : appelé par l'action elle-même ET par l'event `account://switched`.
 */
export async function applyAccountSwitch(current: Account | null): Promise<void> {
  if (!current) {
    tabs.reset();
    accountsStore.list = [];
    setAuth({ status: 'signed_out' });
    return;
  }
  const auth = authStore.value;
  if (auth.status === 'signed_in' && auth.user.id === current.id) {
    void refreshAccounts();
    return;
  }
  tabs.closeServerTabs();
  try {
    const user = await api.getUser();
    setAuth({ status: 'signed_in', user });
  } catch {
    // Clé du nouveau compte refusée ou API injoignable : l'écran de connexion propose de
    // basculer vers un autre compte ou de saisir une clé.
    setAuth({ status: 'signed_out' });
  }
  void refreshAccounts();
}
