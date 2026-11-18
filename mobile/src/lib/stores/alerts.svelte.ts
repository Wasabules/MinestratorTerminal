/**
 * Centre d'alertes mobile : alertes du superviseur (crash, seuils CPU/RAM/disque, échéance MyBox).
 *
 * Contrairement au desktop, qui accumule les alertes dans son webview, la source de vérité est le
 * journal tenu côté Rust (`alerts.rs`) : sur Android, le webview peut être détruit pendant que le
 * service d'arrière-plan garde le process et continue d'alerter. Ce store n'en est que le reflet,
 * relu au démarrage, à chaque nouvelle alerte et à l'ouverture de l'écran.
 */

import type { UnlistenFn } from "@tauri-apps/api/event";
import { api } from "../ipc";
import { alertEvents } from "../events";
import type { AlertEntry } from "../types";

class AlertsStore {
  items = $state<AlertEntry[]>([]);
  private unlisten: UnlistenFn | null = null;
  private starting = false;

  /** Alertes non encore vues (pastille de la liste des serveurs). */
  get unread(): number {
    return this.items.reduce((n, a) => n + (a.read ? 0 : 1), 0);
  }

  /** Alertes d'un serveur (l'Aperçu en montre les dernières). */
  forServer(id: number): AlertEntry[] {
    return this.items.filter((a) => a.server_id === id && a.kind !== "expiry");
  }

  /** S'abonne aux nouvelles alertes (une seule fois) et charge le journal. */
  async start(): Promise<void> {
    if (this.unlisten || this.starting) return;
    this.starting = true;
    try {
      this.unlisten = await alertEvents.new(() => void this.refresh());
    } catch {
      /* hors Tauri (prévisualisation) : pas d'events, le journal reste lisible */
    } finally {
      this.starting = false;
    }
    await this.refresh();
  }

  /** Best-effort : un journal illisible ne doit jamais bloquer l'app. */
  async refresh(): Promise<void> {
    try {
      this.items = await api.alertsList();
    } catch {
      /* journal indisponible : on garde l'état précédent */
    }
  }

  /** Ouverture de l'écran Alertes : tout devient lu, comme à l'ouverture du panneau desktop. */
  async markAllRead(): Promise<void> {
    if (this.unread === 0) return;
    this.items = this.items.map((a) => ({ ...a, read: true }));
    try {
      await api.alertsMarkRead();
    } catch {
      await this.refresh();
    }
  }

  async clear(): Promise<void> {
    const previous = this.items;
    this.items = [];
    try {
      await api.alertsClear();
    } catch {
      this.items = previous;
    }
  }
}

export const alerts = new AlertsStore();
