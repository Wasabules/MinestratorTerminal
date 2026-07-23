/**
 * Auto-complétion des commandes Minecraft, hiérarchique + triée par fréquence d'usage.
 *
 * `MC_ENTRIES` = liste plate de « chemins » de commandes canoniques (avec sous-commandes),
 * ce qui donne naturellement la complétion en cascade : taper `whitelist` propose
 * `whitelist add/remove/list/on/off/reload`. Les suggestions sont classées par nombre
 * d'utilisations (persisté en localStorage), puis par une priorité par défaut, puis alpha.
 */

export const MC_ENTRIES: string[] = [
  // Gestion joueurs / modération
  "op",
  "deop",
  "kick",
  "ban",
  "ban-ip",
  "pardon",
  "pardon-ip",
  "banlist players",
  "banlist ips",
  "whitelist add",
  "whitelist remove",
  "whitelist list",
  "whitelist on",
  "whitelist off",
  "whitelist reload",
  "list",
  "setidletimeout",
  // Communication
  "say",
  "tell",
  "msg",
  "me",
  "tellraw",
  "title title",
  "title subtitle",
  "title actionbar",
  "title clear",
  "title reset",
  "title times",
  // Joueur / déplacement
  "tp",
  "teleport",
  "spawnpoint",
  "setworldspawn",
  "spectate",
  "gamemode survival",
  "gamemode creative",
  "gamemode adventure",
  "gamemode spectator",
  "defaultgamemode survival",
  "defaultgamemode creative",
  "defaultgamemode adventure",
  "defaultgamemode spectator",
  "kill",
  "clear",
  "give",
  "enchant",
  "xp add",
  "xp set",
  "xp query",
  "experience add",
  "experience set",
  "experience query",
  "effect give",
  "effect clear",
  // Monde
  "time set day",
  "time set night",
  "time set noon",
  "time set midnight",
  "time add",
  "time query daytime",
  "time query gametime",
  "time query day",
  "weather clear",
  "weather rain",
  "weather thunder",
  "difficulty peaceful",
  "difficulty easy",
  "difficulty normal",
  "difficulty hard",
  "gamerule",
  "worldborder add",
  "worldborder set",
  "worldborder center",
  "worldborder get",
  "worldborder damage",
  "worldborder warning",
  "seed",
  "setblock",
  "fill",
  "clone",
  "summon",
  "particle",
  "playsound",
  "forceload add",
  "forceload remove",
  "forceload query",
  "locate structure",
  "locate biome",
  "locate poi",
  "spreadplayers",
  // Scoreboard / données / avancé
  "scoreboard objectives",
  "scoreboard players",
  "team add",
  "team remove",
  "team empty",
  "team join",
  "team leave",
  "team list",
  "team modify",
  "advancement grant",
  "advancement revoke",
  "attribute",
  "data get",
  "data merge",
  "data modify",
  "data remove",
  "datapack enable",
  "datapack disable",
  "datapack list",
  "function",
  "recipe give",
  "recipe take",
  "schedule function",
  "schedule clear",
  "tag add",
  "tag remove",
  "tag list",
  "trigger",
  "loot",
  "execute",
  // Serveur
  "save-all",
  "save-on",
  "save-off",
  "stop",
  "reload",
  "help",
];

// Priorité par défaut (avant tout usage) pour les commandes les plus courantes.
const DEFAULT_PRIORITY: Record<string, number> = {
  list: 100,
  "say": 95,
  "tp": 90,
  "gamemode creative": 85,
  "gamemode survival": 84,
  "time set day": 80,
  "weather clear": 78,
  "whitelist add": 75,
  op: 74,
  kick: 70,
  ban: 68,
  "save-all": 65,
  stop: 60,
  give: 58,
  pardon: 40,
};

// --- Fréquence d'usage (persistée) ---
let freq: Record<string, number> = load();

function load(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem("mc.freq") || "{}");
  } catch {
    return {};
  }
}
function save() {
  try {
    localStorage.setItem("mc.freq", JSON.stringify(freq));
  } catch {
    /* ignore */
  }
}

/** Enregistre l'usage d'une commande envoyée (incrémente l'entrée canonique la plus longue). */
export function recordUsage(command: string) {
  const c = command.trim().replace(/^\//, "").toLowerCase();
  if (!c) return;
  let best = "";
  for (const e of MC_ENTRIES) {
    const el = e.toLowerCase();
    if ((c === el || c.startsWith(el + " ")) && el.length > best.length) best = el;
  }
  if (best) {
    freq[best] = (freq[best] ?? 0) + 1;
    save();
  }
}

/** Suggestions pour la saisie courante, classées par fréquence puis priorité par défaut. */
export function suggest(input: string, limit = 8): string[] {
  const q = input.replace(/^\//, "").toLowerCase();
  const cands =
    q === ""
      ? [...MC_ENTRIES]
      : MC_ENTRIES.filter((e) => e.toLowerCase().startsWith(q) && e.toLowerCase() !== q);
  cands.sort((a, b) => {
    const fa = freq[a.toLowerCase()] ?? 0;
    const fb = freq[b.toLowerCase()] ?? 0;
    if (fb !== fa) return fb - fa;
    const pa = DEFAULT_PRIORITY[a] ?? 0;
    const pb = DEFAULT_PRIORITY[b] ?? 0;
    if (pb !== pa) return pb - pa;
    return a.localeCompare(b);
  });
  return cands.slice(0, limit);
}
