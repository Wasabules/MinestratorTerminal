/** Abonnements typés aux events console émis par le core Rust (taggés par conn_id). */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ConsoleConnection,
  ConsoleOutput,
  ConsoleStats,
  ConsoleStatus,
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
