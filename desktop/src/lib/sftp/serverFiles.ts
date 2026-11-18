/**
 * Aides de l'interface pour les opérations de fichiers faites **sur le serveur** (compression,
 * extraction) : quels fichiers proposer, quel nom suggérer. La validation qui compte vit dans le
 * core (`server_files.rs`) ; ceci ne sert qu'à l'affichage.
 */

/** Formats que le serveur sait extraire (liste du panel MineStrator, reprise par le core). */
const SERVER_EXTRACT_EXTS = [
  '.tar.gz', '.tar.bz2', '.tar.xz', '.tar.zst', '.tgz', '.tbz2', '.txz', '.zip', '.jar',
  '.mcaddon', '.mcpack', '.mcworld', '.tar', '.gz', '.bz2', '.xz', '.zst', '.rar', '.7z',
];

/** Archive extractible par le serveur ? (Un `.jar` l'est, mais on ne le propose pas : c'est un plugin ou un mod.) */
export function isServerExtractable(name: string): boolean {
  const n = name.toLowerCase();
  if (n.endsWith('.jar')) return false;
  return SERVER_EXTRACT_EXTS.some((ext) => n.endsWith(ext));
}

/** Horodatage du nom par défaut, au format du panel : `2026-11-18_15h04`. */
function stamp(d: Date): string {
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}h${p(d.getMinutes())}`;
}

function base(path: string): string {
  return path.replace(/\/+$/, '').split('/').pop() ?? path;
}

/**
 * Nom proposé pour une archive créée sur le serveur, comme le panel : le dossier (ou le fichier
 * sans extension) s'il est seul, sinon le dossier qui contient la sélection.
 */
export function defaultArchiveName(paths: string[], isDir: (p: string) => boolean, now = new Date()): string {
  let stem: string;
  if (paths.length === 1) {
    const name = base(paths[0]);
    const dot = name.lastIndexOf('.');
    stem = isDir(paths[0]) || dot <= 0 ? name : name.slice(0, dot);
  } else {
    const parent = paths[0]?.replace(/\/[^/]*$/, '') ?? '';
    stem = parent ? base(parent) : 'archive';
  }
  stem = stem.replace(/[<>:"/\\|?*\u0000-\u001f]/g, '_').trim() || 'archive';
  return `${stem}_${stamp(now)}.zip`;
}
