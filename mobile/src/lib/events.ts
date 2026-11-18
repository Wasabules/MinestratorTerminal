/** Abonnements typés aux events console émis par le core Rust (taggés par conn_id). */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Alert,
  ConsoleConnection,
  ConsoleOutput,
  ConsoleStats,
  ConsoleStatus,
  SftpProgress,
} from "./types";

export const consoleEvents = {
  output: (cb: (p: ConsoleOutput) => void): Promise<UnlistenFn> =>
    listen<ConsoleOutput>("console://output", (e) => cb(e.payload)),
  stats: (cb: (p: ConsoleStats) => void): Promise<UnlistenFn> =>
    listen<ConsoleStats>("console://stats", (e) => cb(e.payload)),
  status: (cb: (p: ConsoleStatus) => void): Promise<UnlistenFn> =>
    listen<ConsoleStatus>("console://status", (e) => cb(e.payload)),
  connection: (cb: (p: ConsoleConnection) => void): Promise<UnlistenFn> =>
    listen<ConsoleConnection>("console://connection", (e) => cb(e.payload)),
};

/** Progression des transferts SFTP pilotés par le core (envoi natif). */
export const sftpEvents = {
  progress: (cb: (p: SftpProgress) => void): Promise<UnlistenFn> =>
    listen<SftpProgress>("sftp://progress", (e) => cb(e.payload)),
};

/**
 * Alertes du superviseur (crash, seuils, échéance MyBox). Le pont Rust les consigne dans le
 * journal local AVANT d'émettre l'event : le recevoir suffit pour relire `api.alertsList()`.
 */
export const alertEvents = {
  new: (cb: (a: Alert) => void): Promise<UnlistenFn> =>
    listen<Alert>("alert://new", (e) => cb(e.payload)),
};
