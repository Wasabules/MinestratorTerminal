/**
 * Aides de l'interface pour les opérations de fichiers faites **sur le serveur** (compression,
 * extraction) : quels fichiers proposer, quel nom suggérer. La validation qui compte vit dans le
 * core (`server_files.rs`) ; ceci ne sert qu'à l'affichage.
 */

/** Formats que le serveur sait extraire (liste du panel MineStrator, reprise par le core). */
const SERVER_EXTRACT_EXTS = [
  ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst", ".tgz", ".tbz2", ".txz", ".zip",
  ".mcaddon", ".mcpack", ".mcworld", ".tar", ".gz", ".bz2", ".xz", ".zst", ".rar", ".7z",
];

/** Archive extractible par le serveur ? (Un `.jar` est un plugin ou un mod : pas proposé.) */
export function isServerExtractable(name: string): boolean {
  const n = name.toLowerCase();
  return SERVER_EXTRACT_EXTS.some((ext) => n.endsWith(ext));
}

/** Nom proposé pour une archive créée sur le serveur, comme le panel : `world_2026-11-18_15h04.zip`. */
export function defaultArchiveName(name: string, isDir: boolean, now = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())}_${p(now.getHours())}h${p(now.getMinutes())}`;
  const dot = name.lastIndexOf(".");
  const stem = (isDir || dot <= 0 ? name : name.slice(0, dot)).replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").trim();
  return `${stem || "archive"}_${stamp}.zip`;
}
