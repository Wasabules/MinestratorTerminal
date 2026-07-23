/**
 * État d'authentification partagé, en rune `$state` (module .svelte.ts).
 * Lu par le layout (garde de navigation) et les écrans.
 */

import type { UserProfile } from '$lib/types';

export type AuthState =
  | { status: 'loading' }
  | { status: 'signed_out' }
  | { status: 'signed_in'; user: UserProfile };

// Objet réactif partagé : on mute `.value`, jamais la référence.
export const authStore = $state<{ value: AuthState }>({ value: { status: 'loading' } });

export function setAuth(next: AuthState): void {
  authStore.value = next;
}
