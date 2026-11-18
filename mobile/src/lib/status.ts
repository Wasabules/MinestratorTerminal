/**
 * États serveur → clé i18n (`status.*`) + couleurs (voir DESIGN.md). Même logique que le desktop
 * (`desktop/src/lib/status.ts`). Deux dimensions : l'état d'infrastructure (liste des serveurs)
 * et l'état d'exécution (échantillon live).
 */

export interface StateMeta {
  /** Clé i18n complète (`status.…`). */
  key: string;
  /** Couleur de la pastille. */
  color: string;
  /** Couleur du libellé : lisible (≥ 4,5:1) là où la couleur de pastille ne le serait pas. */
  ink: string;
}

/** État d'infrastructure d'un serveur (issu de la liste). */
export function statusMeta(status: string): StateMeta {
  switch (status) {
    case "active":
      return { key: "status.active", color: "var(--state-running)", ink: "var(--state-running-text)" };
    case "hibernation":
      return { key: "status.hibernation", color: "var(--state-hibernate)", ink: "var(--state-hibernate)" };
    case "disabled":
      return { key: "status.disabled", color: "var(--state-offline)", ink: "var(--text-dim)" };
    case "suspended":
      return { key: "status.suspended", color: "var(--state-danger)", ink: "var(--state-danger)" };
    case "expired":
      return { key: "status.expired", color: "var(--state-danger)", ink: "var(--state-danger)" };
    default:
      return { key: "status.unknown", color: "var(--text-dim)", ink: "var(--text-dim)" };
  }
}

/**
 * État d'exécution (`stats.state`) → pastille. Inconnu (aucun échantillon) : on s'appuie sur
 * l'état d'infrastructure quand il est parlant (hibernation, suspension…), sinon « Inconnu ».
 */
export function runtimeMeta(state: string | null, infra?: string): StateMeta {
  switch (state) {
    case "running":
      return { key: "status.running", color: "var(--state-running)", ink: "var(--state-running-text)" };
    case "starting":
      return { key: "status.starting", color: "var(--state-pending)", ink: "var(--state-pending)" };
    case "stopping":
      return { key: "status.stopping", color: "var(--state-pending)", ink: "var(--state-pending)" };
    // Déploiement de l'egg par Wings.
    case "installing":
      return { key: "status.installing", color: "var(--state-hibernate)", ink: "var(--state-hibernate)" };
    case null:
    case undefined:
      if (infra && infra !== "active") return statusMeta(infra);
      return { key: "status.unknown", color: "var(--state-offline)", ink: "var(--text-dim)" };
    default:
      if (infra === "hibernation") return statusMeta(infra);
      return { key: "status.offline", color: "var(--state-offline)", ink: "var(--text-dim)" };
  }
}
