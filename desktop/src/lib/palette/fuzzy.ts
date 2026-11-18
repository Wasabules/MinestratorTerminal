/**
 * Recherche floue de la palette de commandes : pure, sans dépendance (testable hors Svelte).
 *
 * - **Tolérante** : insensible à la casse et aux accents (« demarrer » trouve « Démarrer »), aux
 *   apostrophes typographiques, et aux ligatures courantes (œ, æ, ß).
 * - **Sous-séquences** : « svpb » trouve « Survie Publique » ; les lettres doivent apparaître dans
 *   l'ordre, pas forcément côte à côte.
 * - **Multi-mots** : chaque mot de la requête doit correspondre à l'un des champs (titre, contexte,
 *   mots-clés), dans n'importe quel ordre : « console survie » et « survie console » se valent.
 * - **Classement** : une correspondance contiguë, en début de mot ou de texte, et dans le titre
 *   plutôt que dans le contexte, l'emporte sur des lettres éparpillées.
 *
 * Les indices renvoyés pointent dans le texte **d'origine** (pas le texte normalisé), pour la mise
 * en évidence des caractères trouvés.
 */

/** Texte replié + correspondance « indice replié → indice d'origine ». */
interface Folded {
  text: string;
  map: number[];
}

const charCache = new Map<string, string>();

/** Replie un caractère : minuscule, sans diacritique, ponctuation typographique ramenée à l'ASCII. */
function foldChar(ch: string): string {
  let out = charCache.get(ch);
  if (out !== undefined) return out;
  out = ch.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
  if (out === '’' || out === '‘' || out === 'ʼ') out = "'";
  else if (out === 'œ') out = 'oe';
  else if (out === 'æ') out = 'ae';
  else if (out === 'ß') out = 'ss';
  charCache.set(ch, out);
  return out;
}

const foldCache = new Map<string, Folded>();

function fold(s: string): Folded {
  let f = foldCache.get(s);
  if (f) return f;
  let text = '';
  const map: number[] = [];
  for (let i = 0; i < s.length; i++) {
    for (const c of foldChar(s[i])) {
      text += c;
      map.push(i);
    }
  }
  f = { text, map };
  // Borne grossière : les textes indexés (titres, noms) sont en nombre limité.
  if (foldCache.size > 4000) foldCache.clear();
  foldCache.set(s, f);
  return f;
}

/** Forme repliée d'une requête (même repli que les textes indexés). */
export function foldQuery(q: string): string {
  return fold(q).text;
}

const SEPARATORS = new Set([' ', '-', '_', '.', '/', ':', "'", '(', '[', '«', '"', '·', '›', '>']);

function isBoundary(text: string, i: number): boolean {
  return i === 0 || SEPARATORS.has(text[i - 1]);
}

interface TokenMatch {
  score: number;
  /** Indices dans le texte replié. */
  idx: number[];
}

/** Correspondance contiguë : la meilleure occurrence (début de texte > début de mot > première). */
function substringMatch(q: string, h: string): TokenMatch | null {
  let best = -1;
  let bestBonus = -1;
  for (let at = h.indexOf(q); at !== -1; at = h.indexOf(q, at + 1)) {
    const bonus = at === 0 ? 2 : isBoundary(h, at) ? 1 : 0;
    if (bonus > bestBonus) {
      best = at;
      bestBonus = bonus;
      if (bonus === 2) break;
    }
  }
  if (best === -1) return null;
  let score = q.length * 4 + (bestBonus === 2 ? 14 : bestBonus === 1 ? 9 : 0);
  if (q.length === h.length) score += 10; // correspondance exacte
  score -= Math.min(best, 20) * 0.15;
  return { score, idx: Array.from({ length: q.length }, (_, k) => best + k) };
}

/**
 * Sous-séquence, façon fzf v1 : passe avant gloutonne pour trouver la fin, passe arrière pour la
 * fenêtre la plus courte, puis relevé des indices. Bonus aux lettres consécutives et aux débuts de
 * mot (acronymes : « sp » → « Survie Publique »), pénalité légère pour les trous.
 *
 * `strict` (champs secondaires) : chaque lettre doit suivre la précédente ou ouvrir un mot. Sans
 * cela, des mots-clés longs et cachés captent n'importe quelle suite de lettres éparpillées.
 */
function subsequenceMatch(q: string, h: string, strict: boolean): TokenMatch | null {
  let qi = 0;
  let end = -1;
  for (let i = 0; i < h.length && qi < q.length; i++) {
    if (h[i] === q[qi]) {
      qi++;
      if (qi === q.length) end = i;
    }
  }
  if (end === -1) return null;
  let start = end;
  qi = q.length - 1;
  for (let i = end; i >= 0; i--) {
    if (h[i] === q[qi]) {
      start = i;
      if (--qi < 0) break;
    }
  }
  const idx: number[] = [];
  qi = 0;
  for (let i = start; i <= end && qi < q.length; i++) {
    if (h[i] === q[qi]) {
      idx.push(i);
      qi++;
    }
  }
  let score = 0;
  for (let k = 0; k < idx.length; k++) {
    const consecutive = k > 0 && idx[k] === idx[k - 1] + 1;
    const boundary = isBoundary(h, idx[k]);
    if (strict && !consecutive && !boundary) return null;
    score += 1;
    if (consecutive) score += 3;
    if (boundary) score += 4;
  }
  if (idx[0] === 0) score += 4;
  const gaps = idx[idx.length - 1] - idx[0] + 1 - idx.length;
  score -= Math.min(gaps * 0.3, 8);
  return { score, idx };
}

function matchToken(q: string, h: string, strict: boolean): TokenMatch | null {
  if (!q || !h) return null;
  return substringMatch(q, h) ?? subsequenceMatch(q, h, strict);
}

export interface FuzzyResult {
  score: number;
  /** Indices trouvés (texte d'origine), par champ, dans l'ordre des `fields` fournis. */
  hits: number[][];
}

/** Poids par défaut : titre, contexte, mots-clés (non affichés). */
const DEFAULT_WEIGHTS = [1, 0.7, 0.5];

/**
 * Compare une requête à plusieurs champs d'une entrée. `null` si un mot de la requête ne
 * correspond à aucun champ. Requête vide : correspondance neutre (score 0, aucun indice).
 */
export function fuzzyMatch(
  query: string,
  fields: (string | null | undefined)[],
  weights: number[] = DEFAULT_WEIGHTS
): FuzzyResult | null {
  const q = foldQuery(query).trim();
  const hits: number[][] = fields.map(() => []);
  if (!q) return { score: 0, hits };
  const folded = fields.map((f) => fold(f ?? ''));
  const tokens = q.split(/\s+/);
  let total = 0;
  for (const token of tokens) {
    let bestScore = -Infinity;
    let bestField = -1;
    let bestIdx: number[] = [];
    for (let fi = 0; fi < folded.length; fi++) {
      // Le titre (premier champ) tolère les lettres éparpillées ; les autres champs, non.
      const m = matchToken(token, folded[fi].text, fi > 0);
      if (!m) continue;
      const score = m.score * (weights[fi] ?? 0.5);
      if (score > bestScore) {
        bestScore = score;
        bestField = fi;
        bestIdx = m.idx;
      }
    }
    if (bestField === -1) return null;
    total += bestScore;
    const map = folded[bestField].map;
    for (const i of bestIdx) hits[bestField].push(map[i]);
  }
  // Requête multi-mots retrouvée telle quelle dans le titre : bonus de cohérence.
  if (tokens.length > 1 && folded[0]?.text.includes(q)) total += 10;
  return { score: total, hits: hits.map((h) => [...new Set(h)].sort((a, b) => a - b)) };
}

export interface Segment {
  text: string;
  hit: boolean;
}

/** Découpe un texte en segments « trouvé / non trouvé » pour la mise en évidence. */
export function highlight(text: string, hits: number[] | undefined): Segment[] {
  if (!hits || hits.length === 0) return [{ text, hit: false }];
  const set = new Set(hits);
  const out: Segment[] = [];
  for (let i = 0; i < text.length; i++) {
    const hit = set.has(i);
    const last = out[out.length - 1];
    if (last && last.hit === hit) last.text += text[i];
    else out.push({ text: text[i], hit });
  }
  return out;
}
