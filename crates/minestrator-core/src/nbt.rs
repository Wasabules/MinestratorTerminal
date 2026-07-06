//! Décodage NBT (`.dat` Minecraft : `level.dat`, playerdata, structures `.nbt`/`.schem`, et NBT de
//! chunk d'une région) en **arbre typé** repliable (lecture seule). On décompresse (gzip/zlib/brut),
//! on parse via `fastnbt`, puis on projette dans [`NbtNode`] en **préservant le type NBT** de chaque
//! nœud (Byte ≠ Int ≠ Long, List ≠ IntArray…) — indispensable pour un inspecteur fidèle.

use std::io::Read;

/// Nœud d'un arbre NBT typé, sérialisé vers l'UI (inspecteur repliable).
#[derive(serde::Serialize)]
pub struct NbtNode {
    /// Clé dans le compound parent ; absent pour la racine et les éléments de liste.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Type NBT : `Byte|Short|Int|Long|Float|Double|String|ByteArray|IntArray|LongArray|List|Compound`.
    pub tag: &'static str,
    /// Valeur affichable (scalaires + aperçu des tableaux) ; absent pour List/Compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Nombre d'enfants (List/Compound) ou d'éléments (tableaux).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    /// Sous-nœuds (List/Compound uniquement).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NbtNode>,
}

/// Garde-fou : nombre maximal de nœuds construits (les tableaux restent des feuilles, donc seuls
/// les compounds/listes consomment le budget — largement suffisant pour un .dat ou un chunk).
const NODE_BUDGET: usize = 200_000;
/// Nombre d'éléments de tableau affichés en aperçu (le reste est résumé par « … »).
const ARRAY_PREVIEW: usize = 12;

/// Décode un fichier NBT (compressé gzip/zlib ou brut) en arbre typé.
pub fn to_tree(bytes: &[u8]) -> Result<NbtNode, String> {
    let data = decompress(bytes)?;
    let value: fastnbt::Value =
        fastnbt::from_bytes(&data).map_err(|e| format!("NBT illisible : {e}"))?;
    let mut budget = NODE_BUDGET;
    Ok(build(None, &value, &mut budget))
}

/// Décompresse selon l'en-tête : gzip (`1f 8b`), zlib (`78 …`) ou brut (TAG_Compound `0a`).
fn decompress(bytes: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match bytes {
        [0x1f, 0x8b, ..] => flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map(|_| out)
            .map_err(|e| format!("gunzip NBT : {e}")),
        [0x78, ..] => flate2::read::ZlibDecoder::new(bytes)
            .read_to_end(&mut out)
            .map(|_| out)
            .map_err(|e| format!("inflate NBT : {e}")),
        _ => Ok(bytes.to_vec()),
    }
}

fn leaf(name: Option<String>, tag: &'static str, value: String) -> NbtNode {
    NbtNode { name, tag, value: Some(value), len: None, children: vec![] }
}

/// Feuille « tableau » : type conservé, longueur exacte, aperçu des premiers éléments.
fn array_leaf<T: std::fmt::Display>(
    name: Option<String>,
    tag: &'static str,
    len: usize,
    it: impl Iterator<Item = T>,
) -> NbtNode {
    let preview: Vec<String> = it.take(ARRAY_PREVIEW).map(|v| v.to_string()).collect();
    let mut value = preview.join(", ");
    if len > preview.len() {
        value.push_str(", …");
    }
    NbtNode { name, tag, value: Some(value), len: Some(len), children: vec![] }
}

fn build(name: Option<String>, v: &fastnbt::Value, budget: &mut usize) -> NbtNode {
    use fastnbt::Value as N;
    if *budget == 0 {
        return leaf(name, "…", "(tronqué)".into());
    }
    *budget -= 1;
    match v {
        N::Byte(x) => leaf(name, "Byte", x.to_string()),
        N::Short(x) => leaf(name, "Short", x.to_string()),
        N::Int(x) => leaf(name, "Int", x.to_string()),
        N::Long(x) => leaf(name, "Long", x.to_string()),
        N::Float(x) => leaf(name, "Float", x.to_string()),
        N::Double(x) => leaf(name, "Double", x.to_string()),
        N::String(s) => leaf(name, "String", s.clone()),
        N::ByteArray(a) => array_leaf(name, "ByteArray", a.iter().count(), a.iter().copied()),
        N::IntArray(a) => array_leaf(name, "IntArray", a.iter().count(), a.iter().copied()),
        N::LongArray(a) => array_leaf(name, "LongArray", a.iter().count(), a.iter().copied()),
        N::List(l) => NbtNode {
            name,
            tag: "List",
            value: None,
            len: Some(l.len()),
            children: l.iter().map(|c| build(None, c, budget)).collect(),
        },
        N::Compound(m) => {
            let mut kv: Vec<(&String, &N)> = m.iter().collect();
            kv.sort_by(|a, b| a.0.cmp(b.0)); // ordre stable (HashMap → tri par clé)
            NbtNode {
                name,
                tag: "Compound",
                value: None,
                len: Some(kv.len()),
                children: kv.into_iter().map(|(k, c)| build(Some(k.clone()), c, budget)).collect(),
            }
        }
    }
}

/// Sérialise un fichier NBT en **SNBT** indenté (format des commandes `/data`), fidèle (tableaux
/// complets, non tronqués) — pour la vue SNBT et le copier-coller vers une commande.
pub fn to_snbt(bytes: &[u8]) -> Result<String, String> {
    let data = decompress(bytes)?;
    let value: fastnbt::Value =
        fastnbt::from_bytes(&data).map_err(|e| format!("NBT illisible : {e}"))?;
    let mut out = String::new();
    write_snbt(&mut out, &value, 0);
    Ok(out)
}

fn snbt_indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}

fn write_snbt(out: &mut String, v: &fastnbt::Value, depth: usize) {
    use fastnbt::Value as N;
    use std::fmt::Write as _;
    match v {
        N::Byte(x) => { let _ = write!(out, "{x}b"); }
        N::Short(x) => { let _ = write!(out, "{x}s"); }
        N::Int(x) => { let _ = write!(out, "{x}"); }
        N::Long(x) => { let _ = write!(out, "{x}L"); }
        N::Float(x) => { let _ = write!(out, "{x}f"); }
        N::Double(x) => { let _ = write!(out, "{x}d"); }
        N::String(s) => snbt_string(out, s),
        N::ByteArray(a) => snbt_num_array(out, 'B', a.iter().map(|x| format!("{x}b"))),
        N::IntArray(a) => snbt_num_array(out, 'I', a.iter().map(|x| x.to_string())),
        N::LongArray(a) => snbt_num_array(out, 'L', a.iter().map(|x| format!("{x}L"))),
        N::List(l) => {
            if l.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, e) in l.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                snbt_indent(out, depth + 1);
                write_snbt(out, e, depth + 1);
            }
            out.push('\n');
            snbt_indent(out, depth);
            out.push(']');
        }
        N::Compound(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut kv: Vec<_> = m.iter().collect();
            kv.sort_by(|a, b| a.0.cmp(b.0));
            out.push_str("{\n");
            for (i, (k, val)) in kv.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                snbt_indent(out, depth + 1);
                snbt_key(out, k);
                out.push_str(": ");
                write_snbt(out, val, depth + 1);
            }
            out.push('\n');
            snbt_indent(out, depth);
            out.push('}');
        }
    }
}

/// Tableau typé SNBT en ligne : `[I;1,2,3]`, `[B;1b,2b]`, `[L;1L,2L]`.
fn snbt_num_array(out: &mut String, tag: char, items: impl Iterator<Item = String>) {
    out.push('[');
    out.push(tag);
    out.push(';');
    for (i, s) in items.enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&s);
    }
    out.push(']');
}

/// Clé de compound : nue si simple, sinon entre guillemets.
fn snbt_key(out: &mut String, k: &str) {
    let simple = !k.is_empty()
        && k.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'));
    if simple {
        out.push_str(k);
    } else {
        snbt_string(out, k);
    }
}

fn snbt_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::Write;

    #[derive(serde::Serialize)]
    struct Level {
        #[serde(rename = "LevelName")]
        name: String,
        #[serde(rename = "SpawnX")]
        spawn_x: i32,
        #[serde(rename = "RandomSeed")]
        seed: i64,
    }

    fn sample() -> Vec<u8> {
        fastnbt::to_bytes(&Level { name: "monde".into(), spawn_x: 42, seed: 123456789 }).unwrap()
    }

    #[test]
    fn decodes_and_preserves_scalar_types() {
        let root = super::to_tree(&sample()).unwrap();
        assert_eq!(root.tag, "Compound");
        let find = |k: &str| root.children.iter().find(|n| n.name.as_deref() == Some(k)).unwrap();
        assert_eq!(find("LevelName").tag, "String");
        assert_eq!(find("LevelName").value.as_deref(), Some("monde"));
        assert_eq!(find("SpawnX").tag, "Int"); // ≠ Long, la distinction est conservée
        assert_eq!(find("RandomSeed").tag, "Long");
        assert_eq!(find("RandomSeed").value.as_deref(), Some("123456789"));
    }

    #[test]
    fn preserves_array_type_and_length() {
        let mut c: HashMap<String, fastnbt::Value> = HashMap::new();
        c.insert("flag".into(), fastnbt::Value::Byte(1));
        c.insert("data".into(), fastnbt::Value::IntArray(fastnbt::IntArray::new(vec![1, 2, 3, 4])));
        let bytes = fastnbt::to_bytes(&fastnbt::Value::Compound(c)).unwrap();
        let root = super::to_tree(&bytes).unwrap();
        let find = |k: &str| root.children.iter().find(|n| n.name.as_deref() == Some(k)).unwrap();
        assert_eq!(find("flag").tag, "Byte");
        assert_eq!(find("data").tag, "IntArray");
        assert_eq!(find("data").len, Some(4));
        assert!(find("data").children.is_empty()); // tableau = feuille
    }

    #[test]
    fn decodes_gzip_nbt() {
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&sample()).unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(&gz[..2], &[0x1f, 0x8b]);
        assert_eq!(super::to_tree(&gz).unwrap().tag, "Compound");
    }

    #[test]
    fn rejects_garbage() {
        assert!(super::to_tree(&[9, 9, 9, 9, 9]).is_err());
    }

    #[test]
    fn snbt_preserves_type_suffixes() {
        let snbt = super::to_snbt(&sample()).unwrap();
        assert!(snbt.contains("LevelName: \"monde\""));
        assert!(snbt.contains("RandomSeed: 123456789L")); // suffixe Long
        assert!(snbt.contains("SpawnX: 42")); // Int, sans suffixe
    }
}
