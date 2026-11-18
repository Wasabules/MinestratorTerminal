#!/usr/bin/env node
/**
 * Contrôle des traductions fr/en, lancé par la CI (job Typecheck) : mêmes clés des deux côtés et
 * mêmes paramètres `{x}` dans chaque traduction.
 *
 * - `mobile` : dictionnaires plats de `mobile/src/lib/i18n.ts`. Le dictionnaire anglais commence
 *   par `...fr` : une clé oubliée en anglais s'afficherait en français sans que rien ne le signale,
 *   et rien ne vérifie le type des clés. Ce script est la seule garde.
 * - `desktop` : arbres de `desktop/src/lib/i18n/{fr,en}.ts`. Le typecheck impose déjà les mêmes
 *   clés ; ce script vérifie en plus les paramètres. Il s'appuie sur le paquet `typescript` installé
 *   dans `desktop/node_modules` (à lancer après `npm ci`).
 *
 * Usage : `node scripts/check-i18n.mjs [mobile|desktop]...` (les deux sans argument).
 */
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** Paramètres `{nom}` d'une traduction, triés (l'ordre peut légitimement changer). */
function params(text) {
  return [...String(text).matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');
}

/** Compare deux tables plates `clé → texte` et renvoie la liste des problèmes. */
export function compare(fr, en, { ignoreExtraFr = false } = {}) {
  const problems = [];
  for (const key of Object.keys(fr)) {
    if (!(key in en)) {
      if (!ignoreExtraFr) problems.push(`clé absente en anglais : ${key}`);
      continue;
    }
    if (params(fr[key]) !== params(en[key])) {
      problems.push(`paramètres différents pour ${key} : fr {${params(fr[key])}} / en {${params(en[key])}}`);
    }
  }
  for (const key of Object.keys(en)) {
    if (!(key in fr)) problems.push(`clé absente en français : ${key}`);
  }
  return problems;
}

/**
 * Extrait les paires `"clé": "texte"` du littéral d'objet qui suit `const <nom>: Dict = {`.
 * Les valeurs peuvent être sur la ligne suivante ; les chaînes JSON (échappements compris) sont
 * décodées par `JSON.parse`.
 */
export function flatDict(source, name) {
  const start = source.indexOf(`const ${name}: Dict = {`);
  if (start < 0) throw new Error(`dictionnaire « ${name} » introuvable`);
  const end = source.indexOf('\n};', start);
  const body = source.slice(start, end);
  const out = {};
  const pair = /"((?:[^"\\]|\\.)+)"\s*:\s*("(?:[^"\\]|\\.)*")/g;
  for (const m of body.matchAll(pair)) {
    const key = JSON.parse(`"${m[1]}"`);
    if (key in out) throw new Error(`clé en double dans « ${name} » : ${key}`);
    out[key] = JSON.parse(m[2]);
  }
  return out;
}

function checkMobile() {
  const source = readFileSync(join(root, 'mobile/src/lib/i18n.ts'), 'utf8');
  return compare(flatDict(source, 'fr'), flatDict(source, 'en'));
}

/** Aplatit un arbre de traductions en `a.b.c → texte`. */
function flatten(tree, prefix = '', out = {}) {
  for (const [k, v] of Object.entries(tree)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v && typeof v === 'object') flatten(v, key, out);
    else out[key] = v;
  }
  return out;
}

async function loadTs(file, exportName) {
  const require = createRequire(join(root, 'desktop/package.json'));
  const ts = require('typescript');
  const { outputText } = ts.transpileModule(readFileSync(file, 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  });
  const mod = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
  return mod[exportName];
}

async function checkDesktop() {
  const dir = join(root, 'desktop/src/lib/i18n');
  const fr = flatten(await loadTs(join(dir, 'fr.ts'), 'fr'));
  const en = flatten(await loadTs(join(dir, 'en.ts'), 'en'));
  return compare(fr, en);
}

async function main() {
  const targets = process.argv.slice(2);
  const which = targets.length ? targets : ['mobile', 'desktop'];
  let failed = false;
  for (const target of which) {
    let problems;
    if (target === 'mobile') problems = checkMobile();
    else if (target === 'desktop') problems = await checkDesktop();
    else throw new Error(`cible inconnue : ${target}`);
    if (problems.length) {
      failed = true;
      console.error(`✗ i18n ${target} : ${problems.length} problème(s)`);
      for (const p of problems) console.error(`  - ${p}`);
    } else {
      console.log(`✓ i18n ${target} : fr et en identiques (clés et paramètres)`);
    }
  }
  if (failed) process.exit(1);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  await main();
}
