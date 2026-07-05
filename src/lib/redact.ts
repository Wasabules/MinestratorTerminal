/**
 * Anonymisation côté affichage (console) : masque mots de passe de commandes d'auth,
 * adresses IPv4 et e-mails. Miroir des règles de `crates/minestrator-core/src/redact.rs`
 * (l'anonymisation envoyée aux agents IA, elle, est faite en Rust — source unique côté sécurité).
 */

const AUTH_CMDS = new Set([
  'login',
  'l',
  'register',
  'reg',
  'changepassword',
  'changepass',
  'cp',
  'premiumlogin',
  'premiumregister',
  'authme',
  'auth',
  'unregister',
  'password',
]);
const AUTH_SUBCMDS = new Set(['register', 'login', 'changepassword', 'setpassword', 'force']);

/** Anonymise une ligne (les lignes console arrivent une par une). */
export function redactLine(line: string): string {
  return maskEmails(maskIpv4(maskAuthPasswords(line)));
}

function maskAuthPasswords(line: string): string {
  const tokens = line.split(/\s+/);
  const i = tokens.findIndex(isAuthCmd);
  if (i === -1) return line;
  let j = i + 1;
  if (j < tokens.length && AUTH_SUBCMDS.has(tokens[j].toLowerCase())) j += 1;
  let masked = 0;
  while (j < tokens.length && masked < 2) {
    if (tokens[j].startsWith('/')) break;
    tokens[j] = '***';
    masked += 1;
    j += 1;
  }
  return masked === 0 ? line : tokens.join(' ');
}

function isAuthCmd(token: string): boolean {
  return token.startsWith('/') && AUTH_CMDS.has(token.slice(1).toLowerCase());
}

/** IPv4 (octets 0-255) → `[IP]`. Le `\b` évite les versions x.y.z (3 groupes). */
function maskIpv4(s: string): string {
  return s.replace(
    /\b((25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(25[0-5]|2[0-4]\d|1?\d?\d)\b/g,
    '[IP]'
  );
}

function maskEmails(s: string): string {
  return s.replace(/[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/g, '[EMAIL]');
}
