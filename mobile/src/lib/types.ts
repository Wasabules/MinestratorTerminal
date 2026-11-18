/**
 * Types échangés avec le cœur Rust (sous-ensemble mobile).
 *
 * Tout type qui traverse l'IPC est GÉNÉRÉ depuis sa structure Rust par ts-rs, puis ré-exporté
 * ici tel quel : ne jamais le redéclarer à la main.
 * - `./bindings/core.ts` : types du core (identique à celui du desktop) ;
 * - `./bindings/mobile.ts` : types propres à l'adaptation mobile (`mobile/src-tauri`).
 * Pour en ajouter ou en modifier un, changer la structure Rust (derive `ts_rs::TS` sous la feature
 * `ts`), puis lancer `cargo bindings` à la racine. `cargo bindings-check` (CI) détecte toute dérive.
 *
 * Ne restent déclarés ci-dessous que les types purement front, sans équivalent Rust.
 */

export type * from "./bindings/core";
export type * from "./bindings/mobile";

/**
 * Actions d'alimentation proposées par l'app mobile. Côté Rust, la commande `power_action` reçoit
 * une simple chaîne qu'elle relaie à l'API : l'union n'existe que côté front.
 */
export type PowerAction = "start" | "restart" | "stop" | "kill";

/** Type de contenu de la marketplace Minecraft (chaîne côté Rust). */
export type MarketKind = "mods" | "plugins";

/** Catalogues proposés par l'hébergeur (chaîne côté Rust). */
export type MarketSource = "modrinth" | "curseforge" | "spigot";

/** Actions joueur relayées telles quelles à l'API par `player_action` (chaîne côté Rust). */
export type PlayerAction =
  | "kick"
  | "ban"
  | "unban"
  | "op_add"
  | "op_remove"
  | "whitelist_add"
  | "whitelist_remove";
