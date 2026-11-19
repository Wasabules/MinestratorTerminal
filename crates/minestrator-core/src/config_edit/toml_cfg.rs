//! TOML (`velocity.toml`, configs de mods) : `toml_edit`, qui modifie le document en place en
//! gardant commentaires, ordre et mise en forme. La décoration d'une valeur remplacée (espaces,
//! commentaire de fin de ligne) est reportée sur la nouvelle ; une date TOML reste une date.

use super::Seg;
use serde_json::{Map, Number, Value};
use std::ops::Range;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, Value as TValue};

fn parse_err(e: toml_edit::TomlError) -> String {
    format!("TOML invalide : {}", e.message().trim())
}

// --- Arbre typé ---------------------------------------------------------------------------------

fn value_json(v: &TValue) -> Value {
    match v {
        TValue::String(s) => Value::String(s.value().clone()),
        TValue::Integer(i) => Value::from(*i.value()),
        TValue::Float(f) => Number::from_f64(*f.value())
            .map(Value::Number)
            .unwrap_or_else(|| Value::String(f.value().to_string())),
        TValue::Boolean(b) => Value::Bool(*b.value()),
        TValue::Datetime(d) => Value::String(d.value().to_string()),
        TValue::Array(a) => Value::Array(a.iter().map(value_json).collect()),
        TValue::InlineTable(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), value_json(v)))
                .collect(),
        ),
    }
}

fn item_json(i: &Item) -> Value {
    match i {
        Item::None => Value::Null,
        Item::Value(v) => value_json(v),
        Item::Table(t) => table_json(t),
        Item::ArrayOfTables(a) => Value::Array(a.iter().map(table_json).collect()),
    }
}

fn table_json(t: &Table) -> Value {
    Value::Object(
        t.iter()
            .map(|(k, v)| (k.to_string(), item_json(v)))
            .collect::<Map<_, _>>(),
    )
}

/// Arbre typé du document.
pub(super) fn tree(text: &str) -> Result<Value, String> {
    let doc = toml_edit::Document::parse(text).map_err(parse_err)?;
    Ok(item_json(doc.as_item()))
}

// --- Conversion JSON → TOML ------------------------------------------------------------------------

fn to_value(v: &Value) -> Result<TValue, String> {
    Ok(match v {
        Value::Null => return Err("TOML n'a pas de valeur nulle".into()),
        Value::Bool(b) => TValue::from(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) if !n.is_f64() => TValue::from(i),
            (_, Some(f)) if n.is_f64() => TValue::from(f),
            _ => {
                return Err(format!(
                    "nombre {n} hors des bornes d'un entier TOML (64 bits signé)"
                ))
            }
        },
        Value::String(s) => TValue::from(s.as_str()),
        Value::Array(a) => {
            let mut arr = Array::new();
            for x in a {
                arr.push(to_value(x)?);
            }
            TValue::Array(arr)
        }
        Value::Object(m) => {
            let mut t = InlineTable::new();
            for (k, x) in m {
                t.insert(k, to_value(x)?);
            }
            TValue::InlineTable(t)
        }
    })
}

/// Section `[table]` construite depuis un objet (sous-objets en sous-sections).
fn to_table(m: &Map<String, Value>) -> Result<Table, String> {
    let mut t = Table::new();
    for (k, v) in m {
        let item = match v {
            Value::Object(sub) => Item::Table(to_table(sub)?),
            other => Item::Value(to_value(other)?),
        };
        t.insert(k, item);
    }
    Ok(t)
}

/// Nouvelle valeur à la place de `old` : même décoration, une date reste une date.
fn replacement(old: &TValue, v: &Value) -> Result<TValue, String> {
    let mut new = match (old, v) {
        (TValue::Datetime(_), Value::String(s)) => match s.parse::<toml_edit::Datetime>() {
            Ok(d) => TValue::from(d),
            Err(_) => to_value(v)?,
        },
        _ => to_value(v)?,
    };
    *new.decor_mut() = old.decor().clone();
    Ok(new)
}

// --- Édition --------------------------------------------------------------------------------------

/// Pose `value` au bout de `path`.
pub(super) fn set(
    text: &str,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<String, String> {
    let mut doc: DocumentMut = text.parse().map_err(parse_err)?;
    set_in(doc.as_item_mut(), path, value, create_missing, 0)?;
    Ok(doc.to_string())
}

fn set_in(
    cur: &mut Item,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
    depth: usize,
) -> Result<(), String> {
    if depth > 200 {
        return Err("imbrication trop profonde".into());
    }
    let Some((seg, rest)) = path.split_first() else {
        return Err("chemin vide".into());
    };
    let last = rest.is_empty();
    match seg {
        Seg::Key(k) => {
            // Valeur en ligne (`a = { b = 1 }`) : on descend dans la table en ligne.
            if let Item::Value(TValue::InlineTable(t)) = cur {
                return set_in_inline(t, path, value, create_missing);
            }
            let Some(table) = cur.as_table_mut() else {
                return Err(format!("« {k} » : le parent n'est pas une table"));
            };
            match table.get_mut(k) {
                Some(child) if last => set_leaf(child, value),
                Some(child) => set_in(child, rest, value, create_missing, depth + 1),
                None if !create_missing => Err("clé absente (create_missing=false)".into()),
                None if last => {
                    let item = match value {
                        Value::Object(m) if !m.is_empty() => Item::Table(to_table(m)?),
                        other => Item::Value(to_value(other)?),
                    };
                    table.insert(k, item);
                    Ok(())
                }
                None => {
                    // Section intermédiaire absente : `[a.b]` implicite si la suite est une clé.
                    match rest.first() {
                        Some(Seg::Key(_)) => {
                            let mut t = Table::new();
                            t.set_implicit(true);
                            table.insert(k, Item::Table(t));
                        }
                        _ => {
                            table.insert(k, Item::Value(to_value(&super::nest(rest, value)?)?));
                            return Ok(());
                        }
                    }
                    let child = table.get_mut(k).ok_or("insertion impossible")?;
                    set_in(child, rest, value, create_missing, depth + 1)
                }
            }
        }
        Seg::Index(n) => match cur {
            Item::ArrayOfTables(aot) => set_in_aot(aot, *n, rest, value, create_missing, depth),
            Item::Value(TValue::Array(arr)) => set_in_array(arr, *n, rest, value, create_missing),
            _ => Err(format!("[{n}] : ce n'est pas une liste")),
        },
    }
}

fn set_leaf(child: &mut Item, value: &Value) -> Result<(), String> {
    match (child, value) {
        (Item::Value(old), v) => {
            let new = replacement(old, v)?;
            *old = new;
            Ok(())
        }
        (Item::Table(t), Value::Object(m)) => {
            let mut new = to_table(m)?;
            new.set_position(t.position());
            *new.decor_mut() = t.decor().clone();
            *t = new;
            Ok(())
        }
        (Item::ArrayOfTables(aot), Value::Array(items)) if items.iter().all(Value::is_object) => {
            let mut new = ArrayOfTables::new();
            for it in items {
                if let Value::Object(m) = it {
                    new.push(to_table(m)?);
                }
            }
            *aot = new;
            Ok(())
        }
        (slot, v) => {
            *slot = Item::Value(to_value(v)?);
            Ok(())
        }
    }
}

fn set_in_inline(
    t: &mut InlineTable,
    path: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<(), String> {
    let Some((Seg::Key(k), rest)) = path.split_first() else {
        return Err("clé attendue dans une table en ligne".into());
    };
    match t.get_mut(k) {
        Some(old) if rest.is_empty() => {
            let new = replacement(old, value)?;
            *old = new;
            Ok(())
        }
        Some(TValue::InlineTable(inner)) => set_in_inline(inner, rest, value, create_missing),
        Some(TValue::Array(arr)) => match rest.split_first() {
            Some((Seg::Index(n), tail)) => set_in_array(arr, *n, tail, value, create_missing),
            _ => Err(format!("« {k} » est une liste : index [n] attendu")),
        },
        Some(_) => Err(format!("« {k} » est une valeur, pas une table")),
        None if !create_missing => Err("clé absente (create_missing=false)".into()),
        None => {
            t.insert(k.as_str(), to_value(&super::nest(rest, value)?)?);
            Ok(())
        }
    }
}

fn set_in_array(
    arr: &mut Array,
    n: usize,
    rest: &[Seg],
    value: &Value,
    create_missing: bool,
) -> Result<(), String> {
    if n == arr.len() && create_missing {
        arr.push(to_value(&super::nest(rest, value)?)?);
        return Ok(());
    }
    let len = arr.len();
    let Some(el) = arr.get_mut(n) else {
        return Err(format!("index [{n}] hors de la liste ({len} élément(s))"));
    };
    if rest.is_empty() {
        let new = replacement(el, value)?;
        *el = new;
        return Ok(());
    }
    match (el, rest.first()) {
        (TValue::InlineTable(t), Some(Seg::Key(_))) => {
            set_in_inline(t, rest, value, create_missing)
        }
        (TValue::Array(inner), Some(Seg::Index(m))) => {
            set_in_array(inner, *m, &rest[1..], value, create_missing)
        }
        _ => Err(format!(
            "élément [{n}] : structure différente du chemin demandé"
        )),
    }
}

fn set_in_aot(
    aot: &mut ArrayOfTables,
    n: usize,
    rest: &[Seg],
    value: &Value,
    create_missing: bool,
    depth: usize,
) -> Result<(), String> {
    if n == aot.len() && create_missing {
        let Value::Object(m) = super::nest(rest, value)? else {
            return Err("un élément de `[[liste]]` est une table : objet attendu".into());
        };
        aot.push(to_table(&m)?);
        return Ok(());
    }
    let len = aot.len();
    let Some(t) = aot.get_mut(n) else {
        return Err(format!("index [{n}] hors de la liste ({len} élément(s))"));
    };
    if rest.is_empty() {
        let Value::Object(m) = value else {
            return Err("un élément de `[[liste]]` est une table : objet attendu".into());
        };
        let mut new = to_table(m)?;
        new.set_position(t.position());
        *new.decor_mut() = t.decor().clone();
        *t = new;
        return Ok(());
    }
    let mut item = Item::Table(std::mem::take(t));
    let r = set_in(&mut item, rest, value, create_missing, depth + 1);
    if let Item::Table(back) = item {
        *t = back;
    }
    r
}

// --- Secrets ----------------------------------------------------------------------------------------

/// Plages des valeurs secrètes (positions de l'analyseur).
pub(super) fn secret_spans(text: &str) -> Vec<Range<usize>> {
    fn walk_item(i: &Item, secret: bool, out: &mut Vec<Range<usize>>) {
        match i {
            Item::Value(v) => walk_value(v, secret, out),
            Item::Table(t) => walk_table(t.iter(), secret, out),
            Item::ArrayOfTables(a) => a.iter().for_each(|t| walk_table(t.iter(), secret, out)),
            Item::None => {}
        }
    }
    fn walk_table<'a>(
        it: impl Iterator<Item = (&'a str, &'a Item)>,
        secret: bool,
        out: &mut Vec<Range<usize>>,
    ) {
        for (k, v) in it {
            walk_item(v, secret || super::is_secret_key(k), out);
        }
    }
    fn walk_value(v: &TValue, secret: bool, out: &mut Vec<Range<usize>>) {
        match v {
            TValue::Array(a) => a.iter().for_each(|x| walk_value(x, secret, out)),
            TValue::InlineTable(t) => {
                for (k, x) in t.iter() {
                    walk_value(x, secret || super::is_secret_key(k), out);
                }
            }
            _ if secret && super::maskable(&value_json(v)) => {
                if let Some(r) = v.span() {
                    out.push(r);
                }
            }
            _ => {}
        }
    }
    let Ok(doc) = toml_edit::Document::parse(text) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_item(doc.as_item(), false, &mut out);
    out
}
