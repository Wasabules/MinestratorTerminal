#!/usr/bin/env node
/**
 * Construit le binaire du service d'arrière-plan (`crates/minestrator-daemon`) et le dépose là
 * où Tauri attend un `externalBin` : `src-tauri/binaries/minestrator-daemon-<triple>[.exe]`.
 *
 * Lancé par le `beforeBuildCommand` de `src-tauri/tauri.service.conf.json` (la CLI Tauri expose
 * alors `TAURI_ENV_TARGET_TRIPLE` et `TAURI_ENV_DEBUG`), ou à la main avant un
 * `tauri dev --config src-tauri/tauri.service.conf.json`. Voir docs/SERVICE.md.
 */
import { execFileSync } from 'node:child_process';
import { copyFileSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const desktop = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const root = resolve(desktop, '..');
const outDir = join(desktop, 'src-tauri', 'binaries');

function hostTriple() {
  const out = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
  const m = out.match(/^host: (\S+)$/m);
  if (!m) throw new Error('triplet hôte introuvable dans `rustc -vV`');
  return m[1];
}

const triple = process.env.TAURI_ENV_TARGET_TRIPLE || hostTriple();
const debug = process.env.TAURI_ENV_DEBUG === 'true';
const exe = triple.includes('windows') ? '.exe' : '';

/** Compile le daemon pour une cible et renvoie le chemin du binaire produit. */
function build(target) {
  const args = ['build', '-p', 'minestrator-daemon', '--target', target];
  if (!debug) args.push('--release');
  execFileSync('cargo', args, { cwd: root, stdio: 'inherit' });
  return join(root, 'target', target, debug ? 'debug' : 'release', `minestrator-daemon${exe}`);
}

mkdirSync(outDir, { recursive: true });
const dest = join(outDir, `minestrator-daemon-${triple}${exe}`);
if (triple === 'universal-apple-darwin') {
  // Binaire universel macOS : les deux architectures, assemblées par `lipo`.
  const parts = ['aarch64-apple-darwin', 'x86_64-apple-darwin'].map(build);
  execFileSync('lipo', ['-create', '-output', dest, ...parts], { stdio: 'inherit' });
} else {
  copyFileSync(build(triple), dest);
}
console.log(`Binaire du service prêt : ${dest}`);
