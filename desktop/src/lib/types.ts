/**
 * Types échangés avec le cœur Rust.
 *
 * Tout type qui traverse l'IPC (réponse de commande, payload d'event, argument structuré) est
 * GÉNÉRÉ depuis sa structure Rust par ts-rs dans `./bindings/core.ts`, et ré-exporté ici tel quel :
 * ne jamais le redéclarer à la main. Pour en ajouter ou en modifier un, changer la structure Rust
 * (derive `ts_rs::TS` sous la feature `ts`, cf. `crates/minestrator-core/src/bindings.rs`), puis
 * lancer `cargo bindings` à la racine. `cargo bindings-check` (CI) échoue si les fichiers dérivent.
 *
 * Ne restent déclarés ci-dessous que les types purement front, sans équivalent Rust.
 */

export type * from './bindings/core';

/**
 * Actions d'alimentation acceptées par l'endpoint `/poweraction`. Côté Rust, la commande
 * `power_action` reçoit une simple chaîne qu'elle relaie à l'API : l'union n'existe que côté front.
 */
export type PowerAction = 'start' | 'restart' | 'restart10' | 'stop' | 'stop10' | 'kill';

/**
 * Actions joueur relayées telles quelles à l'API par la commande `player_action` (chaîne côté Rust).
 */
export type PlayerAction =
  | 'kick'
  | 'ban'
  | 'unban'
  | 'op_add'
  | 'op_remove'
  | 'whitelist_add'
  | 'whitelist_remove';

// --- Marketplace (mods & plugins) : paramètres de requête, chaînes côté Rust ---
export type MarketKind = 'mods' | 'plugins';
export type MarketSource = 'modrinth' | 'curseforge' | 'spigot';
