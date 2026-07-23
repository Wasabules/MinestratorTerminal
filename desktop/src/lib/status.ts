/**
 * États serveur → clé i18n (`status.*`) + couleur (voir DESIGN.md).
 * Deux dimensions : l'état d'infrastructure (liste) et l'état d'exécution (live).
 */

export interface StateMeta {
  /** Clé i18n sous `status.*`. */
  key: string;
  color: string;
}

/** État d'infrastructure d'un serveur (issu de la liste). */
export function statusMeta(status: string): StateMeta {
  switch (status) {
    case 'active':
      return { key: 'active', color: 'var(--state-running)' };
    case 'hibernation':
      return { key: 'hibernation', color: 'var(--state-hibernate)' };
    case 'disabled':
      return { key: 'disabled', color: 'var(--state-offline)' };
    case 'suspended':
      return { key: 'suspended', color: 'var(--state-danger)' };
    case 'expired':
      return { key: 'expired', color: 'var(--state-danger)' };
    default:
      return { key: status, color: 'var(--text-dim)' };
  }
}

/** État d'exécution (event WS `status`/`stats.state`) → pastille ON/OFF. */
export function runtimeMeta(state: string): StateMeta {
  switch (state) {
    case 'running':
      return { key: 'online', color: 'var(--state-running)' };
    case 'starting':
      return { key: 'starting', color: 'var(--state-pending)' };
    case 'stopping':
      return { key: 'stopping', color: 'var(--state-pending)' };
    default:
      return { key: 'offline', color: 'var(--state-offline)' };
  }
}

export function isRunning(state: string): boolean {
  return state === 'running';
}
