/**
 * Couche IPC typée : unique point de contact frontend ↔ commandes Rust.
 * Aucun composant n'appelle `invoke` directement — tout passe par `api`.
 * (Sous-ensemble de départ ; on étoffe au fil des vues, en miroir du desktop.)
 */

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";
import type {
  AppError,
  LiveLight,
  MetricSample,
  PlayerAction,
  PowerAction,
  ServerDetails,
  ServersOverview,
  UserProfile,
} from "./types";

export const api = {
  // --- Auth ---
  validateAndStoreKey: (key: string) => invoke<UserProfile>("validate_and_store_key", { key }),
  hasStoredKey: () => invoke<boolean>("has_stored_key"),
  getUser: () => invoke<UserProfile>("get_user"),
  logout: () => invoke<void>("logout"),

  // --- Serveurs ---
  listServers: () => invoke<ServersOverview>("list_servers"),
  serverDetails: (id: number) => invoke<ServerDetails>("server_details", { id }),
  liveLight: (id: number) => invoke<LiveLight>("live_light", { id }),
  metricsHistory: (serverId: number, sinceSecs: number) =>
    invoke<MetricSample[]>("metrics_history", { serverId, sinceSecs }),

  // --- Console / power / joueurs ---
  consoleLogs: (id: number) => invoke<string[]>("console_logs", { id }),
  powerAction: (id: number, action: PowerAction) =>
    invoke<void>("power_action", { id, action }),
  sendCommand: (id: number, command: string) => invoke<void>("send_command", { id, command }),
  playerAction: (id: number, action: PlayerAction, player: string) =>
    invoke<void>("player_action", { id, action, player }),
  consoleConnect: (connId: string, serverId: number) =>
    invoke<void>("console_connect", { connId, serverId }),
  consoleDisconnect: (connId: string) => invoke<void>("console_disconnect", { connId }),
};

/** Transforme une erreur du core (`{ kind, message }`) en message lisible/i18n. */
export function humanizeError(e: unknown): string {
  const err = e as Partial<AppError> | undefined;
  if (err && typeof err === "object" && "kind" in err) {
    const byKind = t(`error.${err.kind}`);
    if (byKind !== `error.${err.kind}`) return byKind;
    if (err.message) return err.message;
  }
  if (typeof e === "string") return e;
  return t("error.generic");
}
