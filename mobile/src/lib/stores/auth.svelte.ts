import type { UserProfile } from "../types";

/** État d'authentification global (rune). `null` = non connecté. */
class AuthStore {
  user = $state<UserProfile | null>(null);
  /** true une fois la présence d'une clé vérifiée au démarrage (évite un flash d'onboarding). */
  booted = $state(false);

  setUser(u: UserProfile | null) {
    this.user = u;
  }

  get isAuthed(): boolean {
    return this.user !== null;
  }
}

export const auth = new AuthStore();
