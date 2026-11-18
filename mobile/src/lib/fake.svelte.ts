/**
 * Mode « données de démo » : remplace à l'affichage tout ce qui identifie le compte, les
 * serveurs et les joueurs, pour produire des captures et des GIF publiables.
 *
 * Falsification plutôt que floutage : une capture floutée ne montre plus *quoi* est masqué, et
 * rend mal en vidéo. Ici l'app reste entièrement lisible, avec des valeurs plausibles.
 *
 * Deux propriétés tenues :
 * - **Déterminisme** — une même valeur d'origine donne toujours le même substitut, y compris
 *   d'une capture à l'autre : un serveur garde son nom de démo entre l'accueil, la console et le
 *   journal, sinon les captures d'une même séquence se contrediraient.
 * - **Valeurs non attribuables** — adresses tirées des plages réservées à la documentation
 *   (RFC 5737 : `203.0.113.0/24`) et domaines en `example.net` (RFC 2606). Elles ne peuvent
 *   désigner aucune machine réelle, contrairement à une IP inventée au hasard.
 *
 * Copie volontaire du module desktop : les deux apps ont des bases séparées, et un substitut
 * doit être identique de part et d'autre pour qu'une capture desktop et une capture mobile de
 * la même scène restent cohérentes. Toute évolution ici doit être reportée là-bas.
 *
 * La substitution est purement visuelle : elle s'applique aux réponses IPC, jamais aux valeurs
 * envoyées au serveur. Une commande console reste exécutée telle que tapée.
 */

const KEY = 'mnstr-fake-data';

const store = $state<{ on: boolean }>({ on: false });

/** Charge la préférence au démarrage. */
export function initFake(): void {
  try {
    store.on = localStorage.getItem(KEY) === '1';
  } catch {
    /* localStorage indisponible */
  }
}

/** Le mode démo est-il actif ? (réactif) */
export function isFake(): boolean {
  return store.on;
}

export function setFake(on: boolean): void {
  store.on = on;
  try {
    localStorage.setItem(KEY, on ? '1' : '0');
  } catch {
    /* localStorage indisponible */
  }
}

/** Hash stable (djb2) : c'est lui qui garantit qu'une valeur donne toujours le même substitut. */
function hash(s: string): number {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = ((h << 5) + h + s.charCodeAt(i)) >>> 0;
  return h;
}

function pick<T>(pool: readonly T[], seed: string): T {
  return pool[hash(seed) % pool.length];
}

const PLAYERS = [
  'Steve', 'Alex', 'Zuri', 'Efe', 'Makena', 'Noor', 'Sunny', 'Ari',
  'Kai', 'Nova', 'Rin', 'Milo', 'Juno', 'Sasha', 'Iris', 'Vik',
] as const;

const SERVERS = [
  'Survie Publique', 'Créatif', 'SkyBlock', 'Aventure', 'Mini-Jeux',
  'Bac à sable', 'PvP Arena', 'Serveur Test',
] as const;

const BOXES = ['MyBox Démo', 'MyBox Principale', 'MyBox Secondaire'] as const;

/** Pseudo de joueur ou d'utilisateur. */
export function fakePlayer(v: string): string {
  return pick(PLAYERS, v);
}

/** Nom de serveur — la version numérotée garde l'idée d'un parc de plusieurs serveurs. */
export function fakeServer(v: string): string {
  return pick(SERVERS, v);
}

export function fakeBox(v: string): string {
  return pick(BOXES, v);
}

/**
 * UUID réels des UUID de joueurs substitués : la fiche d'un joueur est demandée avec l'UUID
 * affiché, que `realUuid` ramène à la vraie valeur avant l'appel (rien de faux n'est envoyé).
 */
const realUuids = new Map<string, string>();

/** UUID de joueur : même forme (8-4-4-4-12), contenu dérivé de la valeur réelle. */
export function fakeUuid(v: string): string {
  const hex = [0, 1, 2, 3].map((i) => hash(`${i}:${v.toLowerCase()}`).toString(16).padStart(8, "0")).join("");
  const out = `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-${hex.slice(16, 20)}-${hex.slice(20, 32)}`;
  realUuids.set(out, v);
  return out;
}

/** UUID réel d'un UUID substitué par `fakeUuid` (la valeur reçue si elle n'en est pas un). */
export function realUuid(v: string): string {
  return realUuids.get(v) ?? v;
}

/** Adresse IP dans la plage de documentation RFC 5737 : jamais routable, jamais réelle. */
export function fakeIp(v: string): string {
  return `203.0.113.${(hash(v) % 254) + 1}`;
}

/** Adresse de connexion : domaine `example.net` (RFC 2606), port conservé s'il y en avait un. */
export function fakeAddress(v: string): string {
  const port = v.match(/:(\d+)$/)?.[1];
  const host = `${pick(['play', 'mc', 'jeu', 'serveur'], v)}.example.net`;
  return port ? `${host}:${port}` : host;
}

export function fakeMail(v: string): string {
  return `${pick(PLAYERS, v).toLowerCase()}@example.net`;
}

/** Référence support : même forme (5 caractères), contenu inventé. */
export function fakeRef(v: string): string {
  const alphabet = 'ABCDEFGHJKMNPQRSTUVWXYZ23456789';
  let h = hash(v);
  let out = '';
  for (let i = 0; i < 5; i++) {
    out += alphabet[h % alphabet.length];
    h = Math.floor(h / alphabet.length) + 7;
  }
  return out;
}

/**
 * Remplace dans un texte libre ce qui peut identifier quelqu'un : adresses IP, e-mails, et les
 * pseudos connus passés en `players`. Sert à la console et aux messages de l'hébergeur, où les
 * informations sensibles sont noyées dans la prose.
 */
export function fakeText(text: string, players: string[] = []): string {
  let out = text
    .replace(/\b(\d{1,3}\.){3}\d{1,3}\b/g, (ip) => fakeIp(ip))
    .replace(/\b[\w.+-]+@[\w-]+\.[\w.]+\b/g, (mail) => fakeMail(mail));
  for (const p of players) {
    if (p.length < 3) continue; // trop court : remplacerait des fragments de mots
    out = out.replaceAll(p, fakePlayer(p));
  }
  return out;
}

/**
 * Réglage `server.properties` : substitue ceux qui identifient le serveur (adresse, MOTD, graine,
 * pack de ressources) et masque les secrets. Les autres (difficulté, slots…) restent lisibles.
 */
export function fakeProperty(key: string, value: string): string {
  if (!value) return value;
  if (/password|secret|token/i.test(key)) return "••••••••";
  switch (key) {
    case "server-ip":
      return fakeIp(value);
    case "motd":
      return fakeText(value);
    case "level-seed":
      return "1234567890";
    case "resource-pack":
      return "https://example.net/pack.zip";
    default:
      return value;
  }
}
