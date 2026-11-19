//! JSON des configs de mods : analyseur à positions, qui tolère les commentaires (`//`, `/* */`)
//! et les virgules finales (JSONC, fréquents dans les configs Fabric). Une valeur est remplacée
//! dans sa seule plage ; une clé ajoutée prend l'indentation de ses sœurs. Le reste du fichier
//! (ordre des clés, commentaires, espaces) ne bouge pas.

use super::Seg;
use serde_json::{Map, Value};
use std::ops::Range;

const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    start: usize,
    end: usize,
}

#[derive(Debug, Clone)]
enum Kind {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<Node>),
    Obj(Vec<Member>),
}

#[derive(Debug, Clone)]
struct Member {
    key: String,
    key_start: usize,
    value: Node,
}

struct P<'a> {
    s: &'a [u8],
    text: &'a str,
    i: usize,
}

impl<'a> P<'a> {
    fn err<T>(&self, m: &str) -> Result<T, String> {
        let line = self.text[..self.i.min(self.text.len())]
            .matches('\n')
            .count()
            + 1;
        Err(format!("JSON invalide ligne {line} : {m}"))
    }

    /// Blancs et commentaires.
    fn skip(&mut self) -> Result<(), String> {
        loop {
            while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
                self.i += 1;
            }
            if self.s[self.i..].starts_with(b"//") {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.s[self.i..].starts_with(b"/*") {
                match self.text[self.i + 2..].find("*/") {
                    Some(p) => self.i += 2 + p + 2,
                    None => return self.err("commentaire /* non fermé"),
                }
            } else {
                return Ok(());
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return self.err("imbrication trop profonde");
        }
        self.skip()?;
        let start = self.i;
        let kind = match self.s.get(self.i) {
            None => return self.err("valeur attendue"),
            Some(b'{') => {
                self.i += 1;
                let mut members = Vec::new();
                loop {
                    self.skip()?;
                    match self.s.get(self.i) {
                        Some(b'}') => {
                            self.i += 1;
                            break;
                        }
                        Some(b'"') => {
                            let key_start = self.i;
                            let key = self.string()?;
                            self.skip()?;
                            if self.s.get(self.i) != Some(&b':') {
                                return self.err("`:` attendu après une clé");
                            }
                            self.i += 1;
                            let value = self.value(depth + 1)?;
                            members.push(Member {
                                key,
                                key_start,
                                value,
                            });
                            self.skip()?;
                            match self.s.get(self.i) {
                                Some(b',') => self.i += 1,
                                Some(b'}') => {}
                                _ => return self.err("`,` ou `}` attendu"),
                            }
                        }
                        _ => return self.err("clé entre guillemets ou `}` attendue"),
                    }
                }
                Kind::Obj(members)
            }
            Some(b'[') => {
                self.i += 1;
                let mut items = Vec::new();
                loop {
                    self.skip()?;
                    if self.s.get(self.i) == Some(&b']') {
                        self.i += 1;
                        break;
                    }
                    items.push(self.value(depth + 1)?);
                    self.skip()?;
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {}
                        _ => return self.err("`,` ou `]` attendu"),
                    }
                }
                Kind::Arr(items)
            }
            Some(b'"') => Kind::Str(self.string()?),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Kind::Bool(true)
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Kind::Bool(false)
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Kind::Null
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                while self.i < self.s.len()
                    && matches!(
                        self.s[self.i],
                        b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9'
                    )
                {
                    self.i += 1;
                }
                let raw = &self.text[start..self.i];
                if serde_json::from_str::<serde_json::Number>(raw).is_err() {
                    return self.err(&format!("nombre invalide « {raw} »"));
                }
                Kind::Num(raw.to_string())
            }
            _ => return self.err("valeur attendue"),
        };
        Ok(Node {
            kind,
            start,
            end: self.i,
        })
    }

    fn string(&mut self) -> Result<String, String> {
        let start = self.i;
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.s.get(self.i) else {
                return self.err("chaîne non fermée");
            };
            match c {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\\' => {
                    // Séquence d'échappement : serde_json la décode (sûr et exact).
                    let esc_end = if self.s.get(self.i + 1) == Some(&b'u') {
                        self.i + 6
                    } else {
                        self.i + 2
                    };
                    if esc_end > self.s.len() {
                        return self.err("échappement incomplet");
                    }
                    self.i = esc_end;
                }
                b'\n' => return self.err("saut de ligne dans une chaîne"),
                _ => {
                    let ch = self.text[self.i..].chars().next().unwrap_or('\0');
                    self.i += ch.len_utf8();
                }
            }
        }
        let raw = &self.text[start..self.i];
        match serde_json::from_str::<String>(raw) {
            Ok(s) => out.push_str(&s),
            Err(_) => return self.err("chaîne invalide (échappement)"),
        }
        Ok(out)
    }
}

fn parse(text: &str) -> Result<Node, String> {
    let mut p = P {
        s: text.as_bytes(),
        text,
        i: 0,
    };
    let root = p.value(0)?;
    p.skip()?;
    if p.i < text.len() {
        return p.err("texte après la valeur principale");
    }
    Ok(root)
}

fn to_json(n: &Node) -> Value {
    match &n.kind {
        Kind::Null => Value::Null,
        Kind::Bool(b) => Value::Bool(*b),
        Kind::Num(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
        Kind::Str(s) => Value::String(s.clone()),
        Kind::Arr(items) => Value::Array(items.iter().map(to_json).collect()),
        Kind::Obj(members) => Value::Object(
            members
                .iter()
                .map(|m| (m.key.clone(), to_json(&m.value)))
                .collect::<Map<_, _>>(),
        ),
    }
}

/// Arbre typé du document.
pub(super) fn tree(text: &str) -> Result<Value, String> {
    if text.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    Ok(to_json(&parse(text)?))
}

// --- Rendu ------------------------------------------------------------------------------------------

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind('\n').map_or(0, |i| i + 1)
}

/// Indentation (blancs de tête) de la ligne de `pos`.
fn indent_of(text: &str, pos: usize) -> &str {
    let ls = line_start(text, pos);
    let line = &text[ls..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Unité d'indentation du fichier (premier décalage observé), `"  "` par défaut.
fn unit(text: &str, root: &Node) -> String {
    fn find(text: &str, n: &Node) -> Option<String> {
        let children: Vec<usize> = match &n.kind {
            Kind::Obj(m) => m.iter().map(|x| x.key_start).collect(),
            Kind::Arr(a) => a.iter().map(|x| x.start).collect(),
            _ => return None,
        };
        let first = *children.first()?;
        if line_start(text, first) != line_start(text, n.start) {
            let parent = indent_of(text, n.start);
            let child = indent_of(text, first);
            if let Some(extra) = child.strip_prefix(parent).filter(|e| !e.is_empty()) {
                return Some(extra.to_string());
            }
        }
        match &n.kind {
            Kind::Obj(m) => m.iter().find_map(|x| find(text, &x.value)),
            Kind::Arr(a) => a.iter().find_map(|x| find(text, x)),
            _ => None,
        }
    }
    find(text, root).unwrap_or_else(|| "  ".into())
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Valeur JSON indentée : `base` = indentation de la ligne qui l'ouvre.
fn render(v: &Value, base: &str, unit: &str, nl: &str) -> String {
    match v {
        Value::Array(a) if !a.is_empty() => {
            let inner = format!("{base}{unit}");
            let items: Vec<String> = a
                .iter()
                .map(|x| format!("{inner}{}", render(x, &inner, unit, nl)))
                .collect();
            format!("[{nl}{}{nl}{base}]", items.join(&format!(",{nl}")))
        }
        Value::Object(m) if !m.is_empty() => {
            let inner = format!("{base}{unit}");
            let items: Vec<String> = m
                .iter()
                .map(|(k, x)| {
                    format!(
                        "{inner}{}: {}",
                        Value::String(k.clone()),
                        render(x, &inner, unit, nl)
                    )
                })
                .collect();
            format!("{{{nl}{}{nl}{base}}}", items.join(&format!(",{nl}")))
        }
        other => other.to_string(),
    }
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    format!("{}{with}{}", &text[..range.start], &text[range.end..])
}

/// Position juste après une éventuelle virgule finale (blancs et commentaires sautés) qui suit
/// `from` ; `None` s'il n'y en a pas.
fn trailing_comma(text: &str, from: usize) -> Option<usize> {
    let mut p = P {
        s: text.as_bytes(),
        text,
        i: from,
    };
    p.skip().ok()?;
    (p.s.get(p.i) == Some(&b',')).then_some(p.i + 1)
}

// --- Édition ------------------------------------------------------------------------------------------

/// Pose `value` au bout de `path`.
pub(super) fn set(
    text: &str,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<String, String> {
    if text.trim().is_empty() {
        let v = super::nest(path, value)?;
        return Ok(format!("{}\n", render(&v, "", "  ", "\n")));
    }
    let root = parse(text)?;
    let u = unit(text, &root);
    let nl = newline(text);
    let mut node = &root;
    for (i, seg) in path.iter().enumerate() {
        let rest = &path[i..];
        match (seg, &node.kind) {
            (Seg::Key(k), Kind::Obj(members)) => {
                let found: Vec<&Member> = members.iter().filter(|m| &m.key == k).collect();
                match found.as_slice() {
                    [m] => node = &m.value,
                    [] if create_missing => {
                        return insert_member(
                            text,
                            node,
                            k,
                            &super::nest(&rest[1..], value)?,
                            &u,
                            nl,
                        )
                    }
                    [] => return Err("clé absente (create_missing=false)".into()),
                    _ => return Err(format!("clé « {k} » en double dans le fichier")),
                }
            }
            (Seg::Index(n), Kind::Arr(items)) => {
                if *n < items.len() {
                    node = &items[*n];
                } else if *n == items.len() && create_missing {
                    return append_item(text, node, &super::nest(&rest[1..], value)?, &u, nl);
                } else {
                    return Err(format!(
                        "index [{n}] hors de la liste ({} élément(s))",
                        items.len()
                    ));
                }
            }
            (_, Kind::Null) if create_missing => {
                let base = indent_of(text, node.start).to_string();
                return Ok(splice(
                    text,
                    node.start..node.end,
                    &render(&super::nest(rest, value)?, &base, &u, nl),
                ));
            }
            (Seg::Key(_), _) => {
                return Err(format!(
                    "« {} » n'est pas un objet",
                    super::render_key_path(&path[..i])
                ))
            }
            (Seg::Index(_), _) => {
                return Err(format!(
                    "« {} » n'est pas une liste",
                    super::render_key_path(&path[..i])
                ))
            }
        }
    }
    let base = indent_of(text, node.start).to_string();
    Ok(splice(
        text,
        node.start..node.end,
        &render(value, &base, &u, nl),
    ))
}

fn insert_member(
    text: &str,
    obj: &Node,
    key: &str,
    value: &Value,
    u: &str,
    nl: &str,
) -> Result<String, String> {
    let Kind::Obj(members) = &obj.kind else {
        return Err("objet attendu".into());
    };
    let k = Value::String(key.to_string()).to_string();
    match members.last() {
        None => {
            let base = indent_of(text, obj.start).to_string();
            let inner = format!("{base}{u}");
            let body = format!(
                "{{{nl}{inner}{k}: {}{nl}{base}}}",
                render(value, &inner, u, nl)
            );
            Ok(splice(text, obj.start..obj.end, &body))
        }
        Some(last) => {
            let first = &members[0];
            let one_line = line_start(text, first.key_start) == line_start(text, obj.start);
            let (sep, ind) = if one_line {
                (" ".to_string(), String::new())
            } else {
                (nl.to_string(), indent_of(text, first.key_start).to_string())
            };
            let member = format!("{k}: {}", render(value, &ind, u, nl));
            match trailing_comma(text, last.value.end) {
                Some(after) => Ok(splice(text, after..after, &format!("{sep}{ind}{member},"))),
                None => Ok(splice(
                    text,
                    last.value.end..last.value.end,
                    &format!(",{sep}{ind}{member}"),
                )),
            }
        }
    }
}

fn append_item(text: &str, arr: &Node, value: &Value, u: &str, nl: &str) -> Result<String, String> {
    let Kind::Arr(items) = &arr.kind else {
        return Err("liste attendue".into());
    };
    match items.last() {
        None => {
            let base = indent_of(text, arr.start).to_string();
            let inner = format!("{base}{u}");
            let body = format!("[{nl}{inner}{}{nl}{base}]", render(value, &inner, u, nl));
            Ok(splice(text, arr.start..arr.end, &body))
        }
        Some(last) => {
            let one_line = line_start(text, items[0].start) == line_start(text, arr.start);
            let (sep, ind) = if one_line {
                (" ".to_string(), String::new())
            } else {
                (nl.to_string(), indent_of(text, items[0].start).to_string())
            };
            let item = render(value, &ind, u, nl);
            match trailing_comma(text, last.end) {
                Some(after) => Ok(splice(text, after..after, &format!("{sep}{ind}{item},"))),
                None => Ok(splice(
                    text,
                    last.end..last.end,
                    &format!(",{sep}{ind}{item}"),
                )),
            }
        }
    }
}

// --- Secrets ------------------------------------------------------------------------------------------

/// Plages des valeurs secrètes.
pub(super) fn secret_spans(text: &str) -> Vec<Range<usize>> {
    fn walk(n: &Node, secret: bool, out: &mut Vec<Range<usize>>) {
        match &n.kind {
            Kind::Obj(members) => members
                .iter()
                .for_each(|m| walk(&m.value, secret || super::is_secret_key(&m.key), out)),
            Kind::Arr(items) => items.iter().for_each(|i| walk(i, secret, out)),
            _ if secret && super::maskable(&to_json(n)) => out.push(n.start..n.end),
            _ => {}
        }
    }
    let Ok(root) = parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk(&root, false, &mut out);
    out
}
