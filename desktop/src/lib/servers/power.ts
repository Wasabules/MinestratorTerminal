/**
 * Actions d'alimentation : lesquelles proposer selon l'état d'exécution, et quel avertissement de
 * l'hébergeur afficher avant de les lancer.
 *
 * Source unique partagée par `PowerControl.svelte` (boutons des vues serveur) et la palette de
 * commandes : un garde-fou ajouté ici vaut pour les deux points d'entrée.
 */

import { isRunning } from '#lib/status.ts';
import type { PowerAction } from '#lib/types.ts';

/** Actions secondaires (menu « ⋯ ») d'un serveur en marche. */
const RUNNING_ACTIONS: PowerAction[] = ['restart', 'restart10', 'stop10', 'kill'];
/** Actions secondaires d'un serveur arrêté. */
const STOPPED_ACTIONS: PowerAction[] = ['restart'];
/** Ordre d'affichage quand les deux jeux d'actions sont proposés ensemble. */
const ORDER: PowerAction[] = ['start', 'stop', 'restart', 'restart10', 'stop10', 'kill'];

/** Action principale (bouton) : arrêter un serveur en marche, démarrer sinon. */
export function primaryPowerAction(running: boolean): PowerAction {
  return running ? 'stop' : 'start';
}

/** Actions secondaires selon l'état d'exécution. */
export function secondaryPowerActions(running: boolean): PowerAction[] {
  return running ? RUNNING_ACTIONS : STOPPED_ACTIONS;
}

/**
 * Toutes les actions proposables pour un état d'exécution live (`running`, `offline`…). État
 * inconnu (aucun event reçu) : on propose l'union des deux jeux plutôt que de masquer à tort.
 */
export function availablePowerActions(state: string | undefined): PowerAction[] {
  const set = (running: boolean) => [primaryPowerAction(running), ...secondaryPowerActions(running)];
  const offered = new Set(state === undefined ? [...set(true), ...set(false)] : set(isRunning(state)));
  return ORDER.filter((a) => offered.has(a));
}

/** Avertissements configurés par l'hébergeur pour un serveur (`poweraction_message_*` de l'API). */
export interface PowerWarnings {
  restartWarning?: string | null;
  stopWarning?: string | null;
}

/** Avertissement associé à une action, ou `null` si l'hébergeur n'en a pas défini. */
export function powerWarning(action: PowerAction, warnings: PowerWarnings): string | null {
  if (action === 'restart' || action === 'restart10') return warnings.restartWarning || null;
  if (action === 'stop' || action === 'stop10' || action === 'kill') {
    return warnings.stopWarning || null;
  }
  return null;
}

/** L'action interrompt-elle le serveur (joueurs déconnectés) ? Tout sauf le démarrage. */
export function isDisruptivePower(action: PowerAction): boolean {
  return action !== 'start';
}

/** Statuts d'infrastructure (liste des serveurs) pour lesquels l'hébergeur refuse toute action. */
const BLOCKING_STATUSES = new Set(['hibernation', 'disabled', 'suspended', 'expired']);

/**
 * Le statut d'infrastructure interdit-il les actions d'alimentation ? Même garde-fou que les
 * boutons désactivés des vues serveur, qui lisent, eux, la phase de la connexion live
 * (`hibernated`, `suspended`). Un statut inconnu ne bloque pas : l'API tranchera.
 */
export function powerBlocked(status: string): boolean {
  return BLOCKING_STATUSES.has(status);
}
