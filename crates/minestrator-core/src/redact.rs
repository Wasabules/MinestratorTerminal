//! Anonymisation des données sensibles avant **affichage console** et/ou **envoi aux agents IA**.
//!
//! Masque : mots de passe des commandes d'authentification (`/login`, `/register`, `/authme`…),
//! adresses IPv4, et e-mails. Implémentation **sans regex** (scan manuel) pour éviter une
//! dépendance ; heuristiques volontairement prudentes (mieux vaut masquer un peu trop que fuiter).

use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Réglages de confidentialité (persistés dans `privacy.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Masque les données sensibles AVANT de les envoyer aux agents IA (Copilote, Assistant,
    /// clients MCP externes). Activé par défaut : rien de sensible ne part vers un LLM sans le vouloir.
    #[serde(default = "default_true")]
    pub redact_ai: bool,
    /// Masque aussi les données sensibles dans l'AFFICHAGE de la console (temps réel + préchargement).
    #[serde(default)]
    pub redact_console: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            redact_ai: true,
            redact_console: false,
        }
    }
}

/// Commandes Minecraft dont le/les argument(s) suivant(s) sont des mots de passe.
const AUTH_CMDS: &[&str] = &[
    "login",
    "l",
    "register",
    "reg",
    "changepassword",
    "changepass",
    "cp",
    "premiumlogin",
    "premiumregister",
    "authme",
    "auth",
    "unregister",
    "password",
];

/// Sous-commandes à préserver (ex. `/authme register <pw>`) : on masque APRÈS elles.
const AUTH_SUBCMDS: &[&str] = &["register", "login", "changepassword", "setpassword", "force"];

/// Point d'entrée : anonymise un texte multi-lignes.
pub fn redact(text: &str) -> String {
    // Écrit directement dans un buffer préalloué (pas de `Vec<String>` intermédiaire) ; chaque
    // ligne n'alloue que si un masque s'applique réellement (chaîne de `Cow`, voir `redact_line`).
    let mut out = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&redact_line(line));
    }
    out
}

fn redact_line(line: &str) -> Cow<'_, str> {
    let s = mask_auth_passwords(Cow::Borrowed(line));
    let s = mask_secrets(s);
    let s = mask_ipv4(s);
    mask_emails(s)
}

/// Clés dont la valeur (après `=` ou `:`) est un secret : couvre server.properties
/// (`rcon.password=…`), YAML/props de plugins (`token: …`, `db-password: …`), etc.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "apikey",
    "api-key",
    "api_key",
    "authorization",
    "access-token",
    "private-key",
    "rcon.password",
    "db-password",
    "client-secret",
    "webhook",
];

/// Masque les secrets `clé=valeur` / `clé: valeur` (clé sensible, insensible à la casse) et les
/// identifiants d'URL de connexion `scheme://user:pass@host`.
fn mask_secrets(line: Cow<str>) -> Cow<str> {
    let line = mask_connection_string(line);
    let is_key_char = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    for (pos, sep) in line.char_indices().filter(|(_, c)| *c == '=' || *c == ':') {
        // Clé = suite de caractères de clé juste avant le séparateur.
        let key_start = line[..pos]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_key_char(*c))
            .last()
            .map(|(i, _)| i)
            .unwrap_or(pos);
        let key = line[key_start..pos].to_ascii_lowercase();
        if SECRET_KEYS.contains(&key.as_str())
            && !line[pos + sep.len_utf8()..].trim().is_empty()
        {
            return Cow::Owned(format!("{} [SECRET]", &line[..=pos]));
        }
    }
    line
}

/// `scheme://user:pass@host` → `scheme://[CREDS]@host` (masque les identifiants intégrés à l'URL).
fn mask_connection_string(line: Cow<str>) -> Cow<str> {
    let Some(scheme_end) = line.find("://") else {
        return line;
    };
    let rest = scheme_end + 3;
    let Some(rel_at) = line[rest..].find('@') else {
        return line;
    };
    let at = rest + rel_at;
    // `@` dans le chemin, ou pas de `user:pass` → rien à masquer. (Slices inlinés pour ne pas
    // garder d'emprunt de `line` au-dessus du `return line`.)
    if line[rest..at].contains('/') || !line[rest..at].contains(':') {
        return line;
    }
    Cow::Owned(format!("{}[CREDS]{}", &line[..rest], &line[at..]))
}

/// Masque les mots de passe des commandes d'auth (le token de commande doit commencer par `/`
/// — évite de masquer le mot « login » apparaissant dans une phrase).
fn mask_auth_passwords(line: Cow<str>) -> Cow<str> {
    let Some(i) = line.split_whitespace().position(is_auth_cmd) else {
        return line; // aucune commande d'auth → inchangé, pas de réallocation
    };
    let mut out: Vec<String> = line.split_whitespace().map(|s| s.to_string()).collect();
    let mut j = i + 1;
    // Saute une éventuelle sous-commande (ex. `/authme register <pw>`).
    if j < out.len() && AUTH_SUBCMDS.contains(&out[j].to_lowercase().as_str()) {
        j += 1;
    }
    let mut masked = 0;
    while j < out.len() && masked < 2 {
        if out[j].starts_with('/') {
            break; // nouvelle commande → on s'arrête
        }
        out[j] = "***".to_string();
        masked += 1;
        j += 1;
    }
    if masked == 0 {
        return line;
    }
    Cow::Owned(out.join(" "))
}

fn is_auth_cmd(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('/') else {
        return false;
    };
    AUTH_CMDS.contains(&rest.to_lowercase().as_str())
}

/// Remplace les adresses IPv4 (octets 0-255) par `[IP]`.
fn mask_ipv4(s: Cow<str>) -> Cow<str> {
    // Rejet rapide : une IPv4 exige au moins trois points → la plupart des lignes sortent sans scan.
    if s.matches('.').count() < 3 {
        return s;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || (!chars[i - 1].is_ascii_digit() && chars[i - 1] != '.');
        if boundary && chars[i].is_ascii_digit() {
            if let Some(end) = match_ipv4(&chars, i) {
                out.push_str("[IP]");
                i = end;
                changed = true;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    if changed {
        Cow::Owned(out)
    } else {
        s // aucun octet valide → on rend l'entrée intacte, sans allouer plus loin
    }
}

fn match_ipv4(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    for group in 0..4 {
        if group > 0 {
            if i >= chars.len() || chars[i] != '.' {
                return None;
            }
            i += 1;
        }
        let g_start = i;
        let mut val: u32 = 0;
        while i < chars.len() && chars[i].is_ascii_digit() && i - g_start < 3 {
            val = val * 10 + (chars[i] as u32 - '0' as u32);
            i += 1;
        }
        if i == g_start || val > 255 {
            return None;
        }
    }
    // Rejette si un 5ᵉ groupe suit (`1.2.3.4.5`) ou un chiffre colle (peu probable ici).
    if i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
        return None;
    }
    Some(i)
}

/// Remplace les e-mails par `[EMAIL]`.
fn mask_emails(s: Cow<str>) -> Cow<str> {
    if !s.contains('@') {
        return s;
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '@' && i > 0 && is_email_char(chars[i - 1]) {
            let mut start = i;
            while start > 0 && is_email_char(chars[start - 1]) {
                start -= 1;
            }
            let mut end = i + 1;
            while end < chars.len() && is_email_char(chars[end]) {
                end += 1;
            }
            let domain: String = chars[i + 1..end].iter().collect();
            if domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.') {
                for _ in 0..(i - start) {
                    out.pop(); // retire la partie locale déjà écrite
                }
                out.push_str("[EMAIL]");
                i = end;
                changed = true;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    if changed {
        Cow::Owned(out)
    } else {
        s // un `@` sans e-mail valide → intact
    }
}

fn is_email_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | '%')
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn masks_login_password() {
        assert_eq!(redact("/login hunter2"), "/login ***");
        assert_eq!(redact("/register pass pass"), "/register *** ***");
        assert_eq!(redact("/authme register secret"), "/authme register ***");
        assert_eq!(
            redact("[12:00 INFO]: Bob issued server command: /login topsecret"),
            "[12:00 INFO]: Bob issued server command: /login ***"
        );
    }

    #[test]
    fn does_not_mask_plain_word_login() {
        assert_eq!(redact("le login est cassé"), "le login est cassé");
    }

    #[test]
    fn masks_ipv4_but_not_versions() {
        assert_eq!(redact("connexion depuis 192.168.1.42 ok"), "connexion depuis [IP] ok");
        assert_eq!(redact("version 1.21.1 chargée"), "version 1.21.1 chargée");
        assert_eq!(redact("999.1.1.1"), "999.1.1.1"); // octet invalide → pas une IP
    }

    #[test]
    fn masks_email() {
        assert_eq!(redact("contact ai@octogency.com svp"), "contact [EMAIL] svp");
    }

    #[test]
    fn masks_config_secrets_and_conn_strings() {
        assert_eq!(redact("rcon.password=SuperSecret"), "rcon.password= [SECRET]");
        assert_eq!(redact("  token: abcdef123"), "  token: [SECRET]");
        assert!(redact("db: mysql://root:pw@host/app").contains("[CREDS]"));
        // Clés non sensibles et valeurs vides : intactes.
        assert_eq!(redact("server-port=25565"), "server-port=25565");
        assert_eq!(redact("view-distance: 8"), "view-distance: 8");
        assert_eq!(redact("rcon.password="), "rcon.password=");
    }
}
