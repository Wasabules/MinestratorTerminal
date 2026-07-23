/**
 * Commandes Minecraft de base pour l'auto-complétion de la console (gain de temps au tactile).
 * On complète le premier mot ; l'utilisateur poursuit les arguments. Liste vanilla courante.
 */
export const MC_COMMANDS: string[] = [
  "help",
  "list",
  "say",
  "tell",
  "me",
  "tp",
  "teleport",
  "give",
  "gamemode",
  "defaultgamemode",
  "time set day",
  "time set night",
  "time add",
  "weather clear",
  "weather rain",
  "weather thunder",
  "difficulty",
  "gamerule",
  "kill",
  "clear",
  "xp",
  "experience",
  "effect give",
  "effect clear",
  "enchant",
  "summon",
  "setblock",
  "fill",
  "clone",
  "setworldspawn",
  "spawnpoint",
  "worldborder",
  "seed",
  "save-all",
  "save-on",
  "save-off",
  "stop",
  "reload",
  "whitelist add",
  "whitelist remove",
  "whitelist list",
  "whitelist on",
  "whitelist off",
  "op",
  "deop",
  "ban",
  "ban-ip",
  "pardon",
  "kick",
  "banlist",
  "tellraw",
  "title",
  "playsound",
  "particle",
  "scoreboard",
  "team",
  "advancement",
  "attribute",
  "data",
  "datapack",
  "function",
  "locate",
  "spectate",
  "spreadplayers",
  "trigger",
  "forceload",
  "loot",
  "recipe",
  "schedule",
  "tag",
];

/** Suggestions pour la saisie courante (préfixe insensible à la casse). Vide si rien de pertinent. */
export function suggest(input: string, limit = 8): string[] {
  const raw = input.replace(/^\//, "");
  const q = raw.toLowerCase();
  if (q === "") {
    // Écran vide : quelques commandes fréquentes.
    return ["list", "say ", "tp ", "gamemode ", "time set day", "weather clear", "save-all", "stop"];
  }
  // Ne suggère plus une fois qu'un argument est tapé (espace après une commande complète connue).
  const startsWith = MC_COMMANDS.filter((c) => c.toLowerCase().startsWith(q) && c.toLowerCase() !== q);
  return startsWith.slice(0, limit);
}
