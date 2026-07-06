/** Abonnements typés aux events console émis par le cœur Rust (taggés par `conn_id`). */

import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import type {
  Alert,
  ChatDelta,
  ConsoleConnection,
  ConsoleOutput,
  ConsoleStats,
  ConsoleStatus,
  CopilotProgress,
  CopilotStarted,
  Diagnosis,
  SftpProgress,
} from './types';

export const alertEvents = {
  new: (cb: (a: Alert) => void): Promise<UnlistenFn> =>
    listen<Alert>('alert://new', (e) => cb(e.payload)),
};

export const copilotEvents = {
  started: (cb: (s: CopilotStarted) => void): Promise<UnlistenFn> =>
    listen<CopilotStarted>('copilot://started', (e) => cb(e.payload)),
  progress: (cb: (p: CopilotProgress) => void): Promise<UnlistenFn> =>
    listen<CopilotProgress>('copilot://progress', (e) => cb(e.payload)),
  diagnosis: (cb: (d: Diagnosis) => void): Promise<UnlistenFn> =>
    listen<Diagnosis>('copilot://diagnosis', (e) => cb(e.payload)),
  /** Fragments de réponse assistant en streaming. */
  chatDelta: (cb: (d: ChatDelta) => void): Promise<UnlistenFn> =>
    listen<ChatDelta>('chat://delta', (e) => cb(e.payload)),
};

export const consoleEvents = {
  output: (cb: (p: ConsoleOutput) => void): Promise<UnlistenFn> =>
    listen<ConsoleOutput>('console://output', (e) => cb(e.payload)),
  stats: (cb: (p: ConsoleStats) => void): Promise<UnlistenFn> =>
    listen<ConsoleStats>('console://stats', (e) => cb(e.payload)),
  status: (cb: (p: ConsoleStatus) => void): Promise<UnlistenFn> =>
    listen<ConsoleStatus>('console://status', (e) => cb(e.payload)),
  connection: (cb: (p: ConsoleConnection) => void): Promise<UnlistenFn> =>
    listen<ConsoleConnection>('console://connection', (e) => cb(e.payload)),
};

export const sftpEvents = {
  /** Progression des transferts SFTP (upload/download/zip). */
  progress: (cb: (p: SftpProgress) => void): Promise<UnlistenFn> =>
    listen<SftpProgress>('sftp://progress', (e) => cb(e.payload)),
};
