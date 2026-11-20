//! YAML : analyse stricte à positions (`saphyr-parser`, YAML 1.2) et édition par plages de texte.
//!
//! L'arbre de nœuds garde, pour chaque clé et chaque valeur, sa position dans le texte. Une
//! modification remplace **la seule plage de la valeur** (un scalaire en place, en gardant le
//! commentaire de fin de ligne), ou insère une clé après la dernière ligne de sa section, à
//! l'indentation de ses sœurs. Le style du fichier est repris : pas d'indentation détecté,
//! listes indentées ou non sous leur clé, guillemets d'origine d'un scalaire remplacé.
//!
//! Types des scalaires nus résolus comme SnakeYAML (YAML 1.1, l'analyseur de Bukkit/Paper) :
//! `yes`/`no`/`on`/`off` sont des booléens, `0x1F` un entier… Une chaîne qui pourrait être lue
//! autrement (`'true'`, `'1.20'`, `'2024-01-01'`) est donc écrite entre guillemets.

use super::{nest, Seg};
use saphyr_parser::{Event, Parser, ScalarStyle, Span};
use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::ops::Range;

/// Profondeur d'imbrication maximale (garde-fou).
const MAX_DEPTH: usize = 200;

#[derive(Debug, Clone)]
pub(super) struct Node {
    pub kind: Kind,
    /// Début (octets) : premier caractère du scalaire (guillemet compris), `[`/`{` d'une
    /// collection en ligne, premier élément d'une collection en bloc.
    pub start: usize,
    /// Fin du contenu (octets, exclue).
    pub end: usize,
    /// Collection en ligne (`[a, b]`, `{a: b}`).
    pub flow: bool,
    /// Porte une ancre (`&nom`) : réutilisée ailleurs, jamais modifiée par clé.
    pub anchored: bool,
}

#[derive(Debug, Clone)]
pub(super) enum Kind {
    Scalar {
        value: String,
        style: ScalarStyle,
        tag_str: bool,
    },
    Map(Vec<Entry>),
    Seq(Vec<Node>),
    Alias(usize),
}

#[derive(Debug, Clone)]
pub(super) struct Entry {
    pub key: String,
    pub key_start: usize,
    pub key_end: usize,
    pub value: Node,
}

pub(super) struct Doc<'a> {
    pub text: &'a str,
    pub root: Option<Node>,
    anchors: HashMap<usize, Node>,
}

// --- Analyse ---------------------------------------------------------------------------------

/// Correspondance index de caractère (positions de saphyr) → octet.
struct CharMap(Option<Vec<usize>>);

impl CharMap {
    fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self(None);
        }
        let mut v: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
        v.push(text.len());
        Self(Some(v))
    }
    fn byte(&self, char_idx: usize) -> usize {
        match &self.0 {
            None => char_idx,
            Some(v) => v
                .get(char_idx)
                .copied()
                .unwrap_or_else(|| *v.last().unwrap_or(&0)),
        }
    }
}

struct Builder<'a> {
    text: &'a str,
    parser: Parser<'a, saphyr_parser::StrInput<'a>>,
    map: CharMap,
    anchors: HashMap<usize, Node>,
}

type Ev = (Event<'static>, Span);

fn owned(e: Event<'_>) -> Event<'static> {
    match e {
        Event::Scalar(v, s, a, t) => Event::Scalar(
            v.into_owned().into(),
            s,
            a,
            t.map(|t| std::borrow::Cow::Owned(t.into_owned())),
        ),
        Event::SequenceStart(a, t) => {
            Event::SequenceStart(a, t.map(|t| std::borrow::Cow::Owned(t.into_owned())))
        }
        Event::MappingStart(a, t) => {
            Event::MappingStart(a, t.map(|t| std::borrow::Cow::Owned(t.into_owned())))
        }
        Event::Nothing => Event::Nothing,
        Event::StreamStart => Event::StreamStart,
        Event::StreamEnd => Event::StreamEnd,
        Event::DocumentStart(b) => Event::DocumentStart(b),
        Event::DocumentEnd => Event::DocumentEnd,
        Event::Alias(a) => Event::Alias(a),
        Event::SequenceEnd => Event::SequenceEnd,
        Event::MappingEnd => Event::MappingEnd,
    }
}

impl<'a> Builder<'a> {
    fn next(&mut self) -> Result<Ev, String> {
        match self.parser.next_event() {
            Some(Ok((e, span))) => Ok((owned(e), span)),
            Some(Err(e)) => {
                let m = e.marker();
                Err(format!(
                    "YAML invalide ligne {}, colonne {} : {}",
                    m.line(),
                    m.col() + 1,
                    e.info()
                ))
            }
            None => Err("YAML : fin inattendue".into()),
        }
    }

    fn node(&mut self, ev: Ev, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("YAML : imbrication trop profonde".into());
        }
        let (event, span) = ev;
        let start = self.map.byte(span.start.index());
        let node = match event {
            Event::Scalar(value, style, anchor, tag) => {
                // Scalaire en bloc : `end` peut précéder `start` (scalaire vide → ligne de
                // l'indicateur) ; seules les plages `deux-points..fin de ligne` l'utilisent.
                let end = match style {
                    ScalarStyle::Literal | ScalarStyle::Folded => {
                        block_scalar_end(self.text, self.map.byte(span.end.index()))
                    }
                    _ => self.map.byte(span.end.index()),
                };
                let tag_str = tag.as_ref().is_some_and(|t| t.suffix == "str");
                let n = Node {
                    kind: Kind::Scalar {
                        value: value.into_owned(),
                        style,
                        tag_str,
                    },
                    start,
                    end,
                    flow: false,
                    anchored: anchor != 0,
                };
                if anchor != 0 {
                    self.anchors.insert(anchor, n.clone());
                }
                n
            }
            Event::Alias(id) => Node {
                kind: Kind::Alias(id),
                start,
                end: self.map.byte(span.end.index()),
                flow: false,
                anchored: false,
            },
            Event::SequenceStart(anchor, _) => {
                let flow = span.end.index() > span.start.index();
                let mut items = Vec::new();
                let end_span = loop {
                    let ev = self.next()?;
                    if matches!(ev.0, Event::SequenceEnd) {
                        break ev.1;
                    }
                    items.push(self.node(ev, depth + 1)?);
                };
                let end = if flow {
                    flow_close(self.text, self.map.byte(end_span.start.index()), b']')
                } else {
                    items.iter().map(|n| n.end).max().unwrap_or(start)
                };
                let n = Node {
                    kind: Kind::Seq(items),
                    start,
                    end,
                    flow,
                    anchored: anchor != 0,
                };
                if anchor != 0 {
                    self.anchors.insert(anchor, n.clone());
                }
                n
            }
            Event::MappingStart(anchor, _) => {
                let flow = span.end.index() > span.start.index();
                let mut entries = Vec::new();
                let end_span = loop {
                    let ev = self.next()?;
                    if matches!(ev.0, Event::MappingEnd) {
                        break ev.1;
                    }
                    let (key, key_start, key_end) = match &ev.0 {
                        Event::Scalar(v, _, _, _) => (v.to_string(), self.map.byte(ev.1.start.index()), self.map.byte(ev.1.end.index())),
                        _ => return Err("YAML : clé complexe (liste, objet ou alias en clé) non prise en charge".into()),
                    };
                    let vev = self.next()?;
                    let value = self.node(vev, depth + 1)?;
                    entries.push(Entry {
                        key,
                        key_start,
                        key_end,
                        value,
                    });
                };
                let end = if flow {
                    flow_close(self.text, self.map.byte(end_span.start.index()), b'}')
                } else {
                    entries
                        .iter()
                        .map(|e| e.value.end.max(e.key_end))
                        .max()
                        .unwrap_or(start)
                };
                let n = Node {
                    kind: Kind::Map(entries),
                    start,
                    end,
                    flow,
                    anchored: anchor != 0,
                };
                if anchor != 0 {
                    self.anchors.insert(anchor, n.clone());
                }
                n
            }
            other => return Err(format!("YAML : événement inattendu {other:?}")),
        };
        Ok(node)
    }
}

/// Fin d'une collection en ligne : position après son `]`/`}`, cherché depuis la fin du dernier
/// élément (l'étendue de fin donnée par saphyr englobe un commentaire qui suit).
fn flow_close(text: &str, from: usize, close: u8) -> usize {
    let b = text.as_bytes();
    let mut i = from.min(b.len());
    while i < b.len() {
        match b[i] {
            c if c == close => return i + 1,
            b' ' | b'\t' | b'\r' | b'\n' | b',' => i += 1,
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    from
}

/// Fin d'un scalaire en bloc (`|`, `>`) : fin de la dernière ligne non vide et hors commentaire
/// avant le jeton suivant (`next`). Scalaire vide : fin de la ligne de l'indicateur. Une dernière
/// ligne de contenu qui ressemble à un commentaire serait exclue : la relecture stricte refuserait
/// alors l'édition (valeur changée), jamais d'écriture fautive.
fn block_scalar_end(text: &str, next: usize) -> usize {
    let next = next.min(text.len());
    let mut end = if next >= text.len() {
        text.len()
    } else {
        line_start(text, next)
    };
    while end > 0 {
        let line_end = if text.as_bytes()[end - 1] == b'\n' {
            end - 1
        } else {
            end
        };
        let ls = line_start(text, line_end);
        let line = text[ls..line_end].trim_end_matches('\r');
        if !line.trim().is_empty() && !line.trim_start().starts_with('#') {
            return ls + line.len();
        }
        end = ls;
    }
    0
}

/// Analyse un document (un seul ; un document vide donne `root: None`).
pub(super) fn parse(text: &str) -> Result<Doc<'_>, String> {
    let mut b = Builder {
        text,
        parser: Parser::new_from_str(text),
        map: CharMap::new(text),
        anchors: HashMap::new(),
    };
    let mut root = None;
    let mut docs = 0;
    loop {
        let ev = b.next()?;
        match ev.0 {
            Event::StreamStart | Event::DocumentEnd | Event::Nothing => {}
            Event::StreamEnd => break,
            Event::DocumentStart(_) => {
                docs += 1;
                if docs > 1 {
                    return Err(
                        "plusieurs documents YAML (`---`) dans le fichier : non pris en charge"
                            .into(),
                    );
                }
                let first = b.next()?;
                if matches!(first.0, Event::DocumentEnd) {
                    continue;
                }
                root = Some(b.node(first, 0)?);
            }
            other => return Err(format!("YAML : événement inattendu {other:?}")),
        }
    }
    // Un document réduit à un nul implicite (fichier de commentaires) n'a pas de contenu.
    if let Some(Node {
        kind:
            Kind::Scalar {
                value,
                style: ScalarStyle::Plain,
                ..
            },
        start,
        end,
        ..
    }) = &root
    {
        if value.is_empty() && start == end {
            root = None;
        }
    }
    Ok(Doc {
        text,
        root,
        anchors: b.anchors,
    })
}

// --- Arbre typé ------------------------------------------------------------------------------

/// Résolution d'un scalaire nu, comme SnakeYAML (YAML 1.1).
pub(super) fn resolve_plain(s: &str) -> Value {
    match s {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" => {
            return Value::Bool(true)
        }
        "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" => {
            return Value::Bool(false)
        }
        _ => {}
    }
    if let Some(i) = parse_int(s) {
        return Value::from(i);
    }
    if is_float(s) {
        if let Some(n) = s
            .replace('_', "")
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
        {
            return Value::Number(n);
        }
    }
    Value::String(s.to_string())
}

fn parse_int(s: &str) -> Option<i64> {
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let clean: String = body.chars().filter(|&c| c != '_').collect();
    if clean.is_empty() || body.starts_with('_') {
        return None;
    }
    let v = if let Some(h) = clean.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()?
    } else if let Some(b) = clean.strip_prefix("0b") {
        i64::from_str_radix(b, 2).ok()?
    } else if clean.len() > 1 && clean.starts_with('0') {
        if !clean.bytes().all(|c| (b'0'..=b'7').contains(&c)) {
            return None;
        }
        i64::from_str_radix(&clean, 8).ok()?
    } else {
        if !clean.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        clean.parse::<i64>().ok()?
    };
    Some(if neg { -v } else { v })
}

/// Flottant YAML 1.1 : `[-+]?(\.[0-9]+|[0-9_]+(\.[0-9_]*)?)([eE][-+]?[0-9]+)?` (hors `.inf`/`.nan`).
fn is_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let digits = |i: &mut usize, allow_us: bool| {
        let st = *i;
        while *i < b.len() && (b[*i].is_ascii_digit() || (allow_us && b[*i] == b'_')) {
            *i += 1;
        }
        *i > st
    };
    if b.get(i) == Some(&b'.') {
        i += 1;
        if !digits(&mut i, false) {
            return false;
        }
    } else {
        if !digits(&mut i, true) {
            return false;
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i, true);
        }
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'-' | b'+')) {
            i += 1;
        }
        if !digits(&mut i, false) {
            return false;
        }
    }
    i == b.len()
}

impl Doc<'_> {
    pub(super) fn value(&self, n: &Node) -> Value {
        self.value_at(n, 0)
    }

    fn value_at(&self, n: &Node, depth: usize) -> Value {
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        match &n.kind {
            Kind::Scalar {
                value,
                style,
                tag_str,
            } => match style {
                ScalarStyle::Plain if !tag_str => resolve_plain(value),
                _ => Value::String(value.clone()),
            },
            Kind::Map(entries) => {
                let mut m = Map::new();
                for e in entries {
                    m.insert(e.key.clone(), self.value_at(&e.value, depth + 1));
                }
                Value::Object(m)
            }
            Kind::Seq(items) => {
                Value::Array(items.iter().map(|i| self.value_at(i, depth + 1)).collect())
            }
            Kind::Alias(id) => self
                .anchors
                .get(id)
                .map(|a| self.value_at(a, depth + 1))
                .unwrap_or(Value::Null),
        }
    }
}

/// Arbre typé du document (`null` s'il est vide).
pub(super) fn tree(text: &str) -> Result<Value, String> {
    let doc = parse(text)?;
    Ok(doc
        .root
        .as_ref()
        .map(|r| doc.value(r))
        .unwrap_or(Value::Null))
}

// --- Positions dans le texte -------------------------------------------------------------------

fn line_start(text: &str, pos: usize) -> usize {
    text[..pos.min(text.len())].rfind('\n').map_or(0, |i| i + 1)
}

/// Fin de la ligne de `pos` (position du `\r\n` ou du `\n`, ou fin du texte).
fn eol(text: &str, pos: usize) -> usize {
    let pos = pos.min(text.len());
    let e = text[pos..].find('\n').map_or(text.len(), |i| pos + i);
    if e > pos && text.as_bytes().get(e - 1) == Some(&b'\r') && e < text.len() {
        e - 1
    } else {
        e
    }
}

/// Colonne (en caractères) d'une position.
fn col(text: &str, pos: usize) -> usize {
    text[line_start(text, pos)..pos].chars().count()
}

fn newline(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Position juste après le `:` qui suit une clé.
fn colon_after(text: &str, key_end: usize) -> Result<usize, String> {
    let rest = &text[key_end..];
    let skip = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if rest[skip..].starts_with(':') {
        Ok(key_end + skip + 1)
    } else {
        Err("forme de clé inhabituelle (`?` ou clé multiligne) : non modifiable par clé".into())
    }
}

/// Commentaire de fin de ligne dans `text[from..to]` (rien d'autre que des blancs avant).
fn trailing_comment(text: &str, from: usize, to: usize) -> Option<String> {
    let s = text.get(from..to)?.trim();
    s.starts_with('#').then(|| s.to_string())
}

/// Colonne du tiret d'un élément de liste en bloc.
fn dash_col(text: &str, item_start: usize) -> usize {
    let ls = line_start(text, item_start);
    let before = &text[ls..item_start];
    match before.trim_end_matches([' ', '\t']).strip_suffix('-') {
        Some(head) => head.chars().count(),
        None => col(text, item_start).saturating_sub(2),
    }
}

// --- Style du fichier ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Style {
    /// Pas d'indentation des sections.
    step: usize,
    /// Décalage du tiret d'une liste sous sa clé (0 : `clé:\n- a` ; 2 : `clé:\n  - a`).
    seq_offset: usize,
    nl: &'static str,
}

impl Style {
    fn detect(doc: &Doc) -> Self {
        fn walk(doc: &Doc, n: &Node, steps: &mut Vec<usize>, offs: &mut Vec<usize>) {
            match &n.kind {
                Kind::Map(entries) => {
                    for e in entries {
                        let kc = col(doc.text, e.key_start);
                        match &e.value.kind {
                            Kind::Map(inner) if !e.value.flow => {
                                if let Some(f) = inner.first() {
                                    let c = col(doc.text, f.key_start);
                                    if c > kc {
                                        steps.push(c - kc);
                                    }
                                }
                            }
                            Kind::Seq(items) if !e.value.flow => {
                                if let Some(f) = items.first() {
                                    let d = dash_col(doc.text, f.start);
                                    if d >= kc {
                                        offs.push(d - kc);
                                    }
                                }
                            }
                            _ => {}
                        }
                        walk(doc, &e.value, steps, offs);
                    }
                }
                Kind::Seq(items) => items.iter().for_each(|i| walk(doc, i, steps, offs)),
                _ => {}
            }
        }
        let (mut steps, mut offs) = (Vec::new(), Vec::new());
        if let Some(r) = &doc.root {
            walk(doc, r, &mut steps, &mut offs);
        }
        // Pas le plus fréquent (à égalité, le plus petit).
        let step = most_common(&steps)
            .filter(|s| (1..=8).contains(s))
            .unwrap_or(2);
        let seq_offset = most_common(&offs).filter(|s| *s <= 8).unwrap_or(step);
        Self {
            step,
            seq_offset,
            nl: newline(doc.text),
        }
    }
}

fn most_common(v: &[usize]) -> Option<usize> {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for x in v {
        *counts.entry(*x).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map(|(k, _)| k)
}

// --- Rendu ---------------------------------------------------------------------------------------

/// Une chaîne pourrait-elle être lue autrement qu'en texte (booléen, nombre, nul, date) ?
fn looks_typed(s: &str) -> bool {
    !matches!(resolve_plain(s), Value::String(_))
        || s.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '+' | '.'))
        || (s.starts_with('-') && s[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.'))
        || matches!(
            s.to_ascii_lowercase().as_str(),
            ".inf" | "-.inf" | "+.inf" | ".nan"
        )
}

/// Texte écrivable sans guillemets (contexte bloc, ou en ligne si `flow`).
fn plain_ok(s: &str, flow: bool) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    if s.trim() != s || looks_typed(s) {
        return false;
    }
    if matches!(
        first,
        '-' | '?'
            | ':'
            | ','
            | '['
            | ']'
            | '{'
            | '}'
            | '#'
            | '&'
            | '*'
            | '!'
            | '|'
            | '>'
            | '\''
            | '"'
            | '%'
            | '@'
            | '`'
    ) {
        return false;
    }
    if s.contains(": ") || s.contains(" #") || s.ends_with(':') || s.chars().any(|c| c.is_control())
    {
        return false;
    }
    !(flow && s.contains([',', '[', ']', '{', '}']))
}

fn single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn double_quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Scalaire texte, dans le style d'origine si possible.
fn scalar_str(s: &str, prefer: Option<ScalarStyle>, flow: bool) -> String {
    let needs_double = s.chars().any(|c| c.is_control());
    match prefer {
        Some(ScalarStyle::DoubleQuoted) => double_quoted(s),
        Some(ScalarStyle::SingleQuoted) if !needs_double => single_quoted(s),
        _ if plain_ok(s, flow) => s.to_string(),
        _ if !needs_double => single_quoted(s),
        _ => double_quoted(s),
    }
}

fn key_str(k: &str) -> String {
    scalar_str(k, None, false)
}

/// Valeur écrivable sur une ligne (`None` : collection non vide, à écrire en bloc).
fn render_inline(v: &Value, prefer: Option<ScalarStyle>) -> Option<String> {
    Some(match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => scalar_str(s, prefer, false),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Object(m) if m.is_empty() => "{}".into(),
        _ => return None,
    })
}

/// Rendu en ligne de n'importe quelle valeur (`[a, b]`, `{a: 1}`).
fn render_flow(v: &Value) -> String {
    match v {
        Value::String(s) => scalar_str(s, None, true),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(render_flow).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, x)| format!("{}: {}", scalar_str(k, None, true), render_flow(x)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => render_inline(other, None).unwrap_or_default(),
    }
}

/// Lignes d'une section (indentation absolue `col`).
fn map_lines(m: &Map<String, Value>, col: usize, st: Style) -> Vec<String> {
    let mut out = Vec::new();
    for (k, v) in m {
        let head = format!("{}{}:", " ".repeat(col), key_str(k));
        match render_inline(v, None) {
            Some(s) => out.push(format!("{head} {s}")),
            None => {
                out.push(head);
                out.extend(block_lines(v, col, st));
            }
        }
    }
    out
}

/// Lignes d'une liste dont les tirets sont en colonne `dash`.
fn seq_lines(a: &[Value], dash: usize, st: Style) -> Vec<String> {
    let mut out = Vec::new();
    for item in a {
        let prefix = format!("{}- ", " ".repeat(dash));
        match (render_inline(item, None), item) {
            (Some(s), _) => out.push(format!("{prefix}{s}")),
            (None, Value::Object(m)) => {
                let mut lines = map_lines(m, dash + 2, st);
                if let Some(first) = lines.first_mut() {
                    *first = format!("{prefix}{}", first.trim_start());
                }
                out.extend(lines);
            }
            (None, other) => out.push(format!("{prefix}{}", render_flow(other))),
        }
    }
    out
}

/// Lignes de la valeur en bloc d'une clé en colonne `key_col`.
fn block_lines(v: &Value, key_col: usize, st: Style) -> Vec<String> {
    match v {
        Value::Object(m) => map_lines(m, key_col + st.step, st),
        Value::Array(a) => seq_lines(a, key_col + st.seq_offset, st),
        _ => Vec::new(),
    }
}

/// Lignes d'une valeur en bloc, à la colonne d'enfants donnée (sections existantes).
fn block_lines_at(
    v: &Value,
    child_col: Option<usize>,
    dash: Option<usize>,
    key_col: usize,
    st: Style,
) -> Vec<String> {
    match v {
        Value::Object(m) => map_lines(m, child_col.unwrap_or(key_col + st.step), st),
        Value::Array(a) => seq_lines(a, dash.unwrap_or(key_col + st.seq_offset), st),
        _ => Vec::new(),
    }
}

fn joined(lines: &[String], nl: &str) -> String {
    lines.iter().map(|l| format!("{nl}{l}")).collect()
}

// --- Édition --------------------------------------------------------------------------------------

/// Où vit le nœud modifié.
#[derive(Clone, Copy)]
enum Slot<'d> {
    Root,
    Entry(&'d Entry),
    Item { seq: &'d Node, index: usize },
}

fn splice(text: &str, range: Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

fn is_null_scalar(n: &Node) -> bool {
    matches!(&n.kind, Kind::Scalar { value, style: ScalarStyle::Plain, tag_str: false } if matches!(value.as_str(), "" | "~" | "null" | "Null" | "NULL"))
}

fn is_block_scalar(n: &Node) -> bool {
    matches!(
        &n.kind,
        Kind::Scalar {
            style: ScalarStyle::Literal | ScalarStyle::Folded,
            ..
        }
    )
}

fn is_block_collection(n: &Node) -> bool {
    matches!(n.kind, Kind::Map(_) | Kind::Seq(_)) && !n.flow
}

fn scalar_style(n: &Node) -> Option<ScalarStyle> {
    match &n.kind {
        Kind::Scalar {
            style: s @ (ScalarStyle::SingleQuoted | ScalarStyle::DoubleQuoted | ScalarStyle::Plain),
            ..
        } => Some(*s),
        _ => None,
    }
}

/// Pose `value` au bout de `path`.
pub(super) fn set(
    text: &str,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<String, String> {
    let doc = parse(text)?;
    let st = Style::detect(&doc);
    let Some(root) = &doc.root else {
        return append_root(text, path, value, st);
    };
    let mut node = root;
    let mut slot = Slot::Root;
    for (i, seg) in path.iter().enumerate() {
        let here = || super::render_key_path(&path[..i]);
        if let Kind::Alias(_) = node.kind {
            return Err(format!(
                "« {} » est un alias YAML (*…) : modifie l'ancre d'origine",
                here()
            ));
        }
        if node.anchored && i > 0 {
            return Err(format!("« {} » est ancrée (&…) et réutilisée ailleurs par alias : non modifiable par clé (edit_file)", here()));
        }
        match (seg, &node.kind) {
            (Seg::Key(k), Kind::Map(entries)) => {
                let found: Vec<&Entry> = entries.iter().filter(|e| &e.key == k).collect();
                match found.as_slice() {
                    [] if create_missing => {
                        return insert_into_map(&doc, slot, node, &path[i..], value, st)
                    }
                    [] => return Err("clé absente (create_missing=false)".into()),
                    [e] => {
                        slot = Slot::Entry(e);
                        node = &e.value;
                    }
                    _ => {
                        return Err(format!(
                            "clé « {k} » en double dans le fichier : corrige d'abord le doublon"
                        ))
                    }
                }
            }
            (Seg::Index(n), Kind::Seq(items)) => {
                if *n < items.len() {
                    slot = Slot::Item {
                        seq: node,
                        index: *n,
                    };
                    node = &items[*n];
                } else if *n == items.len() && create_missing {
                    return append_to_seq(&doc, slot, node, &path[i + 1..], value, st);
                } else {
                    return Err(format!(
                        "index [{n}] hors de la liste « {} » ({} élément(s))",
                        here(),
                        items.len()
                    ));
                }
            }
            _ if is_null_scalar(node) && create_missing => {
                if node.anchored {
                    return Err("valeur ancrée (&…) : non modifiable par clé".into());
                }
                return replace(&doc, slot, node, &nest(&path[i..], value)?, st);
            }
            (Seg::Key(_), _) => {
                return Err(format!(
                    "« {} » n'est pas une section (c'est une valeur ou une liste)",
                    here()
                ))
            }
            (Seg::Index(_), _) => return Err(format!("« {} » n'est pas une liste", here())),
        }
    }
    if node.anchored || matches!(node.kind, Kind::Alias(_)) {
        return Err(
            "valeur ancrée (&…) ou alias (*…), réutilisée ailleurs : non modifiable par clé".into(),
        );
    }
    replace(&doc, slot, node, value, st)
}

/// Document vide (ou fait de commentaires) : la section est ajoutée à la fin.
fn append_root(text: &str, path: &[Seg], value: &Value, st: Style) -> Result<String, String> {
    let Value::Object(m) = nest(path, value)? else {
        return Err("document vide : le chemin doit commencer par une clé".into());
    };
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(st.nl);
    }
    for l in map_lines(&m, 0, st) {
        out.push_str(&l);
        out.push_str(st.nl);
    }
    Ok(out)
}

/// Remplace la valeur `node` (vivant dans `slot`) par `value`.
fn replace(doc: &Doc, slot: Slot, node: &Node, value: &Value, st: Style) -> Result<String, String> {
    let text = doc.text;
    match slot {
        Slot::Root => {
            if node.flow {
                return Ok(splice(text, node.start..node.end, &render_flow(value)));
            }
            let c = col(text, node.start);
            let lines = match value {
                Value::Object(m) if !m.is_empty() => map_lines(m, c, st),
                Value::Array(a) if !a.is_empty() => seq_lines(a, c, st),
                other => vec![format!(
                    "{}{}",
                    " ".repeat(c),
                    render_inline(other, scalar_style(node)).unwrap_or_default()
                )],
            };
            let ls = line_start(text, node.start);
            let end = if is_block_collection(node) || is_block_scalar(node) {
                eol(text, node.end)
            } else {
                node.end.max(node.start)
            };
            Ok(splice(text, ls..end, &lines.join(st.nl)))
        }
        Slot::Entry(e) => replace_entry_value(doc, e, value, st),
        Slot::Item { seq, index } => replace_item(doc, slot, seq, index, value, st),
    }
}

fn replace_entry_value(doc: &Doc, e: &Entry, value: &Value, st: Style) -> Result<String, String> {
    let text = doc.text;
    let old = &e.value;
    let key_col = col(text, e.key_start);
    let colon = colon_after(text, e.key_end)?;
    let key_eol = eol(text, e.key_start);
    let old_null_empty = is_null_scalar(old) && old.start == old.end;
    let inline = if old.flow {
        Some(render_flow(value))
    } else {
        render_inline(value, scalar_style(old))
    };
    match inline {
        Some(s) => {
            if is_block_collection(old) || is_block_scalar(old) {
                let comment =
                    trailing_comment(text, colon, key_eol).filter(|_| is_block_collection(old));
                let with = match comment {
                    Some(c) => format!(" {s} {c}"),
                    None => format!(" {s}"),
                };
                Ok(splice(text, colon..eol(text, old.end), &with))
            } else if old_null_empty || old.start < colon {
                Ok(splice(text, colon..colon, &format!(" {s}")))
            } else {
                Ok(splice(text, old.start..old.end, &s))
            }
        }
        None => {
            if is_block_collection(old) {
                let (child, dash) = match &old.kind {
                    Kind::Map(es) => (es.first().map(|f| col(text, f.key_start)), None),
                    Kind::Seq(items) => (None, items.first().map(|f| dash_col(text, f.start))),
                    _ => (None, None),
                };
                // Une liste remplace une section (ou l'inverse) : style par défaut du fichier.
                let (child, dash) = match (value, &old.kind) {
                    (Value::Object(_), Kind::Map(_)) => (child, None),
                    (Value::Array(_), Kind::Seq(_)) => (None, dash),
                    _ => (None, None),
                };
                let lines = block_lines_at(value, child, dash, key_col, st);
                Ok(splice(
                    text,
                    key_eol..eol(text, old.end),
                    &joined(&lines, st.nl),
                ))
            } else {
                let empty = old_null_empty || old.start < colon;
                let end = if empty { key_eol } else { eol(text, old.end) };
                let from = if empty { colon } else { old.end };
                let comment = trailing_comment(text, from, end).filter(|_| !is_block_scalar(old));
                let lines = block_lines(value, key_col, st);
                let head = comment.map(|c| format!(" {c}")).unwrap_or_default();
                Ok(splice(
                    text,
                    colon..end.max(colon),
                    &format!("{head}{}", joined(&lines, st.nl)),
                ))
            }
        }
    }
}

fn replace_item(
    doc: &Doc,
    slot: Slot,
    seq: &Node,
    index: usize,
    value: &Value,
    st: Style,
) -> Result<String, String> {
    let text = doc.text;
    let Kind::Seq(items) = &seq.kind else {
        return Err("liste attendue".into());
    };
    let item = &items[index];
    if seq.flow {
        // Liste en ligne : réécrite en entier, en ligne.
        let mut v = doc.value(seq);
        if let Value::Array(a) = &mut v {
            a[index] = value.clone();
        }
        let _ = slot;
        return Ok(splice(text, seq.start..seq.end, &render_flow(&v)));
    }
    if is_block_scalar(item) {
        return Err(
            "élément de liste en bloc `|`/`>` : non modifiable par clé (utilise edit_file)".into(),
        );
    }
    let dash = dash_col(text, item.start);
    let region = if is_block_collection(item) {
        item.start..eol(text, item.end)
    } else {
        item.start..item.end
    };
    let with = item_text(value, dash, scalar_style(item), st);
    Ok(splice(text, region, &with))
}

/// Texte d'un élément de liste, sans son tiret (la première ligne suit `- `).
fn item_text(value: &Value, dash: usize, prefer: Option<ScalarStyle>, st: Style) -> String {
    match (render_inline(value, prefer), value) {
        (Some(s), _) => s,
        (None, Value::Object(m)) => {
            let lines = map_lines(m, dash + 2, st);
            let mut out = lines
                .first()
                .map(|l| l.trim_start().to_string())
                .unwrap_or_default();
            for l in &lines[1..] {
                out.push_str(st.nl);
                out.push_str(l);
            }
            out
        }
        (None, other) => render_flow(other),
    }
}

/// Ajoute la clé `rest[0]` (et la suite du chemin) dans la section `map`.
fn insert_into_map(
    doc: &Doc,
    slot: Slot,
    map: &Node,
    rest: &[Seg],
    value: &Value,
    st: Style,
) -> Result<String, String> {
    let text = doc.text;
    let Some(Seg::Key(key)) = rest.first() else {
        return Err("clé attendue".into());
    };
    let nested = nest(&rest[1..], value)?;
    if map.anchored {
        return Err("section ancrée (&…), réutilisée ailleurs : non modifiable par clé".into());
    }
    let Kind::Map(entries) = &map.kind else {
        return Err("section attendue".into());
    };
    if map.flow || entries.is_empty() {
        let mut v = doc.value(map);
        if let Value::Object(m) = &mut v {
            m.insert(key.clone(), nested);
        }
        return match slot {
            Slot::Root => Ok(splice(text, map.start..map.end, &render_flow(&v))),
            _ => replace(doc, slot, map, &v, st),
        };
    }
    let key_col = col(text, entries[0].key_start);
    let mut m = Map::new();
    m.insert(key.clone(), nested);
    let lines = map_lines(&m, key_col, st);
    let at = eol(text, map.end);
    Ok(splice(text, at..at, &joined(&lines, st.nl)))
}

/// Ajoute un élément à la fin de la liste `seq`.
fn append_to_seq(
    doc: &Doc,
    slot: Slot,
    seq: &Node,
    rest: &[Seg],
    value: &Value,
    st: Style,
) -> Result<String, String> {
    let text = doc.text;
    let item = nest(rest, value)?;
    if seq.anchored {
        return Err("liste ancrée (&…), réutilisée ailleurs : non modifiable par clé".into());
    }
    let Kind::Seq(items) = &seq.kind else {
        return Err("liste attendue".into());
    };
    if seq.flow || items.is_empty() {
        let mut v = doc.value(seq);
        if let Value::Array(a) = &mut v {
            a.push(item);
        }
        return match slot {
            Slot::Root => Ok(splice(text, seq.start..seq.end, &render_flow(&v))),
            _ => replace(doc, slot, seq, &v, st),
        };
    }
    let dash = dash_col(text, items[0].start);
    let line = format!("{}- {}", " ".repeat(dash), item_text(&item, dash, None, st));
    let at = eol(text, seq.end);
    Ok(splice(text, at..at, &format!("{}{line}", st.nl)))
}

// --- Retrait d'une clé ------------------------------------------------------------------------------

/// Position après le saut de ligne qui suit `pos` (fin de ligne de [`eol`]).
fn after_newline(text: &str, pos: usize) -> usize {
    if text[pos..].starts_with("\r\n") {
        pos + 2
    } else if text[pos..].starts_with('\n') {
        pos + 1
    } else {
        pos
    }
}

/// Retire la clé au bout de `path` (dernier segment : une clé de section). Section du chemin
/// absente, ancre, alias ou clé en double : erreur, rien n'est retiré. La dernière clé d'une
/// section en bloc laisse une section vide (`{}`), jamais une valeur nulle.
pub(super) fn remove(text: &str, path: &[Seg]) -> Result<String, String> {
    let Some((Seg::Key(last), parents)) = path.split_last() else {
        return Err(
            "retrait d'un élément de liste par index non pris en charge : réécris la liste entière"
                .into(),
        );
    };
    let doc = parse(text)?;
    let st = Style::detect(&doc);
    let Some(root) = &doc.root else {
        return Err("clé absente (document vide)".into());
    };
    let mut node = root;
    let mut slot = Slot::Root;
    for (i, seg) in parents.iter().enumerate() {
        let here = || super::render_key_path(&path[..i]);
        if let Kind::Alias(_) = node.kind {
            return Err(format!(
                "« {} » est un alias YAML (*…) : modifie l'ancre d'origine",
                here()
            ));
        }
        if node.anchored && i > 0 {
            return Err(format!(
                "« {} » est ancrée (&…) et réutilisée ailleurs par alias : retrait refusé (edit_file)",
                here()
            ));
        }
        match (seg, &node.kind) {
            (Seg::Key(k), Kind::Map(entries)) => {
                let found: Vec<&Entry> = entries.iter().filter(|e| &e.key == k).collect();
                match found.as_slice() {
                    [] => return Err(format!("section « {k} » absente")),
                    [e] => {
                        slot = Slot::Entry(e);
                        node = &e.value;
                    }
                    _ => {
                        return Err(format!(
                            "clé « {k} » en double dans le fichier : corrige d'abord le doublon"
                        ))
                    }
                }
            }
            (Seg::Index(n), Kind::Seq(items)) => match items.get(*n) {
                Some(item) => {
                    slot = Slot::Item {
                        seq: node,
                        index: *n,
                    };
                    node = item;
                }
                None => {
                    return Err(format!(
                        "index [{n}] hors de la liste « {} » ({} élément(s))",
                        here(),
                        items.len()
                    ))
                }
            },
            (Seg::Key(_), _) => return Err(format!("« {} » n'est pas une section", here())),
            (Seg::Index(_), _) => return Err(format!("« {} » n'est pas une liste", here())),
        }
    }
    if node.anchored || matches!(node.kind, Kind::Alias(_)) {
        return Err(
            "section ancrée (&…) ou alias (*…), réutilisée ailleurs : retrait refusé (edit_file)"
                .into(),
        );
    }
    let Kind::Map(entries) = &node.kind else {
        return Err(format!(
            "« {} » n'est pas une section",
            super::render_key_path(parents)
        ));
    };
    let idx: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| &e.key == last)
        .map(|(i, _)| i)
        .collect();
    let i = match idx.as_slice() {
        [] => return Err("clé absente".into()),
        [i] => *i,
        _ => {
            return Err(format!(
                "clé « {last} » en double dans le fichier : corrige d'abord le doublon"
            ))
        }
    };
    let e = &entries[i];
    if e.value.anchored {
        return Err(
            "valeur ancrée (&…), réutilisée ailleurs par alias : retrait refusé (edit_file)".into(),
        );
    }
    // Section en ligne, ou dernière clé d'une section : la section est réécrite (vide : `{}`).
    if node.flow || entries.len() == 1 {
        let mut v = doc.value(node);
        if let Value::Object(m) = &mut v {
            m.remove(last.as_str());
        }
        return replace(&doc, slot, node, &v, st);
    }
    let colon = colon_after(text, e.key_end)?;
    let ls = line_start(text, e.key_start);
    let value_empty = is_null_scalar(&e.value) && e.value.start == e.value.end;
    let end = if value_empty || e.value.start < colon {
        eol(text, e.key_start)
    } else {
        eol(text, e.value.end.max(e.key_end))
    };
    if text[ls..e.key_start].trim().is_empty() {
        // Lignes de la clé et de sa valeur.
        return Ok(splice(text, ls..after_newline(text, end), ""));
    }
    // Première clé d'un élément de liste (`- a: 1`) : la clé suivante remonte sur la ligne du tiret.
    match entries.get(i + 1) {
        Some(next) if i == 0 => Ok(splice(text, e.key_start..next.key_start, "")),
        _ => Err("clé partageant sa ligne avec une autre construction : retrait par clé non pris en charge (edit_file)".into()),
    }
}

// --- Secrets ---------------------------------------------------------------------------------------

/// Plages des valeurs secrètes (scalaires sous une clé secrète, à tout niveau).
pub(super) fn secret_spans(text: &str) -> Vec<Range<usize>> {
    fn walk(doc: &Doc, n: &Node, secret: bool, out: &mut Vec<Range<usize>>) {
        match &n.kind {
            Kind::Map(entries) => {
                for e in entries {
                    walk(doc, &e.value, secret || super::is_secret_key(&e.key), out);
                }
            }
            Kind::Seq(items) => items.iter().for_each(|i| walk(doc, i, secret, out)),
            Kind::Scalar { .. } if secret && super::maskable(&doc.value(n)) && n.end > n.start => {
                out.push(n.start..n.end)
            }
            _ => {}
        }
    }
    let Ok(doc) = parse(text) else {
        return fallback_spans(text);
    };
    let mut out = Vec::new();
    if let Some(r) = &doc.root {
        walk(&doc, r, false, &mut out);
    }
    out
}

/// YAML illisible pour l'analyseur strict : masquage ligne à ligne (`clé: valeur`).
fn fallback_spans(text: &str) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut pos = 0;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let trimmed = body.trim_start().trim_start_matches("- ");
        if let Some((k, v)) = trimmed.split_once(':') {
            let k = k.trim().trim_matches(['"', '\'']);
            let v_trim = v.trim();
            if super::is_secret_key(k)
                && !v_trim.is_empty()
                && !matches!(v_trim, "true" | "false" | "''" | "\"\"" | "|" | ">")
            {
                let start = pos + body.len() - v.trim_start().len();
                out.push(start..pos + body.len());
            }
        }
        pos += line.len();
    }
    out
}
