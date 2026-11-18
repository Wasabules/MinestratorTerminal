/**
 * Biome de chaque serveur (charte « Biome », DESIGN.md) : la palette qui colore ses onglets,
 * l'en-tête de son tableau de bord et le paysage en pixels dessiné par `BiomeLandscape.svelte`.
 *
 * Un serveur sans choix reçoit un biome **stable** tiré de son id : il garde ses couleurs d'une
 * session à l'autre sans rien enregistrer. Le choix explicite est persisté en localStorage.
 * Réactif (rune `$state`).
 */

export type BiomeKey = 'forest' | 'ocean' | 'desert' | 'snow' | 'nether' | 'end';

export interface Biome {
  key: BiomeKey;
  /** Accent (thème sombre) : bord des onglets, sélections. */
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
  /** Sol du petit bloc en pixels de la tête de groupe (terre sous l'herbe pour la forêt). */
  ground: string;
  /** Pixels clairsemés du ciel (étoiles, flocons, braises). */
  stars: string;
  /** Décor : couleurs du tronc / de la tige, du corps, de la cime. */
  deco: { stem: string; body: string; top: string };
}

/** Ordre d'affichage (sélecteur de biome). */
export const BIOMES: Biome[] = [
  {
    key: 'forest',
    accent: '#3fbf6e',
    accentLight: '#1a6e3f',
    ink: '#0b2414',
    sky: '#13261d',
    skyText: '#8fe8b0',
    softLight: '#e2f1e8',
    far: '#1c3d2c',
    near: '#235237',
    ground: '#6b4a2b',
    stars: '#2f5a45',
    deco: { stem: '#5a3b22', body: '#2f8a4f', top: '#46b26a' },
  },
  {
    key: 'ocean',
    accent: '#4aa3ff',
    accentLight: '#1f65b8',
    ink: '#08243f',
    sky: '#0f2133',
    skyText: '#a8d2ff',
    softLight: '#e2edfa',
    far: '#173755',
    near: '#1d4e7a',
    ground: '#1d4e7a',
    stars: '#24466a',
    // Écume des vagues et petits îlots.
    deco: { stem: '#2a6496', body: '#3a7dba', top: '#7fbfff' },
  },
  {
    key: 'desert',
    accent: '#e0b34f',
    accentLight: '#8a6100',
    ink: '#2a1d00',
    sky: '#2a2212',
    skyText: '#f2d08a',
    softLight: '#f6ecd6',
    far: '#4a3a1c',
    near: '#7a5a1d',
    ground: '#7a5a1d',
    stars: '#4a3d22',
    // Cactus verts.
    deco: { stem: '#3f8a47', body: '#4f9a52', top: '#6dbb6a' },
  },
  {
    key: 'snow',
    accent: '#e6f0f7',
    accentLight: '#4d6782',
    ink: '#0d1113',
    sky: '#1a2430',
    skyText: '#e6f0f7',
    softLight: '#e6edf3',
    far: '#3a4a5c',
    near: '#8fa3b8',
    ground: '#8fa3b8',
    stars: '#3a4a5c',
    // Sapins à cime blanche.
    deco: { stem: '#4a3626', body: '#2e5e4a', top: '#e6f0f7' },
  },
  {
    key: 'nether',
    accent: '#ff6b5b',
    accentLight: '#b3302a',
    ink: '#2a0605',
    sky: '#2a0f10',
    skyText: '#ffb3aa',
    softLight: '#fbe5e2',
    far: '#4a1a1a',
    near: '#7a2626',
    ground: '#7a2626',
    stars: '#5a1f1f',
    // Touches de lave.
    deco: { stem: '#c2410c', body: '#ff8a3d', top: '#ffc46b' },
  },
  {
    key: 'end',
    accent: '#c79bff',
    accentLight: '#7a43c9',
    ink: '#1e0b3a',
    sky: '#160f24',
    skyText: '#dcc4ff',
    softLight: '#eee5fb',
    far: '#2a1d45',
    near: '#3f2a6b',
    ground: '#3f2a6b',
    stars: '#4a3a70',
    // Plantes de chorus.
    deco: { stem: '#6a4aa0', body: '#8f6ccf', top: '#c79bff' },
  },
];

const BY_KEY: Record<BiomeKey, Biome> = Object.fromEntries(BIOMES.map((b) => [b.key, b])) as Record<
  BiomeKey,
  Biome
>;

function isBiomeKey(v: unknown): v is BiomeKey {
  return typeof v === 'string' && v in BY_KEY;
}

const STORAGE_KEY = 'mnstr-server-biomes';
/** Ancien stockage : une couleur hex par serveur, migrée vers le biome le plus proche. */
const LEGACY_KEY = 'mnstr-server-colors';

const store = $state<{ map: Record<number, BiomeKey> }>({ map: {} });

function save(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store.map));
  } catch {
    /* localStorage indisponible : le choix vaut pour la session */
  }
}

export function initColors(): void {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as Record<string, unknown>;
      const map: Record<number, BiomeKey> = {};
      for (const [id, key] of Object.entries(parsed)) if (isBiomeKey(key)) map[Number(id)] = key;
      store.map = map;
    }
  } catch {
    /* localStorage illisible : on repart d'une map vide */
  }
  migrateLegacy();
}

/**
 * Les couleurs choisies avant la charte Biome (hex) deviennent le biome le plus proche. Un choix
 * déjà fait dans le nouveau format l'emporte. L'ancienne clé est retirée une fois lue, même
 * illisible : elle ne sert plus à rien.
 */
function migrateLegacy(): void {
  try {
    const raw = localStorage.getItem(LEGACY_KEY);
    if (!raw) return;
    localStorage.removeItem(LEGACY_KEY);
    const legacy = JSON.parse(raw) as Record<string, unknown>;
    for (const [id, hex] of Object.entries(legacy)) {
      const n = Number(id);
      if (!Number.isFinite(n) || store.map[n] || typeof hex !== 'string') continue;
      const key = nearestBiome(hex);
      if (key) store.map[n] = key;
    }
    save();
  } catch {
    /* ancienne valeur illisible : les serveurs gardent leur biome par défaut */
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

/** Accent du biome du serveur (compatibilité avec l'ancienne couleur de serveur). */
export function serverColor(id: number): string {
  return serverBiome(id).accent;
}

/** Fixe le biome d'un serveur ; `null` le rend à son biome par défaut. */
export function setServerBiome(id: number, key: BiomeKey | null): void {
  if (key) store.map[id] = key;
  else delete store.map[id];
  save();
}

/** Biome par défaut d'un id : mélange entier (les id voisins ne tombent pas sur des biomes voisins). */
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
  ].join('; ');
}

// --- Biome le plus proche d'une couleur (migration) -------------------------------------

function toOklab(hex: string): [number, number, number] | null {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  const lin = (c: number) => {
    const v = c / 255;
    return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  const r = lin((n >> 16) & 255);
  const g = lin((n >> 8) & 255);
  const b = lin(n & 255);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const mm = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  return [
    0.2104542553 * l + 0.793617785 * mm - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * mm + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * mm - 0.808675766 * s,
  ];
}

/**
 * Biome dont l'accent est le plus proche d'une couleur, dans l'espace perceptif OKLab ; la teinte
 * compte plus que la clarté (un bleu ardoise devient océan, pas neige).
 */
export function nearestBiome(hex: string): BiomeKey | null {
  const c = toOklab(hex);
  if (!c) return null;
  let best: BiomeKey | null = null;
  let bestD = Infinity;
  for (const b of BIOMES) {
    const a = toOklab(b.accent)!;
    const d = Math.hypot((c[0] - a[0]) * 0.5, c[1] - a[1], c[2] - a[2]);
    if (d < bestD) {
      bestD = d;
      best = b.key;
    }
  }
  return best;
}

// --- Avatars ------------------------------------------------------------------------------

/** Accents des avatars : la neige, presque blanche, disparaîtrait sur le thème clair. */
const AVATAR_BIOMES = BIOMES.filter((b) => b.key !== 'snow');

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
