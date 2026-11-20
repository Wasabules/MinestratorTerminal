//! `.properties` (règles de `java.util.Properties`) : lignes `clé=valeur`, `clé: valeur` ou
//! `clé valeur`, commentaires `#`/`!`, continuation par `\` final, échappements `\uXXXX`. La clé
//! est prise **entière** (`rcon.password`). Une valeur modifiée remplace la seule partie valeur de
//! sa ligne (séparateur et clé gardés tels qu'écrits) ; une clé absente est ajoutée en fin de
//! fichier. Valeurs écrites en ASCII (`\uXXXX` au-delà) : lues correctement que le fichier soit
//! chargé en ISO-8859-1 (Minecraft) ou en UTF-8.

use super::Seg;
use serde_json::{Map, Value};
use std::ops::Range;

/// Une entrée `clé=valeur` (ligne logique).
#[derive(Debug, Clone)]
struct Entry {
    key: String,
    value: String,
    /// Début de la ligne logique (octets).
    start: usize,
    /// Début de la partie valeur (octets) dans le texte.
    value_start: usize,
    /// Fin de la ligne logique (avant le saut de ligne).
    end: usize,
    /// Un séparateur (`=`, `:` ou blanc) est écrit.
    has_sep: bool,
}

/// Fin de ligne logique : une ligne terminée par un nombre impair de `\` continue.
fn continues(line: &str) -> bool {
    line.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let mut pending_high: Option<u32> = None;
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if let Ok(code) = u32::from_str_radix(&hex, 16) {
                    if (0xD800..0xDC00).contains(&code) {
                        pending_high = Some(code);
                        continue;
                    }
                    let code = match pending_high.take() {
                        Some(h) if (0xDC00..0xE000).contains(&code) => {
                            0x10000 + ((h - 0xD800) << 10) + (code - 0xDC00)
                        }
                        _ => code,
                    };
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Entrées du texte (lignes logiques), dans l'ordre.
fn entries(text: &str) -> Vec<Entry> {
    let mut out = Vec::new();
    let mut pos = 0;
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let start = pos;
        // Ligne logique : lignes physiques jointes tant qu'elles finissent par un `\` impair.
        let mut logical = String::new();
        let mut end;
        loop {
            let raw = lines[i];
            let body = raw.trim_end_matches(['\n', '\r']);
            end = pos + body.len();
            pos += raw.len();
            i += 1;
            let piece = if logical.is_empty() {
                body
            } else {
                body.trim_start_matches([' ', '\t', '\u{c}'])
            };
            if continues(body) && i < lines.len() {
                logical.push_str(&piece[..piece.len() - 1]);
                continue;
            }
            logical.push_str(piece);
            break;
        }
        let trimmed = logical.trim_start_matches([' ', '\t', '\u{c}']);
        if trimmed.is_empty() || trimmed.starts_with(['#', '!']) {
            continue;
        }
        // Clé : jusqu'au premier `=`, `:` ou blanc non échappé.
        let lead = logical.len() - trimmed.len();
        let bytes = trimmed.as_bytes();
        let mut k = 0;
        while k < bytes.len() {
            match bytes[k] {
                b'\\' => k += 2,
                b'=' | b':' | b' ' | b'\t' | 0x0c => break,
                _ => k += 1,
            }
        }
        let k = k.min(bytes.len());
        let key = unescape(&trimmed[..k]);
        let mut v = k;
        while v < bytes.len() && matches!(bytes[v], b' ' | b'\t' | 0x0c) {
            v += 1;
        }
        let mut has_sep = v > k;
        if v < bytes.len() && matches!(bytes[v], b'=' | b':') {
            v += 1;
            has_sep = true;
            while v < bytes.len() && matches!(bytes[v], b' ' | b'\t' | 0x0c) {
                v += 1;
            }
        }
        // Position de la valeur dans le texte : uniquement exacte pour une ligne physique seule ;
        // une valeur sur plusieurs lignes est remplacée en entier (de son début à la fin logique).
        let first_line_len = text[start..]
            .split('\n')
            .next()
            .unwrap_or("")
            .trim_end_matches('\r')
            .len();
        let value_start = (start + lead + v).min(start + first_line_len);
        out.push(Entry {
            key,
            value: unescape(&trimmed[v..]),
            start,
            value_start,
            end,
            has_sep,
        });
    }
    out
}

/// Arbre : objet plat `clé → valeur` (la dernière occurrence d'une clé l'emporte, comme Java).
pub(super) fn tree(text: &str) -> Value {
    let mut m = Map::new();
    for e in entries(text) {
        m.insert(e.key, Value::String(e.value));
    }
    Value::Object(m)
}

fn escape(s: &str, is_key: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{c}' => out.push_str("\\f"),
            ' ' if is_key || i == 0 => out.push_str("\\ "),
            '=' | ':' | '#' | '!' if is_key => {
                out.push('\\');
                out.push(c);
            }
            '#' | '!' if i == 0 => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{unit:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Pose `value` (texte) sous la clé.
pub(super) fn set(
    text: &str,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<String, String> {
    let [Seg::Key(key)] = path else {
        return Err(
            "un `.properties` n'a qu'un niveau : donne la clé entière (ex. `rcon.password`)".into(),
        );
    };
    let Value::String(v) = value else {
        return Err("valeur texte attendue".into());
    };
    let escaped = escape(v, false);
    match entries(text).into_iter().rev().find(|e| &e.key == key) {
        Some(e) => {
            let with = if e.has_sep {
                escaped
            } else {
                format!("={escaped}")
            };
            Ok(format!(
                "{}{with}{}",
                &text[..e.value_start],
                &text[e.end..]
            ))
        }
        None if !create_missing => Err("clé absente (create_missing=false)".into()),
        None => {
            let nl = newline(text);
            let mut out = text.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push_str(nl);
            }
            out.push_str(&format!("{}={escaped}{nl}", escape(key, true)));
            Ok(out)
        }
    }
}

/// Retire la clé : toutes ses lignes logiques (une occurrence plus ancienne reprendrait sinon
/// la main, Java gardant la dernière).
pub(super) fn remove(text: &str, path: &[Seg]) -> Result<String, String> {
    let [Seg::Key(key)] = path else {
        return Err(
            "un `.properties` n'a qu'un niveau : donne la clé entière (ex. `rcon.password`)".into(),
        );
    };
    let found: Vec<Entry> = entries(text)
        .into_iter()
        .filter(|e| &e.key == key)
        .collect();
    if found.is_empty() {
        return Err("clé absente".into());
    }
    let mut out = text.to_string();
    for e in found.iter().rev() {
        let mut end = e.end;
        if out[end..].starts_with("\r\n") {
            end += 2;
        } else if out[end..].starts_with('\n') {
            end += 1;
        }
        out.replace_range(e.start..end, "");
    }
    Ok(out)
}

/// Plages des valeurs secrètes.
pub(super) fn secret_spans(text: &str) -> Vec<Range<usize>> {
    entries(text)
        .into_iter()
        .filter(|e| super::is_secret_key(&e.key) && !e.value.is_empty() && e.end > e.value_start)
        .map(|e| e.value_start..e.end)
        .collect()
}
