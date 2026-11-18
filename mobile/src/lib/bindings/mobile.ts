// Fichier GÉNÉRÉ depuis les structures Rust (mobile/src-tauri, feature `ts`) par ts-rs.
// Ne pas modifier à la main : changer le type Rust, puis lancer `cargo bindings`.
// `cargo bindings-check` (CI) échoue si ce fichier ne correspond plus au Rust.

import type { SftpEntry } from './core';

// --- Mise à jour de l'APK (update.rs) ---

export type UpdateInfo = {
  /**
   * Version disponible (ex. `0.4.1`).
   */
  version: string,
  /**
   * Corps de la release (changelog markdown).
   */
  notes: string,
  /**
   * URL de l'APK à télécharger.
   */
  apk_url: string,
};

// --- SFTP (commands.rs) ---

/**
 * Résultat de recherche : le tuple `(entrées, tronqué)` du core se sérialiserait en tableau
 * anonyme côté JS, illisible. La couche d'adaptation lui donne des noms.
 */
export type SftpSearchResult = {
  entries: Array<SftpEntry>,
  /**
   * La recherche s'est arrêtée avant d'avoir tout parcouru (garde-fou du core).
   */
  truncated: boolean,
};

// --- Centre d'alertes (alerts.rs) ---

/**
 * Une alerte consignée, telle que l'écran Alertes l'affiche.
 */
export type AlertEntry = {
  /**
   * Identifiant local strictement croissant (clé de liste côté front).
   */
  id: number,
  /**
   * Déjà vue dans le centre d'alertes (pastille de la liste des serveurs).
   */
  read: boolean,
  /**
   * ID serveur, ou ID MyBox pour une alerte d'expiration.
   */
  server_id: number,
  server_name: string,
  /**
   * `crash` | `cpu` | `ram` | `disk` | `expiry`
   */
  kind: string,
  /**
   * `warning` | `critical`
   */
  severity: string,
  message: string,
  ts: number,
};
