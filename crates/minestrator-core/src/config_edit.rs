//! Édition structurée des fichiers de configuration (YAML, TOML, JSON, `.properties`) par chemin
//! de clé, avec vérification de la syntaxe avant écriture.
//!
//! Un agent qui modifie du YAML par rechercher-remplacer casse l'indentation ou les types. Ici, la
//! modification est **ciblée** : la clé est localisée par un analyseur du format, seule sa valeur
//! est réécrite (ou la clé insérée au bon niveau), le reste du fichier — commentaires, ordre,
//! mise en forme — ne bouge pas. Formats (par extension) :
//! - YAML (`.yml`, `.yaml`) : analyseur YAML 1.2 strict à positions (`saphyr-parser`), éditeur
//!   par plages de texte ([`yaml`]) ;
//! - TOML (`.toml`) : `toml_edit`, qui garde commentaires et mise en forme ([`toml_cfg`]) ;
//! - JSON (`.json`, `.json5`, `.jsonc`, `.mcmeta`) : analyseur à positions tolérant les
//!   commentaires et virgules finales des configs de mods ([`json`]) ;
//! - `.properties` : règles de `java.util.Properties` ([`props`]).
//!
//! **Vérification avant écriture** ([`edit_text`]) : le fichier est relu par l'analyseur strict
//! du format, puis l'arbre obtenu est comparé à l'arbre attendu (l'ancien, avec les seules
//! modifications demandées). S'il ne s'analyse plus, si la valeur relue n'est pas celle demandée
//! ou si une autre clé a bougé, rien n'est écrit.
//!
//! **Secrets** : toute valeur sous une clé de type mot de passe / secret / jeton / clé
//! ([`is_secret_key`]) est masquée en `[SECRET]` dans ce qui sort vers le modèle (lecture, ancien
//! et nouvel état, diff des deux côtés). Une valeur qui contient un masque d'anonymisation
//! (`[SECRET]`, `[CREDS]`, `[IP]`, `[EMAIL]`) est refusée : elle écraserait la vraie valeur.

mod json;
mod props;
mod toml_cfg;
mod yaml;
// Retrait de clés (`set_config` avec `remove`, `link_proxy` avec `prune`, annulation clé par clé).
mod remove;
pub(crate) use remove::{edit_text_removing, parse_removals};

use crate::Core;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::ops::Range;

/// Valeur montrée au modèle à la place d'un secret (même masque que `read_properties`).
pub(crate) const SECRET_MASK: &str = "[SECRET]";
/// Marques d'anonymisation (`redact.rs` et masque des secrets) : une valeur qui en contient vient
/// d'une lecture masquée, l'écrire écraserait la vraie valeur.
pub(crate) const MASK_MARKERS: &[&str] = &["[SECRET]", "[CREDS]", "[IP]", "[EMAIL]"];
/// Modifications au plus par appel de `set_config`.
pub(crate) const MAX_CHANGES: usize = 100;
/// Clés au plus d'une édition (config d'un manifeste exporté en entier : 300 valeurs).
const MAX_EDIT_KEYS: usize = 1000;
/// Entrées de l'arborescence renvoyée par `get_config` sans `key`.
const MAX_OUTLINE: usize = 300;
/// Éléments montrés au plus pour une liste de scalaires dans l'arborescence.
const MAX_LIST_PREVIEW: usize = 20;
/// Taille maximale (caractères) d'une valeur renvoyée par `get_config`.
const MAX_VALUE_CHARS: usize = 24_000;
/// Lignes de contexte des diffs.
const DIFF_CONTEXT: usize = 3;

// --- Formats ----------------------------------------------------------------------------------

/// Format d'un fichier de configuration, déduit de son extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    Yaml,
    Toml,
    Json,
    Properties,
}

impl Format {
    /// Format d'après l'extension (insensible à la casse) ; `None` si non pris en charge.
    pub(crate) fn detect(path: &str) -> Option<Self> {
        let name = path.rsplit('/').next().unwrap_or(path);
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
        match ext.as_str() {
            "yml" | "yaml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            "json" | "json5" | "jsonc" | "mcmeta" => Some(Self::Json),
            "properties" => Some(Self::Properties),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Properties => "properties",
        }
    }

    /// Arbre typé du document, par l'analyseur **strict** du format.
    pub(crate) fn tree(self, text: &str) -> Result<Value, String> {
        match self {
            Self::Yaml => yaml::tree(text),
            Self::Toml => toml_cfg::tree(text),
            Self::Json => json::tree(text),
            Self::Properties => Ok(props::tree(text)),
        }
    }

    /// Texte modifié : `path` reçoit `value` (déjà normalisée pour le format).
    fn set(
        self,
        text: &str,
        path: &[Seg],
        value: &Value,
        create_missing: bool,
    ) -> Result<String, String> {
        match self {
            Self::Yaml => yaml::set(text, path, value, create_missing),
            Self::Toml => toml_cfg::set(text, path, value, create_missing),
            Self::Json => json::set(text, path, value, create_missing),
            Self::Properties => props::set(text, path, value, create_missing),
        }
    }

    /// Plages (octets) des valeurs secrètes du texte, triées et disjointes.
    fn secret_spans(self, text: &str) -> Vec<Range<usize>> {
        let mut spans = match self {
            Self::Yaml => yaml::secret_spans(text),
            Self::Toml => toml_cfg::secret_spans(text),
            Self::Json => json::secret_spans(text),
            Self::Properties => props::secret_spans(text),
        };
        spans.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        let mut out: Vec<Range<usize>> = Vec::with_capacity(spans.len());
        for r in spans {
            match out.last_mut() {
                Some(last) if r.start < last.end => last.end = last.end.max(r.end),
                _ => out.push(r),
            }
        }
        out
    }

    /// Chemin de clé : `a.b[0]` ; pour `.properties`, la clé entière telle qu'écrite
    /// (`rcon.password` est UNE clé).
    pub(crate) fn parse_key(self, raw: &str) -> Result<Vec<Seg>, String> {
        match self {
            Self::Properties => {
                let k = raw.trim();
                if k.is_empty() {
                    return Err("clé vide".into());
                }
                Ok(vec![Seg::Key(k.to_string())])
            }
            _ => parse_key_path(raw),
        }
    }

    /// Valeur telle que le format la stocke : tout est texte dans un `.properties`.
    fn normalize(self, value: Value) -> Result<Value, String> {
        match self {
            Self::Properties => match value {
                Value::String(_) => Ok(value),
                Value::Null => Ok(Value::String(String::new())),
                Value::Bool(b) => Ok(Value::String(b.to_string())),
                Value::Number(n) => Ok(Value::String(n.to_string())),
                _ => {
                    Err("un `.properties` ne contient que du texte : liste et objet refusés".into())
                }
            },
            Self::Toml if value.is_null() => {
                Err("TOML n'a pas de valeur nulle : donne une valeur".into())
            }
            _ => Ok(value),
        }
    }
}

// --- Chemins de clés ---------------------------------------------------------------------------

/// Segment d'un chemin de clé.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seg {
    Key(String),
    Index(usize),
}

/// Lit un chemin de clé : segments séparés par `.`, index de liste `[0]`, clé contenant un point
/// entre guillemets (`"lobby.example.com"`, `'a.b'`), entre crochets (`["a.b"]`) ou échappée
/// (`a\.b`).
pub(crate) fn parse_key_path(raw: &str) -> Result<Vec<Seg>, String> {
    let s = raw.trim();
    if s.is_empty() {
        return Err("clé vide".into());
    }
    let chars: Vec<char> = s.chars().collect();
    let mut segs = Vec::new();
    let mut cur = String::new();
    // Le segment courant a-t-il été ouvert (texte, guillemets) ? Distingue `a..b` (erreur) de `a[0].b`.
    let mut open = false;
    let mut i = 0;
    let bad = |m: &str| Err(format!("clé « {s} » illisible : {m}"));
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let Some(&n) = chars.get(i + 1) else {
                    return bad("`\\` final");
                };
                cur.push(n);
                open = true;
                i += 2;
            }
            '"' | '\'' if cur.is_empty() && !open => {
                let (text, next) = read_quoted_seg(&chars, i)
                    .ok_or_else(|| format!("clé « {s} » : guillemet non fermé"))?;
                cur = text;
                open = true;
                i = next;
                if !matches!(chars.get(i), None | Some('.') | Some('[')) {
                    return bad("texte après une clé entre guillemets");
                }
            }
            '.' => {
                if open {
                    segs.push(Seg::Key(std::mem::take(&mut cur)));
                    open = false;
                } else if i == 0 || chars[i - 1] != ']' {
                    return bad("segment vide");
                }
                if i + 1 >= chars.len() {
                    return bad("point final");
                }
                i += 1;
            }
            '[' => {
                if open {
                    segs.push(Seg::Key(std::mem::take(&mut cur)));
                    open = false;
                } else if segs.is_empty() {
                    return bad("index sans liste");
                }
                let close = chars[i..].iter().position(|&c| c == ']').map(|p| p + i);
                let inner: String = match chars.get(i + 1) {
                    Some('"') | Some('\'') => {
                        let (text, next) = read_quoted_seg(&chars, i + 1)
                            .ok_or_else(|| format!("clé « {s} » : guillemet non fermé"))?;
                        if chars.get(next) != Some(&']') {
                            return bad("`]` attendu");
                        }
                        segs.push(Seg::Key(text));
                        i = next + 1;
                        continue;
                    }
                    _ => match close {
                        Some(c) => chars[i + 1..c].iter().collect(),
                        None => return bad("`]` manquant"),
                    },
                };
                let n = inner.trim().parse::<usize>().map_err(|_| {
                    format!("clé « {s} » : index « {inner} » invalide (entier ≥ 0)")
                })?;
                segs.push(Seg::Index(n));
                i = close.unwrap_or(i) + 1;
                if !matches!(chars.get(i), None | Some('.') | Some('[')) {
                    return bad("`.` ou `[` attendu après `]`");
                }
            }
            _ => {
                cur.push(c);
                open = true;
                i += 1;
            }
        }
    }
    if open {
        segs.push(Seg::Key(cur));
    }
    if segs.is_empty() {
        return bad("aucun segment");
    }
    if segs
        .iter()
        .any(|g| matches!(g, Seg::Key(k) if k.is_empty()))
    {
        return bad("segment vide");
    }
    Ok(segs)
}

/// Chaîne entre guillemets à partir de `start` ; renvoie (contenu, position après le guillemet).
fn read_quoted_seg(chars: &[char], start: usize) -> Option<(String, usize)> {
    let q = chars[start];
    let mut out = String::new();
    let mut i = start + 1;
    while i < chars.len() {
        match chars[i] {
            '\\' if q == '"' => {
                out.push(*chars.get(i + 1)?);
                i += 2;
            }
            c if c == q => return Some((out, i + 1)),
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    None
}

/// Forme lisible d'un chemin (inverse de [`parse_key_path`]).
pub(crate) fn render_key_path(segs: &[Seg]) -> String {
    let mut out = String::new();
    for (i, s) in segs.iter().enumerate() {
        match s {
            Seg::Index(n) => {
                let _ = std::fmt::Write::write_fmt(&mut out, format_args!("[{n}]"));
            }
            Seg::Key(k) => {
                if i > 0 {
                    out.push('.');
                }
                let plain = !k.is_empty()
                    && !k.contains(['.', '[', ']', '"', '\'', '\\'])
                    && !k.starts_with(char::is_whitespace)
                    && !k.ends_with(char::is_whitespace);
                if plain {
                    out.push_str(k);
                } else {
                    out.push('"');
                    out.push_str(&k.replace('\\', "\\\\").replace('"', "\\\""));
                    out.push('"');
                }
            }
        }
    }
    out
}

// --- Arbres typés : accès et comparaison --------------------------------------------------------

/// Valeur au bout d'un chemin.
pub(crate) fn get_path<'a>(v: &'a Value, path: &[Seg]) -> Option<&'a Value> {
    path.iter().try_fold(v, |cur, seg| match (seg, cur) {
        (Seg::Key(k), Value::Object(m)) => m.get(k),
        (Seg::Index(n), Value::Array(a)) => a.get(*n),
        _ => None,
    })
}

/// Pose `value` au bout de `path`, en créant les objets intermédiaires (une valeur nulle devient
/// un objet ; un index égal à la longueur ajoute un élément).
pub(crate) fn set_path(root: &mut Value, path: &[Seg], value: Value) -> Result<(), String> {
    let Some((first, rest)) = path.split_first() else {
        *root = value;
        return Ok(());
    };
    match first {
        Seg::Key(k) => {
            if root.is_null() {
                *root = Value::Object(Map::new());
            }
            let Value::Object(m) = root else {
                return Err(format!("« {k} » : le parent n'est pas une section"));
            };
            let child = m.entry(k.clone()).or_insert(Value::Null);
            if rest.is_empty() {
                *child = value;
                Ok(())
            } else {
                set_path(child, rest, value)
            }
        }
        Seg::Index(n) => {
            if root.is_null() {
                *root = Value::Array(Vec::new());
            }
            let Value::Array(a) = root else {
                return Err(format!("[{n}] : le parent n'est pas une liste"));
            };
            if *n == a.len() {
                a.push(Value::Null);
            }
            let len = a.len();
            let child = a
                .get_mut(*n)
                .ok_or_else(|| format!("index [{n}] hors de la liste ({len} éléments)"))?;
            if rest.is_empty() {
                *child = value;
                Ok(())
            } else {
                set_path(child, rest, value)
            }
        }
    }
}

/// Valeur imbriquée pour la partie absente d'un chemin (`b.c` = 1 → `{b: {c: 1}}`).
pub(crate) fn nest(rest: &[Seg], value: &Value) -> Result<Value, String> {
    match rest.split_first() {
        None => Ok(value.clone()),
        Some((Seg::Key(k), tail)) => {
            let mut m = Map::new();
            m.insert(k.clone(), nest(tail, value)?);
            Ok(Value::Object(m))
        }
        Some((Seg::Index(0), tail)) => Ok(Value::Array(vec![nest(tail, value)?])),
        Some((Seg::Index(n), _)) => Err(format!(
            "index [{n}] dans une liste à créer : seul [0] est possible"
        )),
    }
}

/// Premier chemin où deux arbres diffèrent (`None` s'ils sont égaux).
pub(crate) fn first_difference(a: &Value, b: &Value, at: &mut Vec<Seg>) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, va) in x {
                at.push(Seg::Key(k.clone()));
                let d = match y.get(k) {
                    Some(vb) => first_difference(va, vb, at),
                    None => Some(render_key_path(at)),
                };
                at.pop();
                if d.is_some() {
                    return d;
                }
            }
            y.keys().find(|k| !x.contains_key(*k)).map(|k| {
                at.push(Seg::Key(k.clone()));
                let p = render_key_path(at);
                at.pop();
                p
            })
        }
        (Value::Array(x), Value::Array(y)) => {
            for (i, (va, vb)) in x.iter().zip(y).enumerate() {
                at.push(Seg::Index(i));
                let d = first_difference(va, vb, at);
                at.pop();
                if d.is_some() {
                    return d;
                }
            }
            (x.len() != y.len()).then(|| {
                at.push(Seg::Index(x.len().min(y.len())));
                let p = render_key_path(at);
                at.pop();
                p
            })
        }
        _ if values_equal(a, b) => None,
        _ => Some(if at.is_empty() {
            "(racine)".into()
        } else {
            render_key_path(at)
        }),
    }
}

/// Égalité de valeurs, nombres comparés par leur valeur (`5` et `5` entier/flottant non mélangés).
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            match (x.as_i64(), y.as_i64(), x.as_f64(), y.as_f64()) {
                (Some(i), Some(j), _, _) => i == j,
                (_, _, Some(f), Some(g)) => x.is_f64() == y.is_f64() && f == g,
                _ => x == y,
            }
        }
        _ => a == b,
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}

// --- Secrets ------------------------------------------------------------------------------------

/// Sous-chaînes (clé en minuscules, sans séparateurs) qui désignent un secret. Couvre les clés de
/// `redact.rs` et de `is_secret_property`, et les configs de plugins (`discord.bot-token`,
/// `database.password`, `apiKey`, `license-key`, `webhook-url`…).
const SECRET_PARTS: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "apikey",
    "privatekey",
    "accesskey",
    "secretkey",
    "licensekey",
    "authkey",
    "webhook",
    "credential",
    "authorization",
    "bearer",
];
/// Mots entiers (après découpage `a-b`, `a_b`, `aB`) qui désignent un secret : `key` (`api-key`,
/// `server.key`, `encryption-key`), `pass`/`pwd`/`pw` (`db-pass`, `mysql.pwd`).
const SECRET_WORDS: &[&str] = &["key", "pass", "pwd", "pw"];

/// Clé de configuration au contenu secret (insensible à la casse).
pub(crate) fn is_secret_key(key: &str) -> bool {
    if crate::models::is_secret_property(key) {
        return true;
    }
    let compact: String = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase();
    if SECRET_PARTS.iter().any(|p| compact.contains(p)) {
        return true;
    }
    key_words(key)
        .iter()
        .any(|w| SECRET_WORDS.contains(&w.as_str()))
}

/// Mots d'une clé : séparés par tout caractère non alphanumérique et aux passages
/// minuscule → majuscule (`apiKey` → `api`, `key`).
fn key_words(key: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in key.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                words.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            words.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
}

/// Le chemin désigne-t-il un secret (une de ses clés, à n'importe quel niveau) ?
pub(crate) fn path_is_secret(path: &[Seg]) -> bool {
    path.iter()
        .any(|s| matches!(s, Seg::Key(k) if is_secret_key(k)))
}

/// Une valeur sous une clé secrète est-elle masquée ? Un booléen, un nul ou un texte vide ne sont
/// pas des secrets (et dire « vide » est utile : mot de passe RCON non défini…).
pub(crate) fn maskable(v: &Value) -> bool {
    match v {
        Value::String(s) => !s.is_empty(),
        Value::Number(_) => true,
        _ => false,
    }
}

/// Copie masquée d'une valeur ; `secret` : un parent est déjà une clé secrète. Renvoie le nombre
/// de valeurs masquées.
pub(crate) fn mask_value(v: &Value, secret: bool) -> (Value, usize) {
    match v {
        Value::Object(m) => {
            let mut n = 0;
            let out = m
                .iter()
                .map(|(k, x)| {
                    let (y, c) = mask_value(x, secret || is_secret_key(k));
                    n += c;
                    (k.clone(), y)
                })
                .collect();
            (Value::Object(out), n)
        }
        Value::Array(a) => {
            let mut n = 0;
            let out = a
                .iter()
                .map(|x| {
                    let (y, c) = mask_value(x, secret);
                    n += c;
                    y
                })
                .collect();
            (Value::Array(out), n)
        }
        _ if secret && maskable(v) => (Value::String(SECRET_MASK.into()), 1),
        _ => (v.clone(), 0),
    }
}

/// Texte où chaque plage secrète est remplacée par le masque.
pub(crate) fn mask_text(text: &str, spans: &[Range<usize>]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for r in spans {
        if r.start < pos
            || r.end > text.len()
            || !text.is_char_boundary(r.start)
            || !text.is_char_boundary(r.end)
        {
            continue;
        }
        out.push_str(&text[pos..r.start]);
        out.push_str(SECRET_MASK);
        pos = r.end;
    }
    out.push_str(&text[pos..]);
    out
}

/// Le texte d'un fichier, secrets masqués (diffs, contenus montrés au modèle).
pub(crate) fn masked_text(format: Format, text: &str) -> String {
    let (bom, body) = split_bom(text);
    format!("{bom}{}", mask_text(body, &format.secret_spans(body)))
}

/// Anonymisation IA (`redact.rs`) appliquée à chaque texte d'une valeur JSON. Passer le JSON
/// sérialisé entier par le filtre ligne à ligne le corromprait (`"secret": "",` deviendrait
/// `"secret": [SECRET]`, guillemets et virgule perdus) : un manifeste exporté doit rester
/// réutilisable tel quel.
pub(crate) fn redact_value(v: Value) -> Value {
    match v {
        Value::String(s) => Value::String(crate::redact::redact(&s)),
        Value::Array(a) => Value::Array(a.into_iter().map(redact_value).collect()),
        Value::Object(m) => {
            Value::Object(m.into_iter().map(|(k, x)| (k, redact_value(x))).collect())
        }
        other => other,
    }
}

/// Sortie JSON d'un outil pour un agent IA : anonymisée valeur par valeur si le réglage
/// « anonymiser pour l'IA » est actif, toujours valide.
pub(crate) fn render_for_ai(core: &Core, v: Value) -> String {
    let v = if core.get_privacy_config().redact_ai {
        redact_value(v)
    } else {
        v
    };
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Refuse une valeur qui contient un masque d'anonymisation.
pub(crate) fn find_mask_marker(v: &Value) -> Option<&'static str> {
    match v {
        Value::String(s) => MASK_MARKERS.iter().copied().find(|m| s.contains(m)),
        Value::Array(a) => a.iter().find_map(find_mask_marker),
        Value::Object(m) => m.iter().find_map(|(k, x)| {
            MASK_MARKERS
                .iter()
                .copied()
                .find(|mk| k.contains(mk))
                .or_else(|| find_mask_marker(x))
        }),
        _ => None,
    }
}

// --- Moteur d'édition vérifiée -------------------------------------------------------------------

/// Une modification demandée.
#[derive(Debug, Clone)]
pub(crate) struct EditRequest {
    pub key: String,
    pub value: Value,
}

/// Options d'une édition.
#[derive(Debug, Clone, Copy)]
pub(crate) struct EditOptions {
    /// Créer les clés absentes (défaut : oui).
    pub create_missing: bool,
    /// Garder le type d'un nombre ou d'un booléen existant quand la nouvelle valeur est un texte
    /// qui s'y convertit sans perte (`"10"` sur un nombre → `10`, `"false"` sur un booléen).
    pub keep_type: bool,
    /// Manifeste : une valeur masquée (`[SECRET]`…) veut dire « ne pas toucher » au lieu d'une erreur.
    pub skip_masked: bool,
}

impl Default for EditOptions {
    fn default() -> Self {
        Self {
            create_missing: true,
            keep_type: true,
            skip_masked: false,
        }
    }
}

/// Compte rendu d'une modification (valeurs masquées si secrètes).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChangeReport {
    pub key: String,
    /// `modifiée`, `créée`, `inchangée` ou `ignorée`.
    pub action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old: Option<Value>,
    pub new: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Résultat d'une édition vérifiée (rien n'est encore écrit).
#[derive(Debug, Clone)]
pub(crate) struct EditOutcome {
    pub format: Format,
    pub new_text: String,
    pub changes: Vec<ChangeReport>,
    /// Diff unifié, secrets masqués des deux côtés.
    pub diff: String,
    /// Le texte change-t-il ?
    pub changed: bool,
    pub warnings: Vec<String>,
}

/// BOM UTF-8 éventuel et reste du texte.
fn split_bom(text: &str) -> (&str, &str) {
    match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    }
}

/// Convertit `new` vers le type de `old` quand c'est sans perte (voir [`EditOptions::keep_type`]).
/// Renvoie la valeur et la note à afficher.
fn coerce(old: Option<&Value>, new: Value) -> (Value, Option<String>) {
    let Some(old) = old else { return (new, None) };
    match (old, &new) {
        (Value::Number(o), Value::String(s)) => {
            let t = s.trim();
            if o.is_f64() {
                if let Some(n) = t
                    .parse::<f64>()
                    .ok()
                    .filter(|f| f.is_finite())
                    .and_then(serde_json::Number::from_f64)
                {
                    return (Value::Number(n), Some("type conservé : nombre".into()));
                }
            } else if let Ok(i) = t.parse::<i64>() {
                return (Value::from(i), Some("type conservé : nombre entier".into()));
            }
            (
                new,
                Some("attention : la valeur était un nombre, elle devient un texte".into()),
            )
        }
        (Value::Bool(_), Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => (Value::Bool(true), Some("type conservé : booléen".into())),
            "false" => (Value::Bool(false), Some("type conservé : booléen".into())),
            _ => (
                new,
                Some("attention : la valeur était un booléen, elle devient un texte".into()),
            ),
        },
        // Un nombre ou un booléen JSON envoyé explicitement est gardé tel quel, même sur un texte :
        // `view-distance: default` (spigot.yml) attend bien un nombre à la place de `default`.
        (Value::String(_), Value::Number(_) | Value::Bool(_)) => (
            new,
            Some("type changé : texte → nombre ou booléen, comme demandé".into()),
        ),
        (Value::Number(o), Value::Number(n)) if o.is_f64() && !n.is_f64() => {
            match n.as_f64().and_then(serde_json::Number::from_f64) {
                Some(f) => (
                    Value::Number(f),
                    Some("type conservé : nombre décimal".into()),
                ),
                None => (new, None),
            }
        }
        _ => (new, None),
    }
}

/// Applique des modifications à un texte, puis **vérifie** le résultat avec l'analyseur strict du
/// format : il doit s'analyser, contenir les valeurs demandées, et rien d'autre ne doit avoir
/// changé. `label` sert aux en-têtes du diff ; `redact` filtre chaque ligne du diff (anonymisation
/// IA). Erreur = rien ne doit être écrit.
pub(crate) fn edit_text(
    format: Format,
    label: &str,
    old_text: &str,
    requests: &[EditRequest],
    opts: EditOptions,
    redact: &dyn Fn(&str) -> String,
) -> Result<EditOutcome, String> {
    if requests.is_empty() {
        return Err("aucune modification demandée (`changes` vide)".into());
    }
    if requests.len() > MAX_EDIT_KEYS {
        return Err(format!(
            "{} modifications : {MAX_EDIT_KEYS} au plus par fichier",
            requests.len()
        ));
    }
    let (bom, body) = split_bom(old_text);
    let old_tree = format
        .tree(body)
        .map_err(|e| format!("le fichier actuel n'est pas lisible en {} ({e}) : corrige-le d'abord (edit_file) ; rien n'a été modifié", format.label()))?;

    let mut text = body.to_string();
    let mut expected = old_tree.clone();
    let mut reports = Vec::new();
    let mut warnings = Vec::new();
    for req in requests {
        let path = format.parse_key(&req.key)?;
        let shown = if format == Format::Properties {
            req.key.trim().to_string()
        } else {
            render_key_path(&path)
        };
        let secret = path_is_secret(&path);
        if let Some(marker) = find_mask_marker(&req.value) {
            if opts.skip_masked {
                reports.push(ChangeReport {
                    key: shown,
                    action: "ignorée",
                    old: None,
                    new: Value::String(marker.into()),
                    note: Some(
                        "valeur masquée à la lecture : la valeur actuelle est gardée".into(),
                    ),
                });
                continue;
            }
            return Err(format!(
                "« {shown} » : la valeur contient {marker}, un masque d'anonymisation (la vraie valeur n'est pas connue) ; la réécrire l'écraserait. Donne la vraie valeur, ou ne modifie pas cette clé."
            ));
        }
        let current = get_path(&expected, &path).cloned();
        let value = format
            .normalize(req.value.clone())
            .map_err(|e| format!("« {shown} » : {e}"))?;
        let (value, note) = if opts.keep_type && format != Format::Properties {
            coerce(current.as_ref(), value)
        } else {
            (value, None)
        };
        if current.is_none() && !opts.create_missing {
            return Err(format!(
                "« {shown} » est absente du fichier (create_missing=false) : rien n'a été modifié"
            ));
        }
        let mask = |v: &Value| {
            if secret {
                mask_value(v, true).0
            } else {
                mask_value(v, false).0
            }
        };
        if current
            .as_ref()
            .is_some_and(|c| first_difference(c, &value, &mut Vec::new()).is_none())
        {
            reports.push(ChangeReport {
                key: shown,
                action: "inchangée",
                old: None,
                new: mask(&value),
                note: None,
            });
            continue;
        }
        text = format
            .set(&text, &path, &value, opts.create_missing)
            .map_err(|e| format!("« {shown} » : {e}"))?;
        set_path(&mut expected, &path, value.clone()).map_err(|e| format!("« {shown} » : {e}"))?;
        if let Some(n) = &note {
            if n.starts_with("attention") || n.starts_with("type changé") {
                warnings.push(format!("« {shown} » : {n}"));
            }
        }
        reports.push(ChangeReport {
            key: shown,
            action: if current.is_some() {
                "modifiée"
            } else {
                "créée"
            },
            old: current.as_ref().map(mask),
            new: mask(&value),
            note: if secret {
                Some(note.map_or("valeur secrète (masquée)".into(), |n| {
                    format!("{n} ; valeur secrète (masquée)")
                }))
            } else {
                note
            },
        });
    }

    // Relecture stricte : le résultat doit s'analyser et donner exactement l'arbre attendu.
    let new_tree = format
        .tree(&text)
        .map_err(|e| format!("modification refusée, rien n'a été écrit : le résultat ne s'analyserait plus en {} ({e})", format.label()))?;
    if let Some(at) = first_difference(&expected, &new_tree, &mut Vec::new()) {
        return Err(format!(
            "modification refusée, rien n'a été écrit : après édition, « {at} » ne vaudrait pas ce qui est attendu (structure inhabituelle à cet endroit ; utilise edit_file pour ce cas)"
        ));
    }

    let new_text = format!("{bom}{text}");
    let changed = new_text != old_text;
    let diff = if changed {
        let a = masked_text(format, old_text);
        let b = masked_text(format, &new_text);
        crate::fleet::unified_diff(
            &a,
            &b,
            &format!("a{label}"),
            &format!("b{label}"),
            DIFF_CONTEXT,
            redact,
        )
        .0
    } else {
        String::new()
    };
    Ok(EditOutcome {
        format,
        new_text,
        changes: reports,
        diff,
        changed,
        warnings,
    })
}

// --- Lecture : valeur ou arborescence ------------------------------------------------------------

/// Arborescence aplatie d'un arbre (`get_config` sans `key`) : une entrée par valeur, listes de
/// scalaires d'un bloc, secrets masqués. Plafonnée à [`MAX_OUTLINE`] entrées.
pub(crate) fn outline(tree: &Value) -> (Vec<Value>, bool, usize) {
    fn walk(
        v: &Value,
        at: &mut Vec<Seg>,
        secret: bool,
        out: &mut Vec<Value>,
        masked: &mut usize,
    ) -> bool {
        if out.len() >= MAX_OUTLINE {
            return true;
        }
        let key = || {
            if at.is_empty() {
                "(racine)".to_string()
            } else {
                render_key_path(at)
            }
        };
        match v {
            Value::Object(m) if !m.is_empty() => {
                for (k, x) in m {
                    at.push(Seg::Key(k.clone()));
                    let cut = walk(x, at, secret || is_secret_key(k), out, masked);
                    at.pop();
                    if cut {
                        return true;
                    }
                }
                false
            }
            Value::Array(a) if a.iter().any(|x| x.is_object() || x.is_array()) => {
                for (i, x) in a.iter().enumerate() {
                    at.push(Seg::Index(i));
                    let cut = walk(x, at, secret, out, masked);
                    at.pop();
                    if cut {
                        return true;
                    }
                }
                false
            }
            Value::Array(a) => {
                let (shown, n) = mask_value(
                    &Value::Array(a.iter().take(MAX_LIST_PREVIEW).cloned().collect()),
                    secret,
                );
                *masked += n;
                let mut e = json!({ "key": key(), "type": "list", "value": shown });
                if a.len() > MAX_LIST_PREVIEW {
                    e["items"] = json!(a.len());
                }
                out.push(e);
                false
            }
            _ => {
                let (shown, n) = mask_value(v, secret);
                *masked += n;
                out.push(json!({ "key": key(), "type": type_name(v), "value": shown }));
                false
            }
        }
    }
    let mut out = Vec::new();
    let mut masked = 0;
    let cut = walk(tree, &mut Vec::new(), false, &mut out, &mut masked);
    (out, cut, masked)
}

/// Clés existantes au niveau le plus profond atteint d'un chemin introuvable (aide l'agent).
fn nearest_keys(tree: &Value, path: &[Seg]) -> (String, Vec<String>) {
    let mut cur = tree;
    let mut reached = Vec::new();
    for seg in path {
        match get_path(cur, std::slice::from_ref(seg)) {
            Some(next) => {
                reached.push(seg.clone());
                cur = next;
            }
            None => break,
        }
    }
    let keys = match cur {
        Value::Object(m) => m.keys().take(60).cloned().collect(),
        Value::Array(a) => vec![format!(
            "liste de {} élément(s) : index [0] à [{}]",
            a.len(),
            a.len().saturating_sub(1)
        )],
        _ => Vec::new(),
    };
    (
        if reached.is_empty() {
            "(racine)".into()
        } else {
            render_key_path(&reached)
        },
        keys,
    )
}

/// Rendu de `get_config` (pur) : `text` = contenu du fichier distant.
pub(crate) fn render_get(path: &str, text: &str, key: Option<&str>) -> Result<Value, String> {
    let format = Format::detect(path).ok_or_else(|| unsupported_format(path))?;
    let (_, body) = split_bom(text);
    let mut warnings = Vec::new();
    let tree = match format.tree(body) {
        Ok(t) => t,
        // YAML écrit à la main que l'analyseur strict refuse : lecture tolérante, sans édition.
        Err(e) if format == Format::Yaml => {
            warnings.push(format!("YAML invalide pour un analyseur strict ({e}) : lecture tolérante (valeurs en texte) ; set_config refusera de l'éditer tant qu'il n'est pas corrigé."));
            crate::yaml_lite::parse(body)
        }
        Err(e) => {
            return Err(format!(
                "« {path} » n'est pas lisible en {} : {e}",
                format.label()
            ))
        }
    };
    let Some(key) = key.map(str::trim).filter(|k| !k.is_empty()) else {
        let (entries, truncated, masked) = outline(&tree);
        let mut out = json!({
            "path": path,
            "format": format.label(),
            "keys": entries,
            "truncated": truncated,
            "secrets_masked": masked,
        });
        if truncated {
            out["note"] = json!(format!("arborescence limitée à {MAX_OUTLINE} valeurs : précise `key` pour descendre dans une section"));
        }
        if !warnings.is_empty() {
            out["warnings"] = json!(warnings);
        }
        return Ok(out);
    };
    let segs = format.parse_key(key)?;
    let shown = if format == Format::Properties {
        key.to_string()
    } else {
        render_key_path(&segs)
    };
    let mut out = json!({ "path": path, "format": format.label(), "key": shown });
    match get_path(&tree, &segs) {
        Some(v) => {
            let (masked, n) = mask_value(v, path_is_secret(&segs));
            let rendered = serde_json::to_string(&masked).unwrap_or_default();
            out["found"] = json!(true);
            out["type"] = json!(type_name(v));
            if rendered.chars().count() > MAX_VALUE_CHARS {
                let (entries, truncated, _) = outline(&masked);
                out["value_outline"] = json!(entries);
                out["truncated"] = json!(truncated);
                out["note"] = json!("valeur trop longue : arborescence de la section, précise une clé plus profonde");
            } else {
                out["value"] = masked;
            }
            if n > 0 {
                out["secrets_masked"] = json!(n);
            }
        }
        None => {
            let (at, keys) = nearest_keys(&tree, &segs);
            out["found"] = json!(false);
            out["deepest_existing"] = json!(at);
            out["available_keys"] = json!(keys);
        }
    }
    if !warnings.is_empty() {
        out["warnings"] = json!(warnings);
    }
    Ok(out)
}

fn unsupported_format(path: &str) -> String {
    format!("format de « {path} » non pris en charge : .yml/.yaml, .toml, .json (.json5, .jsonc, .mcmeta) ou .properties ; sinon edit_file")
}

/// Rendu JSON d'une édition (outil `set_config`, étapes de manifeste).
pub(crate) fn render_outcome(path: &str, o: &EditOutcome, dry_run: bool, written: bool) -> Value {
    let mut out = json!({
        "path": path,
        "format": o.format.label(),
        "dry_run": dry_run,
        "written": written,
        "changes": o.changes,
        "diff": if o.changed { Value::String(o.diff.clone()) } else { Value::Null },
    });
    if !o.changed {
        out["note"] = json!("déjà conforme : rien à écrire");
    } else if dry_run {
        out["note"] =
            json!("simulation : rien n'a été écrit (relance sans dry_run pour appliquer)");
    } else {
        out["note"] = json!("fichier vérifié puis écrit ; effet au prochain rechargement du plugin ou redémarrage du serveur");
    }
    if !o.warnings.is_empty() {
        out["warnings"] = json!(o.warnings);
    }
    out
}

// --- Arguments des outils -------------------------------------------------------------------------

/// Schéma JSON du paramètre `changes` de `set_config`.
pub(crate) fn changes_schema() -> Value {
    json!({
        "type": "array",
        "description": "Modifications : [{ key, value }] ; `value` typée (nombre, booléen, texte, liste, objet ; null : valeur nulle, hors TOML)",
        "items": {
            "type": "object",
            "properties": {
                "key": { "type": "string", "description": "Chemin de clé : a.b.c, a.list[0], a.\"clé.à.points\" ; .properties : clé entière" },
                "value": { "description": "Nouvelle valeur (JSON)" }
            },
            "required": ["key", "value"]
        }
    })
}

/// Modifications passées à `set_config` : tableau `[{ key, value }]`, ou objet `{ clé: valeur }`.
pub(crate) fn parse_changes(v: Option<&Value>) -> Result<Vec<EditRequest>, String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let key = it
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("`changes[{i}].key` (texte) requis"))?;
                let value = it
                    .get("value")
                    .cloned()
                    .ok_or_else(|| format!("`changes[{i}].value` requis (null accepté)"))?;
                Ok(EditRequest {
                    key: key.to_string(),
                    value,
                })
            })
            .collect(),
        Some(Value::Object(m)) => Ok(m
            .iter()
            .map(|(k, v)| EditRequest {
                key: k.clone(),
                value: v.clone(),
            })
            .collect()),
        _ => Err("paramètre `changes` requis : tableau [{ key, value }]".into()),
    }
}

// --- Façade Core ----------------------------------------------------------------------------------

impl Core {
    /// Lit un fichier de configuration distant (SFTP) : chemin normalisé, format, texte.
    pub(crate) async fn config_read(
        &self,
        server_id: i64,
        path: &str,
    ) -> crate::Result<(String, Format, String)> {
        let path = crate::fleet::normalize_path(path)?;
        let format = Format::detect(&path)
            .ok_or_else(|| crate::Error::Invalid(unsupported_format(&path)))?;
        let p = path.clone();
        let text = self
            .with_sftp(server_id, |conn| {
                let p = p.clone();
                async move { crate::sftp::read_text(&conn, &p).await }
            })
            .await?;
        Ok((path, format, text))
    }

    /// Édite un fichier de configuration distant : lecture, modifications vérifiées, écriture
    /// (sauf `dry_run` ou rien à changer). Erreur = rien n'est écrit.
    pub(crate) async fn config_edit(
        &self,
        server_id: i64,
        path: &str,
        requests: &[EditRequest],
        opts: EditOptions,
        dry_run: bool,
    ) -> Result<(String, EditOutcome, bool), String> {
        let (path, format, text) = self
            .config_read(server_id, path)
            .await
            .map_err(|e| e.to_string())?;
        let redact = |s: &str| self.redact_ai(s);
        let outcome = edit_text(format, &path, &text, requests, opts, &redact)?;
        if dry_run || !outcome.changed {
            return Ok((path, outcome, false));
        }
        let body = outcome.new_text.clone();
        let p = path.clone();
        self.with_sftp(server_id, |conn| {
            let (p, body) = (p.clone(), body.clone());
            async move { crate::sftp::write_text(&conn, &p, &body).await }
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok((path, outcome, true))
    }
}

/// Outil MCP `get_config`.
pub(crate) async fn get_config_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = args
        .get("server_id")
        .and_then(Value::as_i64)
        .ok_or("paramètre `server_id` (entier) requis")?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("paramètre `path` (texte) requis")?;
    let key = args.get("key").and_then(Value::as_str);
    let (path, _, text) = core
        .config_read(id, path)
        .await
        .map_err(|e| e.to_string())?;
    let out = render_get(&path, &text, key)?;
    Ok(render_for_ai(core, out))
}

/// Outil MCP `set_config`.
pub(crate) async fn set_config_tool(core: &Core, args: &Value) -> Result<String, String> {
    let id = args
        .get("server_id")
        .and_then(Value::as_i64)
        .ok_or("paramètre `server_id` (entier) requis")?;
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("paramètre `path` (texte) requis")?;
    // `remove` : clés à retirer ; `changes` devient alors facultatif.
    let removals = parse_removals(args.get("remove"))?;
    let requests = match args.get("changes") {
        None | Some(Value::Null) if !removals.is_empty() => Vec::new(),
        v => parse_changes(v)?,
    };
    if requests.len() + removals.len() > MAX_CHANGES {
        return Err(format!(
            "{} modifications : {MAX_CHANGES} au plus par appel (découpe en plusieurs appels)",
            requests.len() + removals.len()
        ));
    }
    let flag = |k: &str, d: bool| args.get(k).and_then(Value::as_bool).unwrap_or(d);
    let opts = EditOptions {
        create_missing: flag("create_missing", true),
        keep_type: flag("keep_type", true),
        skip_masked: false,
    };
    let dry_run = flag("dry_run", false);
    let (path, outcome, written) = if removals.is_empty() {
        core.config_edit(id, path, &requests, opts, dry_run).await?
    } else {
        core.config_edit_removing(id, path, &requests, &removals, opts, dry_run)
            .await?
    };
    let out = render_outcome(&path, &outcome, dry_run, written);
    Ok(render_for_ai(core, out))
}

#[cfg(test)]
mod tests;
