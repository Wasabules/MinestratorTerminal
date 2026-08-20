/**
 * Gestionnaire de transferts mobile : suit les envois/téléchargements en cours (rune `$state`),
 * et porte la mécanique complète — sélecteur système, lecture/écriture de fichier, appel IPC.
 * Global : un transfert survit au changement d'onglet ou de serveur.
 *
 * **L'envoi est piloté par le Rust** : Kotlin cède le descripteur du fichier choisi, le core lit
 * et écrit en SFTP, et la progression revient par l'event `sftp://progress`. Le webview ne voit
 * passer aucun octet. Le chemin précédent — lire le fichier depuis le JS et encoder chaque bloc
 * en base64 — bornait bien la mémoire instantanée, mais allouait de grosses chaînes plus vite que
 * le ramasse-miettes ne les libérait : l'envoi d'une vidéo a fait tuer une dizaine d'applications
 * par le `lowmemorykiller` d'Android. Bénéfice supplémentaire : un envoi survit désormais à un
 * rechargement de la page, puisqu'il ne vit plus dedans.
 *
 * Le téléchargement, lui, reste piloté côté JS (blocs base64) : il écrit dans un fichier choisi
 * via le sélecteur système, qui n'expose pas de descripteur exploitable de la même façon.
 */

import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import { open as openFile } from "@tauri-apps/plugin-fs";
import { api, humanizeError } from "../ipc";
import { sftpEvents } from "../events";

export interface Transfer {
  id: string;
  name: string;
  direction: "up" | "down";
  status: "active" | "done" | "error";
  /** Octets déjà transférés. */
  done: number;
  /** Taille totale, connue dès le départ (fichier local lu, ou `stat` distant). */
  total: number;
  /** Alias de `done` une fois terminé, pour l'affichage. */
  size: number;
  /** Débit courant en octets/s, lissé. `0` tant qu'aucune mesure n'est exploitable. */
  speed: number;
  /** Dernier point de mesure (horodatage + octets), pour calculer le débit entre deux avancées. */
  lastAt: number;
  lastDone: number;
  error: string | null;
  /** Horodatage de fin, pour trier et purger. */
  endedAt: number | null;
}

/** Au-delà, les plus anciens sont oubliés : c'est un journal récent, pas un historique complet. */
const MAX = 40;

/**
 * Le journal est persisté : un rechargement du webview (renderer tué par Android, ou client de
 * développement qui se reconnecte) repartirait sinon d'une liste vide, effaçant sous les yeux de
 * l'utilisateur le résultat de ce qu'il vient de faire.
 */
const STORAGE_KEY = "mnstr-transfers";

function restore(): Transfer[] {
    try {
        const raw = localStorage.getItem(STORAGE_KEY);
        if (!raw) return [];
        const stored = JSON.parse(raw) as Transfer[];
        // Dédoublonnage défensif : un journal écrit par une version antérieure peut contenir
        // deux entrées de même identifiant, et Svelte refuse alors de rendre la liste entière —
        // le centre de transferts devenait inouvrable. On garde la plus récente (les items sont
        // ordonnés du plus récent au plus ancien).
        const seen = new Set<string>();
        const items = stored.filter((t) => {
            if (!t?.id || seen.has(t.id)) return false;
            seen.add(t.id);
            return true;
        });
        return items.map((t) => {
            if (t.status !== "active") return t;
            // Un ENVOI vit côté Rust : il continue malgré le rechargement, et ses events le
            // remettront à jour. Un TÉLÉCHARGEMENT, lui, est piloté par la page qui vient d'être
            // rechargée : il est mort, on le dit plutôt que de le laisser tourner sans fin.
            // Débit remis à zéro : la mesure d'avant le rechargement ne veut plus rien dire.
            t.speed = 0;
            t.lastAt = Date.now();
            t.lastDone = t.done;
            if (t.direction === "up") return t;
            // Téléchargement mort avec la page : sa notification est `ongoing`, donc impossible à
            // écarter d'un balayage. Sans ce nettoyage, elle resterait épinglée indéfiniment.
            void api.clearTransfer(5000 + (Number(t.id.slice(1)) || 0)).catch(() => {});
            return { ...t, status: "error" as const, error: "Transfert interrompu.", endedAt: Date.now() };
        });
    } catch {
        return [];
    }
}

const store = $state<{ items: Transfer[] }>({ items: restore() });

/**
 * Un seul abonnement pour toute l'app : le core émet la progression des envois natifs, on la
 * reporte sur l'entrée correspondante du journal.
 */
let listening = false;
function listenProgress(): void {
  if (listening) return;
  listening = true;
  void sftpEvents
    .progress((p) => {
      const t = store.items.find((x) => x.id === p.id);
      if (!t) return;
      // L'event TERMINAL du core porte `done: 0, total: 0` (cf. `emit_transfer_end`) : écraser
      // l'état avec ces zéros ferait afficher « — » comme taille finale de chaque envoi réussi.
      // On ne recopie donc la progression que tant que le transfert est actif.
      if (p.status === "active") {
        t.total = p.total;
        bump(t, p.done);
      }
      if (p.status === "done") {
        finish(p.id, t.total || t.done);
      } else if (p.status === "error") {
        fail(p.id, p.error ?? "Échec du transfert.");
      } else {
        notifyProgress(t);
      }
    })
    .catch(() => {
      listening = false; // abonnement impossible : on retentera au prochain transfert
    });
}

// Abonnement immédiat : un envoi lancé avant un rechargement continue côté Rust, et ses events
// doivent retrouver l'entrée restaurée du journal.
listenProgress();

function persist(): void {
    try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(store.items));
    } catch {
        /* stockage indisponible : le journal reste en mémoire */
    }
}

/**
 * Le compteur reprend au-delà du plus grand identifiant restauré : remis à zéro, il réattribuait
 * `t1` à un nouveau transfert alors que le journal en contenait déjà un — clé dupliquée, et tout
 * le composant de transferts cessait de s'afficher.
 */
let seq = store.items.reduce((max, t) => Math.max(max, Number(t.id.slice(1)) || 0), 0);

function nextId(): string {
  seq += 1;
  return `t${seq}`;
}

/** Liste complète, du plus récent au plus ancien (réactif). */
export function transferItems(): Transfer[] {
  return store.items;
}

/** Nombre de transferts en cours — sert au badge de l'icône. */
export function activeCount(): number {
  return store.items.filter((t) => t.status === "active").length;
}

/**
 * Demande l'arrêt d'un transfert.
 *
 * L'entrée passe immédiatement en échec côté interface : un envoi peut rester bloqué dans une
 * lecture qui ne rend pas la main (fichier qu'un fournisseur Android rapatrie depuis le cloud),
 * et laisser la ligne « en cours… » donnerait l'impression que l'annulation n'a rien fait.
 * Le Rust, lui, s'arrêtera dès qu'il reprendra la main.
 */
export function cancel(id: string): void {
  const t = store.items.find((x) => x.id === id);
  if (!t || t.status !== "active") return;
  // Le téléchargement est piloté ici : son drapeau interrompt la boucle au prochain bloc.
  cancelled.add(id);
  void api.cancelTransfer(id).catch(() => {});
  t.status = "error";
  t.error = "Transfert annulé.";
  t.endedAt = Date.now();
  persist();
  void api.clearTransfer(notifId(id)).catch(() => {});
}

/** Identifiants annulés, consultés par la boucle de téléchargement (pilotée côté JS). */
const cancelled = new Set<string>();

/** Retire les transferts achevés, en gardant ceux qui tournent encore. */
export function clearFinished(): void {
  store.items = store.items.filter((t) => t.status === "active");
  persist();
}

/**
 * Taille d'un bloc. 512 Kio est un compromis : plus petit multiplie les allers-retours IPC et
 * l'ouverture SFTP par bloc ; plus gros rend la barre saccadée et gonfle le pic mémoire (chaque
 * bloc est encodé en base64, +33 %).
 */
const CHUNK = 512 * 1024;

function add(name: string, direction: "up" | "down", total: number): string {
  const id = nextId();
  store.items.unshift({
    id, name, direction, status: "active", done: 0, total, size: 0, error: null, endedAt: null,
    speed: 0, lastAt: Date.now(), lastDone: 0,
  });
  // Purge par la fin, en ne retirant QUE des transferts achevés : tronquer aveuglément pouvait
  // évincer un transfert encore en cours, dont les events suivants ne trouvaient plus d'entrée
  // et dont la notification restait orpheline.
  while (store.items.length > MAX) {
    const idx = [...store.items].reverse().findIndex((t) => t.status !== "active");
    if (idx === -1) break; // que des transferts actifs : on garde tout plutôt que d'en perdre un
    store.items.splice(store.items.length - 1 - idx, 1);
  }
  persist();
  return id;
}

/** Identifiant numérique pour la notification système (une par transfert). */
function notifId(id: string): number {
  return 5000 + Number(id.slice(1));
}

function fmt(n: number): string {
  if (n <= 0) return "0 Ko";
  const mb = n / 1024 ** 2;
  return mb >= 1 ? `${mb.toFixed(1)} Mo` : `${Math.max(1, Math.round(n / 1024))} Ko`;
}

/**
 * Enregistre une avancée et en déduit le débit.
 *
 * Moyenne glissante exponentielle plutôt que débit instantané : les blocs arrivent par à-coups
 * (latence réseau, ordonnancement Android), et une valeur brute sauterait de 0 à 20 Mo/s d'un
 * rafraîchissement à l'autre, illisible. Les intervalles trop courts sont ignorés — diviser par
 * quelques millisecondes produit des valeurs absurdes.
 */
function bump(t: Transfer, done: number): void {
  const now = Date.now();
  const dt = (now - t.lastAt) / 1000;
  const db = done - t.lastDone;
  if (dt >= 0.25 && db > 0) {
    const instant = db / dt;
    // Première mesure : on prend la valeur telle quelle, sinon la moyenne démarrerait à zéro.
    t.speed = t.speed > 0 ? t.speed * 0.7 + instant * 0.3 : instant;
    t.lastAt = now;
    t.lastDone = done;
  }
  t.done = done;
}

/** Débit lisible : l'unité s'adapte à l'ordre de grandeur. */
export function fmtSpeed(bytesPerSec: number): string {
  if (!bytesPerSec || bytesPerSec <= 0) return "";
  const mb = bytesPerSec / 1024 ** 2;
  if (mb >= 1) return `${mb.toFixed(1)} Mo/s`;
  return `${Math.max(1, Math.round(bytesPerSec / 1024))} Ko/s`;
}

/** Reflète l'avancement d'un transfert dans la notification Android. */
function notifyProgress(t: Transfer): void {
  const label = t.direction === "up" ? "Envoi" : "Téléchargement";
  const speed = fmtSpeed(t.speed);
  // Taille inconnue (fournisseur Android qui ne déclare pas `statSize`) : on annonce ce qui est
  // parti, sans total. Afficher « / 0 » n'apprendrait rien et donnerait l'impression d'un bug.
  const done = t.total > 0 ? `${fmt(t.done)} / ${fmt(t.total)}` : `${fmt(t.done)} envoyés`;
  void api
    .notifyTransfer(
      notifId(t.id),
      `${label} — ${t.name}`,
      speed ? `${done} · ${speed}` : done,
      t.done,
      t.total,
      false,
    )
    .catch(() => {});
}

/** Avance la progression d'un transfert piloté côté JS (téléchargement). */
function progress(id: string, done: number): void {
  const t = store.items.find((x) => x.id === id);
  if (!t) return;
  bump(t, done);
  notifyProgress(t);
}

function finish(id: string, size: number): void {
  const t = store.items.find((x) => x.id === id);
  if (!t) return;
  t.status = "done";
  t.size = size;
  t.done = size;
  t.endedAt = Date.now();
  persist();
  const label = t.direction === "up" ? "Envoi terminé" : "Téléchargement terminé";
  void api
    .notifyTransfer(notifId(id), `${label} — ${t.name}`, fmt(size), size, size, true)
    .catch(() => {});
}

function fail(id: string, err: unknown): void {
  const t = store.items.find((x) => x.id === id);
  if (!t) return;
  t.status = "error";
  t.error = humanizeError(err);
  t.endedAt = Date.now();
  persist();
  // La notification disparaît : l'échec est rapporté dans l'app, pas dans le tiroir système,
  // où un message d'erreur figé n'aiderait pas.
  void api.clearTransfer(notifId(id)).catch(() => {});
}

/** Base64 → octets. */
function b64ToBytes(b64: string): Uint8Array {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Octets → base64, par tranches : `fromCharCode(...bytes)` fait déborder la pile sur un gros fichier. */
function bytesToB64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(bin);
}

/**
 * Choisit un fichier du téléphone et l'envoie dans `remoteDir`.
 * Renvoie `true` si un transfert a bien eu lieu (`false` = sélecteur annulé), pour que
 * l'appelant sache s'il doit rafraîchir sa liste.
 */
export async function uploadTo(serverId: number, remoteDir: string, title: string): Promise<boolean> {
  const picked = await openDialog({ multiple: false, title });
  if (!picked || Array.isArray(picked)) return false;

  // Le nom doit venir du système : sur Android, l'URI ne le contient pas
  // (`content://…/document/image%3A3604` → dernier segment = identifiant interne).
  let name = "fichier";
  try {
    name = (await api.fileDisplayName(picked)) || name;
  } catch {
    name = decodeURIComponent(picked.split(/[\/]/).pop() || name).replace(/[:*?"<>|]/g, "_");
  }

  listenProgress();
  const id = add(name, "up", 0);
  try {
    // Rend la main quand le transfert est fini ; l'avancement arrive par `sftp://progress`.
    // Le résultat de la commande ne dit PAS si le transfert a réussi : le core signale l'issue
    // par un event terminal, qui emprunte un autre chemin et peut arriver après. On lui laisse un
    // instant, sinon un envoi échoué serait annoncé comme réussi (et notifié comme tel).
    await api.sftpUploadUri(serverId, picked, name, remoteDir, id);
    await new Promise((r) => setTimeout(r, 150));
    const t = store.items.find((x) => x.id === id);
    if (t?.status === "error") return false;
    // Event terminal perdu : on clôt nous-mêmes plutôt que de laisser l'entrée active à jamais.
    if (t?.status === "active") finish(id, t.done);
    return true;
  } catch (err) {
    fail(id, err);
    return false;
  }
}

/**
 * Télécharge un fichier distant vers l'emplacement choisi par l'utilisateur.
 * Renvoie `false` si le sélecteur a été annulé.
 */
export async function downloadFrom(
  serverId: number,
  remotePath: string,
  name: string,
  title: string,
): Promise<boolean> {
  const dest = await saveDialog({ defaultPath: name, title });
  if (!dest) return false;

  const id = add(name, "down", 0);
  let handle;
  try {
    const total = await api.sftpFileSize(serverId, remotePath);
    const t = store.items.find((x) => x.id === id);
    if (t) t.total = total;

    // La taille doit être connue AVANT d'ouvrir la destination : `truncate` écrase le fichier
    // existant, et un total nul produirait un fichier de 0 octet présenté comme un succès.
    if (total <= 0) throw new Error("Taille du fichier distant inconnue : téléchargement annulé.");

    handle = await openFile(dest, { write: true, create: true, truncate: true });
    let got = 0;
    while (got < total) {
      if (cancelled.has(id)) {
        cancelled.delete(id);
        return false; // `cancel()` a déjà consigné l'échec et retiré la notification
      }
      const b64 = await api.sftpDownloadChunk(serverId, remotePath, got, CHUNK);
      const chunk = b64ToBytes(b64);
      // Bloc vide avant la fin : la session est en train de mourir. C'est un échec, pas une fin
      // de fichier — le déclarer terminé livrerait un fichier tronqué en le disant complet.
      if (chunk.length === 0) {
        throw new Error(`Téléchargement interrompu à ${fmt(got)} sur ${fmt(total)}.`);
      }
      // Écrit au fil de l'eau : accumuler les blocs doublerait la mémoire du fichier entier.
      await handle.write(chunk);
      got += chunk.length;
      progress(id, got);
    }
    if (got !== total) {
      throw new Error(`Téléchargement incomplet : ${fmt(got)} sur ${fmt(total)}.`);
    }
    finish(id, got);
    return true;
  } catch (err) {
    fail(id, err);
    return false;
  } finally {
    await handle?.close().catch(() => {});
  }
}
