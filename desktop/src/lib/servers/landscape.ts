/**
 * Générateur des paysages en pixels des biomes (charte « Biome », DESIGN.md).
 *
 * Même principe que les maquettes : une hauteur entière par colonne pour chaque plan (collines
 * lointaines, collines proches), puis les colonnes de même hauteur regroupées en un seul
 * rectangle, et **un seul chemin SVG par couleur**. Le résultat se dessine avec
 * `shape-rendering: crispEdges` (voir `BiomeLandscape.svelte`).
 *
 * Tout est tiré d'une graine (l'id du serveur) et de l'abscisse de la colonne : chaque serveur a
 * son propre relief, identique d'une session à l'autre, et élargir la fenêtre dévoile des
 * colonnes de plus sans redessiner celles déjà visibles.
 */

import type { Biome, BiomeKey } from './colors.svelte.ts';

/** Chemins SVG (attribut `d`) d'un paysage, un par couleur. */
export interface Landscape {
  stars: string;
  far: string;
  near: string;
  stem: string;
  body: string;
  top: string;
}

/** Bruit déterministe dans [0, 1) pour (graine, colonne, canal). */
function rnd(seed: number, x: number, salt: number): number {
  let h = Math.imul(seed ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(x + 0x632be5ab, 0xc2b2ae35);
  h ^= Math.imul(salt + 1, 0x27d4eb2f);
  h ^= h >>> 15;
  h = Math.imul(h, 0x2c1b3c6d);
  h ^= h >>> 12;
  h = Math.imul(h, 0x297a2d39);
  h ^= h >>> 15;
  return (h >>> 0) / 4294967296;
}

/**
 * Relief d'un plan, en fraction du nombre de lignes : `base` + deux sinusoïdes (amplitudes `a1`,
 * `a2`, périodes `p1`, `p2` en lignes, pour garder les proportions quelle que soit la hauteur)
 * et un grain aléatoire `jag` (Nether : relief déchiqueté).
 */
interface Relief {
  base: number;
  a1: number;
  p1: number;
  a2: number;
  p2: number;
  jag?: number;
}

interface Shape {
  far: Relief;
  near: Relief;
  /** Probabilité qu'une colonne porte un décor, et écart minimal entre deux décors. */
  deco: number;
  gap: number;
  /** Probabilité qu'une colonne porte un pixel de ciel (étoile, braise). */
  stars: number;
}

const SHAPES: Record<BiomeKey, Shape> = {
  forest: {
    far: { base: 0.38, a1: 0.09, p1: 0.4, a2: 0.06, p2: 0.17 },
    near: { base: 0.19, a1: 0.06, p1: 0.28, a2: 0.045, p2: 0.12 },
    deco: 0.09,
    gap: 6,
    stars: 0.04,
  },
  // Plan proche quasi plat : la mer ; le plan lointain fait des îles.
  ocean: {
    far: { base: 0.26, a1: 0.1, p1: 0.5, a2: 0.05, p2: 0.2 },
    near: { base: 0.17, a1: 0.012, p1: 0.4, a2: 0.008, p2: 0.15 },
    deco: 0.16,
    gap: 3,
    stars: 0.05,
  },
  // Dunes : grandes périodes, peu de grain.
  desert: {
    far: { base: 0.32, a1: 0.08, p1: 0.75, a2: 0.03, p2: 0.3 },
    near: { base: 0.19, a1: 0.07, p1: 0.55, a2: 0.025, p2: 0.22 },
    deco: 0.05,
    gap: 9,
    stars: 0.04,
  },
  // Montagnes enneigées au loin.
  snow: {
    far: { base: 0.42, a1: 0.16, p1: 0.45, a2: 0.08, p2: 0.16 },
    near: { base: 0.2, a1: 0.05, p1: 0.3, a2: 0.04, p2: 0.13 },
    deco: 0.09,
    gap: 6,
    stars: 0.05,
  },
  nether: {
    far: { base: 0.46, a1: 0.1, p1: 0.35, a2: 0.07, p2: 0.12, jag: 0.06 },
    near: { base: 0.17, a1: 0.05, p1: 0.25, a2: 0.04, p2: 0.1, jag: 0.04 },
    deco: 0.06,
    gap: 8,
    stars: 0.03,
  },
  // Îles basses dans le vide, beaucoup d'étoiles.
  end: {
    far: { base: 0.22, a1: 0.07, p1: 0.45, a2: 0.04, p2: 0.18 },
    near: { base: 0.14, a1: 0.04, p1: 0.3, a2: 0.03, p2: 0.12 },
    deco: 0.06,
    gap: 7,
    stars: 0.09,
  },
};

/** Hauteurs (en pixels depuis le bas) d'un plan, une par colonne. */
function heights(r: Relief, seed: number, salt: number, cols: number, rows: number): number[] {
  const ph1 = rnd(seed, -1, salt) * Math.PI * 2;
  const ph2 = rnd(seed, -2, salt) * Math.PI * 2;
  const p1 = Math.max(1, r.p1 * rows * (0.85 + rnd(seed, -3, salt) * 0.3));
  const p2 = Math.max(1, r.p2 * rows * (0.85 + rnd(seed, -4, salt) * 0.3));
  const out: number[] = [];
  for (let x = 0; x < cols; x++) {
    let v = r.base + r.a1 * Math.sin(x / p1 + ph1) + r.a2 * Math.sin(x / p2 + ph2);
    if (r.jag) v += (rnd(seed, x, salt + 10) - 0.5) * r.jag;
    out.push(Math.max(1, Math.round(v * rows)));
  }
  return out;
}

/** Colonnes de même hauteur regroupées en rectangles, dans un seul chemin. */
function runs(hs: number[], rows: number): string {
  let d = '';
  let x = 0;
  while (x < hs.length) {
    let e = x;
    while (e + 1 < hs.length && hs[e + 1] === hs[x]) e++;
    const h = hs[x];
    const w = e - x + 1;
    if (h > 0) d += `M${x} ${rows - h}h${w}v${h}h-${w}z`;
    x = e + 1;
  }
  return d;
}

/** Rectangle de pixels (`w` × `h`) en coin haut-gauche (x, y). */
function px(x: number, y: number, w = 1, h = 1): string {
  return `M${x} ${y}h${w}v${h}h-${w}z`;
}

/** Paysage d'un biome sur `cols` × `rows` pixels, graine `seed` (l'id du serveur). */
export function landscape(biome: Biome, seed: number, cols: number, rows: number): Landscape {
  const shape = SHAPES[biome.key];
  const far = heights(shape.far, seed, 1, cols, rows);
  const near = heights(shape.near, seed, 2, cols, rows);
  // Les collines proches restent devant : le plan lointain les dépasse toujours d'au moins 2.
  for (let x = 0; x < cols; x++) far[x] = Math.max(far[x], near[x] + 2);

  let stars = '';
  for (let x = 0; x < cols; x++) {
    if (rnd(seed, x, 3) >= shape.stars) continue;
    const y = Math.floor(rnd(seed, x, 4) * rows * 0.4);
    if (y < rows - far[x] - 1) stars += px(x, y);
  }

  let stem = '';
  let body = '';
  let top = '';
  let last = -Infinity;
  for (let x = 2; x < cols - 2; x++) {
    if (x - last < shape.gap || rnd(seed, x, 5) >= shape.deco) continue;
    last = x;
    const g = rows - near[x]; // première ligne du sol sous la colonne
    const tall = rnd(seed, x, 6) < 0.5 ? 0 : 1;
    switch (biome.key) {
      case 'forest': {
        // Arbre : tronc, feuillage 3×3, cime.
        const th = 2 + tall;
        stem += px(x, g - th, 1, th);
        body += px(x - 1, g - th - 3, 3, 3);
        top += px(x, g - th - 4);
        break;
      }
      case 'snow': {
        // Sapin à cime blanche.
        stem += px(x, g - 1);
        body += px(x - 1, g - 3 - tall, 3, 2 + tall) + px(x, g - 5 - tall, 1, 2);
        top += px(x, g - 6 - tall) + px(x - 1, g - 3 - tall) + px(x + 1, g - 3 - tall);
        break;
      }
      case 'desert': {
        // Cactus : tige, un ou deux bras.
        const h = 3 + tall;
        body += px(x, g - h, 1, h);
        if (rnd(seed, x, 7) < 0.7) body += px(x - 1, g - 2) + px(x - 2, g - 3, 1, 2);
        if (rnd(seed, x, 8) < 0.5) body += px(x + 1, g - 3) + px(x + 2, g - 4, 1, 2);
        top += px(x, g - h);
        break;
      }
      case 'ocean': {
        // Écume à la surface et reflets sous l'eau.
        top += px(x, g, 2, 1);
        if (rnd(seed, x, 7) < 0.5) body += px(x + 3, g + 2);
        if (rnd(seed, x, 8) < 0.25) stem += px(x - 1, g + 4, 2, 1);
        break;
      }
      case 'nether': {
        // Bassin de lave affleurant, avec un éclat.
        const w = 2 + Math.floor(rnd(seed, x, 7) * 3);
        stem += px(x - 1, g + 1, w + 2, 1);
        body += px(x, g, w, 1);
        top += px(x + Math.floor(w / 2), g);
        break;
      }
      case 'end': {
        // Plante de chorus : tige, rameaux, fleur.
        const h = 3 + tall + (rnd(seed, x, 7) < 0.3 ? 1 : 0);
        stem += px(x, g - h, 1, h);
        body += px(x + 1, g - h + 1) + px(x - 1, g - h + 2);
        top += px(x, g - h - 1);
        break;
      }
    }
  }

  return { stars, far: runs(far, rows), near: runs(near, rows), stem, body, top };
}
