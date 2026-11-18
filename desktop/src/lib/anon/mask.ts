/**
 * Masques du mode anonyme (`anon.svelte.ts`) : fonctions pures, sans état.
 *
 * Masquer, pas falsifier : là où le mode démo (`fake.svelte.ts`) invente des valeurs plausibles,
 * le mode anonyme remplace la valeur par un masque lisible (`•••.•••.•••.•••`). On voit qu'une
 * adresse est là, jamais laquelle, et rien ne se reconstitue depuis la capture, contrairement à
 * un flou. Le port est gardé : seul, il n'identifie rien et aide à se repérer.
 *
 * Les motifs IPv4 et e-mail sont ceux de l'anonymisation de la console (`redact.ts`).
 */

import { EMAIL_PATTERN, IPV4_PATTERN } from '#lib/redact.ts';

export const IPV4_MASK = '•••.•••.•••.•••';
export const IPV6_MASK = '••••:••••::••••';
export const EMAIL_MASK = '•••@•••';
const LABEL_MASK = '••••••';
/** Domaine qui n'est pas celui de l'hébergeur : rien n'en est gardé, pas même l'extension. */
export const DOMAIN_MASK = '••••••.•••';

/**
 * Domaines de l'hébergeur : c'est le sous-domaine qui désigne le serveur. Le suffixe est gardé
 * pour que le masque reste parlant (`••••••.minesr.com`).
 */
const HOSTER_DOMAINS = ['minesr.com', 'minestrator.com'];

const HEX = '[0-9A-Fa-f]{1,4}';
/**
 * IPv6 : forme complète (8 groupes) ou abrégée (`::`). Un horodatage `12:34:56` n'a ni l'une ni
 * l'autre. Le caractère qui précède est vérifié à part (`maskFor`) : pas de lookbehind, encore
 * absent de certains WebKitGTK.
 */
const IPV6_PATTERN = `(?:${HEX}(?::${HEX}){7}|${HEX}(?::${HEX}){0,6}::(?:${HEX}(?::${HEX}){0,6})?|::${HEX}(?::${HEX}){0,6})(?![0-9A-Za-z:.])`;
/**
 * Adresse de serveur de l'hébergeur dans un texte libre (`xxx.minesr.com`). Les autres domaines
 * ne sont pas touchés : ce sont des URL publiques (mclogs, CDN) ou des noms de paquets Java.
 */
const HOSTER_HOST_PATTERN = String.raw`\b(?:[A-Za-z0-9-]{1,63}\.){1,8}minesr\.com\b`;

/**
 * Tout ce qui se masque dans un texte libre. Le `\b` devant l'e-mail et les longueurs bornées
 * limitent le travail par point de départ : une très longue ligne (fichier minifié) reste rapide.
 */
const SENSITIVE = new RegExp(
  `(?<email>\\b${EMAIL_PATTERN})|(?<host>${HOSTER_HOST_PATTERN})|(?<v4>${IPV4_PATTERN})|(?<v6>${IPV6_PATTERN})`,
  'gi',
);
const IPV4_ONLY = new RegExp(`^${IPV4_PATTERN}$`);
const IPV6_PREV = /[0-9A-Za-z:.]/;

export interface Segment {
  text: string;
  masked: boolean;
}

/** Masque d'une correspondance de `SENSITIVE`, ou `null` si ce n'en est pas vraiment une. */
function maskFor(m: RegExpMatchArray, input: string): string | null {
  const g = m.groups ?? {};
  if (g.email) return EMAIL_MASK;
  if (g.host) return `${LABEL_MASK}.minesr.com`;
  if (g.v4) return IPV4_MASK;
  if (g.v6) {
    // Collé à un mot (`std::abc`) ou sans aucun chiffre (`Cafe::Bad`, méthode Java ou C++) :
    // un identifiant, pas une adresse.
    const prev = input[(m.index ?? 0) - 1];
    if ((prev && IPV6_PREV.test(prev)) || !/\d/.test(m[0])) return null;
    return IPV6_MASK;
  }
  return null;
}

/** Correspondances à masquer dans `text`, avec leur position et leur masque. */
export function sensitiveMatches(text: string): { from: number; to: number; mask: string }[] {
  const out: { from: number; to: number; mask: string }[] = [];
  for (const m of text.matchAll(SENSITIVE)) {
    const mask = maskFor(m, text);
    if (mask === null) continue;
    const from = m.index ?? 0;
    out.push({ from, to: from + m[0].length, mask });
  }
  return out;
}

/** Découpe un texte libre en segments, masqués ou non (rendu avec un style propre aux masques). */
export function maskSegments(text: string): Segment[] {
  const out: Segment[] = [];
  let last = 0;
  for (const { from, to, mask } of sensitiveMatches(text)) {
    if (from > last) out.push({ text: text.slice(last, from), masked: false });
    out.push({ text: mask, masked: true });
    last = to;
  }
  if (last < text.length) out.push({ text: text.slice(last), masked: false });
  return out;
}

/** Texte libre (ligne de console, message, journal) : IP, e-mails et adresses de serveurs masqués. */
export function maskText(text: string): string {
  if (!text) return text;
  let out = '';
  let last = 0;
  for (const { from, to, mask } of sensitiveMatches(text)) {
    out += text.slice(last, from) + mask;
    last = to;
  }
  return last === 0 ? text : out + text.slice(last);
}

/** Le texte contient-il quelque chose que le mode anonyme masquerait ? */
export function hasSensitive(text: string | null | undefined): boolean {
  return !!text && sensitiveMatches(text).length > 0;
}

/** Hôte seul (IP ou domaine). */
export function maskHost(host: string): string {
  if (IPV4_ONLY.test(host)) return IPV4_MASK;
  const h = host.toLowerCase().replace(/\.$/, '');
  for (const d of HOSTER_DOMAINS) {
    if (h === d) return host; // le domaine nu de l'hébergeur ne désigne aucun serveur
    if (h.endsWith(`.${d}`)) return `${LABEL_MASK}.${d}`;
  }
  return DOMAIN_MASK;
}

/**
 * Adresse de connexion : `hôte`, `hôte:port`, `[ipv6]:port` ou IPv6 nue. Le port est gardé.
 */
export function maskAddress(addr: string): string {
  const v = addr.trim();
  if (!v) return addr;
  const bracket = /^\[[^\]]*\](?::(\d+))?$/.exec(v);
  if (bracket) return withPort(`[${IPV6_MASK}]`, bracket[1]);
  const hostPort = /^([^:\s]+)(?::(\d+))?$/.exec(v);
  if (hostPort) return withPort(maskHost(hostPort[1]), hostPort[2]);
  // IPv6 nue (plusieurs deux-points) ou forme inattendue : masquée d'un bloc.
  return v.includes(':') ? IPV6_MASK : DOMAIN_MASK;
}

function withPort(host: string, port: string | undefined): string {
  return port ? `${host}:${port}` : host;
}
