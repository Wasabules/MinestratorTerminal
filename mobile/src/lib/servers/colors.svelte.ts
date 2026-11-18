/**
 * Biome de chaque serveur (charte « Biome », DESIGN.md) : la palette qui colore sa vue (accent de
 * la barre du bas, sélections), l'en-tête et le paysage en pixels dessiné par
 * `BiomeLandscape.svelte`.
 *
 * Copie du module desktop (`desktop/src/lib/servers/colors.svelte.ts`) : les deux apps n'ont aucun
 * fichier front en commun. Les palettes et `defaultBiomeKey` doivent rester **identiques** : un
 * serveur sans choix a ainsi le même biome sur le téléphone et sur l'ordinateur. Toute évolution
 * ici doit être reportée là-bas, et inversement.
 *
 * Un serveur sans choix reçoit un biome **stable** tiré de son id. Le choix explicite est mémorisé
 * sur l'appareil (localStorage) : il n'est pas synchronisé avec le desktop. Réactif (rune `$state`).
 */

export type BiomeKey = "forest" | "ocean" | "desert" | "snow" | "nether" | "end";

export interface Biome {
  key: BiomeKey;
  /** Accent (thème sombre) : élément actif de la barre du bas, sélections. */
  accent: string;
  /** Accent lisible sur fond clair (texte ≥ 4,5:1 sur blanc) : thème clair. */
  accentLight: string;
  /** Texte posé sur l'accent (thème sombre) : sombre, même sur un accent clair (neige). */
  ink: string;
  /** Ciel du paysage, et fond teinté du biome en thème sombre. */
  sky: string;
  /** Texte posé sur le ciel (fond teinté) en thème sombre. */
  skyText: string;
  /** Fond teinté du biome en thème clair. */
  softLight: string;
  /** Collines lointaines. */
  far: string;
  /** Collines proches. */
  near: string;
  /** Sol du petit bloc en pixels (terre sous l'herbe pour la forêt). */
  ground: string;
  /** Pixels clairsemés du ciel (étoiles, flocons, braises). */
  stars: string;
  /** Décor : couleurs du tronc / de la tige, du corps, de la cime. */
  deco: { stem: string; body: string; top: string };
}

/** Ordre d'affichage (sélecteur de biome). */
export const BIOMES: Biome[] = [
  {
    key: "forest",
    accent: "#3fbf6e",
    accentLight: "#1a6e3f",
    ink: "#0b2414",
    sky: "#13261d",
    skyText: "#8fe8b0",
    softLight: "#e2f1e8",
    far: "#1c3d2c",
    near: "#235237",
    ground: "#6b4a2b",
    stars: "#2f5a45",
    deco: { stem: "#5a3b22", body: "#2f8a4f", top: "#46b26a" },
  },
  {
    key: "ocean",
    accent: "#4aa3ff",
    accentLight: "#1f65b8",
    ink: "#08243f",
    sky: "#0f2133",
    skyText: "#a8d2ff",
    softLight: "#e2edfa",
    far: "#173755",
    near: "#1d4e7a",
    ground: "#1d4e7a",
    stars: "#24466a",
    // Écume des vagues et petits îlots.
    deco: { stem: "#2a6496", body: "#3a7dba", top: "#7fbfff" },
  },
  {
    key: "desert",
    accent: "#e0b34f",
    accentLight: "#8a6100",
    ink: "#2a1d00",
    sky: "#2a2212",
    skyText: "#f2d08a",
    softLight: "#f6ecd6",
    far: "#4a3a1c",
    near: "#7a5a1d",
    ground: "#7a5a1d",
    stars: "#4a3d22",
    // Cactus verts.
    deco: { stem: "#3f8a47", body: "#4f9a52", top: "#6dbb6a" },
  },
  {
    key: "snow",
    accent: "#e6f0f7",
    accentLight: "#4d6782",
    ink: "#0d1113",
    sky: "#1a2430",
    skyText: "#e6f0f7",
    softLight: "#e6edf3",
    far: "#3a4a5c",
    near: "#8fa3b8",
    ground: "#8fa3b8",
    stars: "#3a4a5c",
    // Sapins à cime blanche.
    deco: { stem: "#4a3626", body: "#2e5e4a", top: "#e6f0f7" },
  },
  {
    key: "nether",
    accent: "#ff6b5b",
    accentLight: "#b3302a",
    ink: "#2a0605",
    sky: "#2a0f10",
    skyText: "#ffb3aa",
    softLight: "#fbe5e2",
    far: "#4a1a1a",
    near: "#7a2626",
    ground: "#7a2626",
    stars: "#5a1f1f",
    // Touches de lave.
    deco: { stem: "#c2410c", body: "#ff8a3d", top: "#ffc46b" },
  },
  {
    key: "end",
    accent: "#c79bff",
    accentLight: "#7a43c9",
    ink: "#1e0b3a",
    sky: "#160f24",
    skyText: "#dcc4ff",
    softLight: "#eee5fb",
    far: "#2a1d45",
    near: "#3f2a6b",
    ground: "#3f2a6b",
    stars: "#4a3a70",
    // Plantes de chorus.
    deco: { stem: "#6a4aa0", body: "#8f6ccf", top: "#c79bff" },
  },
];

const BY_KEY: Record<BiomeKey, Biome> = Object.fromEntries(BIOMES.map((b) => [b.key, b])) as Record<
  BiomeKey,
  Biome
>;

function isBiomeKey(v: unknown): v is BiomeKey {
  return typeof v === "string" && v in BY_KEY;
}

/** Même clé que le desktop : chaque app garde la sienne, rien n'est partagé entre appareils. */
const STORAGE_KEY = "mnstr-server-biomes";

function load(): Record<number, BiomeKey> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const map: Record<number, BiomeKey> = {};
    for (const [id, key] of Object.entries(parsed)) if (isBiomeKey(key)) map[Number(id)] = key;
    return map;
  } catch {
    /* localStorage illisible : on repart d'une map vide */
    return {};
  }
}

const store = $state<{ map: Record<number, BiomeKey> }>({ map: load() });

function save(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store.map));
  } catch {
    /* localStorage indisponible : le choix vaut pour la session */
  }
}

/** Biome d'un serveur : son choix, sinon un biome stable tiré de son id. */
export function serverBiome(id: number): Biome {
  return BY_KEY[store.map[id] ?? defaultBiomeKey(id)];
}

/** Le biome du serveur a-t-il été choisi (sinon il découle de son id) ? */
export function hasChosenBiome(id: number): boolean {
  return id in store.map;
}

/** Fixe le biome d'un serveur ; `null` le rend à son biome par défaut. */
export function setServerBiome(id: number, key: BiomeKey | null): void {
  if (key) store.map[id] = key;
  else delete store.map[id];
  save();
}

/**
 * Biome par défaut d'un id : mélange entier (les id voisins ne tombent pas sur des biomes
 * voisins). **Identique au desktop**, à l'opération près : c'est ce qui donne le même biome aux
 * deux apps sans rien synchroniser.
 */
export function defaultBiomeKey(id: number): BiomeKey {
  let h = Math.imul(id ^ 0x5bd1e995, 0x27d4eb2d) >>> 0;
  h = Math.imul(h ^ (h >>> 15), 0x165667b1) >>> 0;
  return BIOMES[((h ^ (h >>> 13)) >>> 0) % BIOMES.length].key;
}

/**
 * Variables CSS de la palette, à poser en `style` sur un conteneur qui porte la classe `.biome`
 * (app.css résout --accent, --on-accent, --accent-soft, --accent-text selon le thème).
 */
export function biomeVars(b: Biome): string {
  return [
    `--b-accent: ${b.accent}`,
    `--b-accent-l: ${b.accentLight}`,
    `--b-ink: ${b.ink}`,
    `--b-soft: ${b.sky}`,
    `--b-soft-text: ${b.skyText}`,
    `--b-soft-l: ${b.softLight}`,
  ].join("; ");
}

// --- Avatars ------------------------------------------------------------------------------

/** Accents des avatars : la neige, presque blanche, disparaîtrait sur le thème clair. */
const AVATAR_BIOMES = BIOMES.filter((b) => b.key !== "snow");

/**
 * Couleurs d'un avatar à initiale (compte, joueur) : un accent de biome tiré du texte, avec son
 * encre sombre. Identique dans les deux thèmes (l'encre reste lisible sur l'accent).
 */
export function avatarStyle(seed: string): string {
  let h = 5381;
  for (let i = 0; i < seed.length; i++) h = ((h << 5) + h + seed.charCodeAt(i)) >>> 0;
  const b = AVATAR_BIOMES[h % AVATAR_BIOMES.length];
  return `background: ${b.accent}; color: ${b.ink}`;
}
