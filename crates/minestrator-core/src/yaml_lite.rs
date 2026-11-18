//! Sous-ensemble de **YAML** suffisant pour les descripteurs de plugins (`plugin.yml`,
//! `paper-plugin.yml`, `bungee.yml`) et les réglages lus par l'audit (`spigot.yml`,
//! `paper-global.yml`) : mappings et séquences en blocs (indentation), listes et mappings en ligne
//! (`[a, b]`, `{a: b}`), scalaires nus ou entre guillemets, blocs `|` et `>`.
//!
//! Ni ancres, ni alias, ni étiquettes, ni documents multiples (seul le premier est lu) : ces
//! fichiers n'en utilisent pas. **Tolérant** : une ligne incomprise est ignorée au lieu de faire
//! échouer tout le fichier, car ce sont des fichiers écrits à la main. Les scalaires restent du
//! texte (`Value::String`) ; `~`, `null` et une valeur vide donnent `Value::Null`. Pur, sans
//! dépendance (pas de crate YAML maintenue qui ne tire pas un analyseur complet).

use serde_json::{Map, Value};

/// Ligne logique : indentation (espaces) et texte sans commentaire ni espaces de fin.
struct Line {
    indent: usize,
    text: String,
}

/// Analyse un document. Un texte vide ou sans clé donne un objet vide.
pub(crate) fn parse(text: &str) -> Value {
    let lines = logical_lines(text);
    let Some(indent) = lines.iter().map(|l| l.indent).min() else {
        return Value::Object(Map::new());
    };
    let mut p = Parser { lines, pos: 0 };
    let mut root = p.block(indent);
    // Lignes moins indentées que la première (fichier mal aligné) : on continue au même niveau.
    while p.pos < p.lines.len() {
        let indent = p.lines[p.pos].indent;
        match (&mut root, p.block(indent)) {
            (Value::Object(a), Value::Object(b)) => a.extend(b),
            _ => break,
        }
    }
    root
}

/// Valeur au bout d'un chemin de clés (`["proxies", "velocity", "enabled"]`).
pub(crate) fn path<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().try_fold(v, |cur, k| cur.get(*k))
}

/// Booléen YAML (`true`/`yes`/`on`, insensible à la casse) ; `None` si absent ou autre chose.
pub(crate) fn as_bool(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "yes" | "on" => Some(true),
            "false" | "no" | "off" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Texte d'un scalaire (nombre accepté) ; `None` si absent, nul ou composé.
pub(crate) fn as_text(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Liste de textes : une séquence, ou un scalaire seul (`depend: Vault`) ; vides retirés.
pub(crate) fn as_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().filter_map(|x| as_text(Some(x))).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
    .into_iter()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect()
}

// --- Découpage en lignes ------------------------------------------------------

fn logical_lines(text: &str) -> Vec<Line> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Vec::new();
    for raw in text.lines() {
        let trimmed_start = raw.trim_start_matches([' ', '\t']);
        if trimmed_start == "---" || trimmed_start.starts_with("--- ") {
            if out.is_empty() {
                continue; // début du premier document
            }
            break; // document suivant : ignoré
        }
        if trimmed_start == "..." {
            break;
        }
        let indent = raw.len() - trimmed_start.len();
        let text = strip_comment(trimmed_start).trim_end().to_string();
        if !text.is_empty() {
            out.push(Line { indent, text });
        }
    }
    out
}

/// Retire un commentaire `#` (en début de texte ou précédé d'un blanc, hors guillemets). Un
/// guillemet n'ouvre une chaîne qu'en début de valeur : l'apostrophe de « Don't » n'en ouvre pas.
fn strip_comment(s: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev: Option<char> = None;
    for (i, c) in s.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => {
                let at_start = prev.is_none_or(|p| p.is_whitespace() || matches!(p, ':' | '[' | '{' | ',' | '-'));
                if matches!(c, '"' | '\'') && at_start {
                    quote = Some(c);
                } else if c == '#' && prev.is_none_or(char::is_whitespace) {
                    return &s[..i];
                }
            }
        }
        prev = Some(c);
    }
    s
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ") || text.starts_with("-\t")
}

// --- Blocs ----------------------------------------------------------------------

struct Parser {
    lines: Vec<Line>,
    pos: usize,
}

impl Parser {
    fn block(&mut self, indent: usize) -> Value {
        match self.lines.get(self.pos) {
            Some(l) if is_seq_item(&l.text) => self.seq(indent),
            Some(_) => self.map(indent),
            None => Value::Null,
        }
    }

    fn map(&mut self, indent: usize) -> Value {
        let mut map = Map::new();
        while let Some(line) = self.lines.get(self.pos) {
            if line.indent < indent {
                break;
            }
            // Ligne plus indentée sans clé parente, ou tiret orphelin : ignorée (tolérance).
            if line.indent > indent || is_seq_item(&line.text) {
                self.pos += 1;
                continue;
            }
            let Some((key, rest)) = split_key(&line.text) else {
                self.pos += 1;
                continue;
            };
            self.pos += 1;
            let value = self.value(&rest, indent);
            map.insert(key, value);
        }
        Value::Object(map)
    }

    fn seq(&mut self, indent: usize) -> Value {
        let mut out = Vec::new();
        while let Some(line) = self.lines.get(self.pos) {
            if line.indent < indent || (line.indent == indent && !is_seq_item(&line.text)) {
                break;
            }
            if line.indent > indent {
                self.pos += 1; // ligne sans élément parent : ignorée
                continue;
            }
            let item = line.text[1..].trim_start().to_string();
            let item_indent = indent + (line.text.len() - item.len());
            if item.is_empty() {
                self.pos += 1;
                let value = match self.lines.get(self.pos) {
                    Some(next) if next.indent > indent => {
                        let i = next.indent;
                        self.block(i)
                    }
                    _ => Value::Null,
                };
                out.push(value);
            } else if is_seq_item(&item) || (!item.starts_with(['[', '{']) && split_key(&item).is_some()) {
                // Élément composé ouvert sur la ligne du tiret (`- name: x`) : la ligne est réécrite
                // à la colonne réelle de l'élément, puis lue comme un bloc.
                self.lines[self.pos] = Line { indent: item_indent, text: item };
                out.push(self.block(item_indent));
            } else {
                self.pos += 1;
                out.push(self.value(&item, indent));
            }
        }
        Value::Array(out)
    }

    /// Valeur qui suit `clé:` (ou un tiret) sur la même ligne, ou le bloc qui s'ouvre dessous.
    fn value(&mut self, rest: &str, indent: usize) -> Value {
        let rest = rest.trim();
        if rest.is_empty() {
            return match self.lines.get(self.pos) {
                Some(next) if next.indent > indent => {
                    let i = next.indent;
                    self.block(i)
                }
                // `clé:` suivie d'une séquence au même niveau (forme courante de YAML).
                Some(next) if next.indent == indent && is_seq_item(&next.text) => self.seq(indent),
                _ => Value::Null,
            };
        }
        if is_block_scalar(rest) {
            return self.block_scalar(rest, indent);
        }
        if rest.starts_with(['[', '{']) {
            let text = self.flow_text(rest);
            return parse_flow(&text);
        }
        scalar(rest)
    }

    /// Bloc `|` (lignes conservées) ou `>` (lignes jointes par un espace).
    fn block_scalar(&mut self, indicator: &str, indent: usize) -> Value {
        let mut parts = Vec::new();
        while let Some(line) = self.lines.get(self.pos) {
            if line.indent <= indent {
                break;
            }
            parts.push(line.text.clone());
            self.pos += 1;
        }
        let sep = if indicator.starts_with('>') { " " } else { "\n" };
        Value::String(parts.join(sep))
    }

    /// Texte complet d'une valeur en ligne, éventuellement répartie sur plusieurs lignes.
    fn flow_text(&mut self, first: &str) -> String {
        let mut text = first.to_string();
        let mut guard = 0;
        while !balanced(&text) && guard < 500 {
            let Some(line) = self.lines.get(self.pos) else { break };
            text.push(' ');
            text.push_str(&line.text);
            self.pos += 1;
            guard += 1;
        }
        text
    }
}

fn is_block_scalar(rest: &str) -> bool {
    let mut chars = rest.chars();
    matches!(chars.next(), Some('|' | '>')) && chars.all(|c| matches!(c, '-' | '+' | '0'..='9'))
}

/// Crochets et accolades équilibrés hors guillemets.
fn balanced(s: &str) -> bool {
    let (mut depth, mut single, mut double) = (0i32, false, false);
    for c in s.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '[' | '{' if !single && !double => depth += 1,
            ']' | '}' if !single && !double => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// Sépare `clé: reste`. La clé peut être entre guillemets ; le `:` doit être suivi d'un blanc ou
/// finir la ligne (une URL `https://…` n'est donc pas une clé).
fn split_key(text: &str) -> Option<(String, String)> {
    if text.starts_with(['"', '\'']) {
        let (key, used) = read_quoted(text)?;
        let after = text[used..].trim_start();
        let rest = after.strip_prefix(':')?;
        if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
            return None;
        }
        return Some((key, rest.trim().to_string()));
    }
    if is_seq_item(text) || text.starts_with(['[', '{', '#', '&', '*', '!', '|', '>']) {
        return None;
    }
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' || b == b'\'' {
            return None; // guillemet avant tout `:` : c'est un scalaire, pas une clé
        }
        if b == b':' {
            let next = bytes.get(i + 1);
            if next.is_none() || matches!(next, Some(b' ' | b'\t')) {
                let key = text[..i].trim_end();
                if key.is_empty() {
                    return None;
                }
                return Some((key.to_string(), text[i + 1..].trim().to_string()));
            }
        }
    }
    None
}

/// Lit une chaîne entre guillemets au début de `s` ; renvoie (contenu, octets consommés).
fn read_quoted(s: &str) -> Option<(String, usize)> {
    let quote = s.chars().next()?;
    let mut out = String::new();
    let mut chars = s.char_indices().skip(1).peekable();
    while let Some((i, c)) = chars.next() {
        if quote == '\'' {
            if c == '\'' {
                if matches!(chars.peek(), Some((_, '\''))) {
                    chars.next();
                    out.push('\'');
                    continue;
                }
                return Some((out, i + 1));
            }
            out.push(c);
        } else {
            match c {
                '"' => return Some((out, i + 1)),
                '\\' => match chars.next() {
                    Some((_, 'n')) => out.push('\n'),
                    Some((_, 't')) => out.push('\t'),
                    Some((_, other)) => out.push(other),
                    None => break,
                },
                _ => out.push(c),
            }
        }
    }
    None
}

/// Scalaire nu ou entre guillemets ; étiquette (`!!str`) et ancre (`&a`) ignorées.
fn scalar(s: &str) -> Value {
    let mut s = s.trim();
    while s.starts_with('!') || s.starts_with('&') {
        s = s.split_once([' ', '\t']).map(|(_, r)| r.trim_start()).unwrap_or("");
    }
    if s.starts_with(['"', '\'']) {
        if let Some((text, _)) = read_quoted(s) {
            return Value::String(text);
        }
    }
    match s {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        _ if s.starts_with('*') => Value::Null, // alias : non interprété
        _ => Value::String(s.to_string()),
    }
}

// --- Valeurs en ligne : [a, b] et {a: b} ----------------------------------------

fn parse_flow(s: &str) -> Value {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    flow_value(&chars, &mut i)
}

fn skip_blank(c: &[char], i: &mut usize) {
    while *i < c.len() && c[*i].is_whitespace() {
        *i += 1;
    }
}

fn flow_value(c: &[char], i: &mut usize) -> Value {
    skip_blank(c, i);
    match c.get(*i) {
        Some('[') => {
            *i += 1;
            let mut out = Vec::new();
            loop {
                skip_blank(c, i);
                match c.get(*i) {
                    None => break,
                    Some(']') => {
                        *i += 1;
                        break;
                    }
                    Some(',') => *i += 1,
                    Some(_) => {
                        let before = *i;
                        out.push(flow_value(c, i));
                        if *i == before {
                            *i += 1; // garde-fou : progresser coûte que coûte
                        }
                    }
                }
            }
            Value::Array(out.into_iter().filter(|v| !v.is_null()).collect())
        }
        Some('{') => {
            *i += 1;
            let mut map = Map::new();
            loop {
                skip_blank(c, i);
                match c.get(*i) {
                    None => break,
                    Some('}') => {
                        *i += 1;
                        break;
                    }
                    Some(',') => *i += 1,
                    Some(_) => {
                        let before = *i;
                        let key = flow_scalar(c, i, true);
                        skip_blank(c, i);
                        let value = if c.get(*i) == Some(&':') {
                            *i += 1;
                            flow_value(c, i)
                        } else {
                            Value::Null
                        };
                        if let Some(k) = as_text(Some(&key)) {
                            map.insert(k, value);
                        }
                        if *i == before {
                            *i += 1;
                        }
                    }
                }
            }
            Value::Object(map)
        }
        Some(_) => flow_scalar(c, i, false),
        None => Value::Null,
    }
}

/// Scalaire dans une valeur en ligne : jusqu'à `,`, `]`, `}` (et `:` pour une clé).
fn flow_scalar(c: &[char], i: &mut usize, key: bool) -> Value {
    skip_blank(c, i);
    if matches!(c.get(*i), Some('"' | '\'')) {
        let rest: String = c[*i..].iter().collect();
        if let Some((text, used)) = read_quoted(&rest) {
            *i += rest[..used].chars().count();
            return Value::String(text);
        }
    }
    let start = *i;
    while *i < c.len() {
        let ch = c[*i];
        if matches!(ch, ',' | ']' | '}') {
            break;
        }
        if key && ch == ':' && c.get(*i + 1).is_none_or(|n| n.is_whitespace() || matches!(n, ',' | '}')) {
            break;
        }
        *i += 1;
    }
    let text: String = c[start..*i].iter().collect();
    scalar(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_real_bukkit_plugin_yml() {
        // Extrait de WorldGuard 7 (forme publiée), commentaires et commandes compris.
        let v = parse(
            "name: WorldGuard\n\
             main: com.sk89q.worldguard.bukkit.WorldGuardPlugin\n\
             version: \"7.0.13+82fdc65\"\n\
             depend: [WorldEdit]\n\
             softdepend: [CommandBook] # optionnel\n\
             api-version: \"1.20\"\n\
             website: https://enginehub.org/worldguard/\n\
             commands:\n  \
               region:\n    \
                 aliases: [rg, regions]\n    \
                 description: Gestion des régions\n",
        );
        assert_eq!(v["name"], "WorldGuard");
        assert_eq!(v["version"], "7.0.13+82fdc65");
        assert_eq!(as_list(v.get("depend")), vec!["WorldEdit"]);
        assert_eq!(as_list(v.get("softdepend")), vec!["CommandBook"]);
        assert_eq!(v["api-version"], "1.20");
        assert_eq!(v["website"], "https://enginehub.org/worldguard/");
        assert_eq!(v["commands"]["region"]["aliases"], json!(["rg", "regions"]));
    }

    #[test]
    fn reads_block_sequences_in_both_indentation_styles() {
        let v = parse("depend:\n  - Vault\n  - ProtocolLib\nsoftdepend:\n- PlaceholderAPI\n- 'Multiverse-Core'\nname: X\n");
        assert_eq!(as_list(v.get("depend")), vec!["Vault", "ProtocolLib"]);
        assert_eq!(as_list(v.get("softdepend")), vec!["PlaceholderAPI", "Multiverse-Core"]);
        assert_eq!(v["name"], "X");
        // Valeur seule au lieu d'une liste.
        assert_eq!(as_list(parse("depend: Vault").get("depend")), vec!["Vault"]);
    }

    #[test]
    fn reads_a_real_paper_plugin_yml() {
        // Forme publiée par Chunky (paper-plugin.yml), dépendances par section.
        let v = parse(
            "name: Chunky\nversion: '1.4.40'\nmain: org.popcraft.chunky.ChunkyBukkit\napi-version: '1.20'\n\
             folia-supported: true\ndependencies:\n  server:\n    WorldBorder:\n      load: BEFORE\n      \
             required: false\n      join-classpath: true\n  bootstrap:\n    Lib:\n      required: true\n",
        );
        assert_eq!(v["version"], "1.4.40");
        assert_eq!(as_bool(v.get("folia-supported")), Some(true));
        assert_eq!(as_bool(path(&v, &["dependencies", "server", "WorldBorder", "required"])), Some(false));
        assert_eq!(as_bool(path(&v, &["dependencies", "bootstrap", "Lib", "required"])), Some(true));
    }

    #[test]
    fn reads_sequences_of_mappings() {
        // Ancien format de paper-plugin.yml (1.19.3) : liste de dépendances.
        let v = parse("dependencies:\n  - name: Vault\n    required: true\n  - name: LuckPerms\n    required: false\n");
        let deps = v["dependencies"].as_array().unwrap();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0]["name"], "Vault");
        assert_eq!(deps[1]["required"], "false");
    }

    #[test]
    fn reads_server_config_sections() {
        // paper-global.yml (extrait réel, 26.1) et spigot.yml.
        let paper = parse(
            "_version: 31\nproxies:\n  bungee-cord:\n    online-mode: true\n  proxy-protocol: false\n  velocity:\n    \
             enabled: false\n    online-mode: true\n    secret: ''\nscoreboards:\n  save-empty-scoreboard-teams: true\n",
        );
        assert_eq!(as_bool(path(&paper, &["proxies", "velocity", "enabled"])), Some(false));
        assert_eq!(path(&paper, &["proxies", "velocity", "secret"]), Some(&json!("")));
        let spigot = parse("config-version: 12\nsettings:\n  debug: false\n  bungeecord: true\n  sample-count: 12\n");
        assert_eq!(as_bool(path(&spigot, &["settings", "bungeecord"])), Some(true));
    }

    #[test]
    fn handles_comments_quotes_block_scalars_and_flow_maps() {
        let v = parse(
            "# en-tête\nname: \"Mon # plugin\"\ndescription: |\n  Ligne 1\n  Ligne 2\nmeta: {a: 1, 'b c': [x, y]}\n\
             empty:\nnull_value: ~\nquoted_key\": bad\n\"key: with colon\": ok\nlist: [\n  A,\n  B\n]\n",
        );
        assert_eq!(v["name"], "Mon # plugin");
        assert_eq!(v["description"], "Ligne 1\nLigne 2");
        assert_eq!(v["meta"]["a"], "1");
        assert_eq!(v["meta"]["b c"], json!(["x", "y"]));
        assert!(v["empty"].is_null() && v["null_value"].is_null());
        assert_eq!(v["key: with colon"], "ok");
        assert_eq!(as_list(v.get("list")), vec!["A", "B"]);
    }

    #[test]
    fn tolerates_garbage_and_odd_indentation() {
        assert_eq!(parse(""), json!({}));
        assert_eq!(parse("\u{feff}---\nname: A\n---\nname: B\n")["name"], "A");
        // Ligne orpheline plus indentée : ignorée, la suite est lue.
        let v = parse("name: A\n   orphan\nversion: 2\n");
        assert_eq!(v["name"], "A");
        assert_eq!(v["version"], "2");
        // Tout le document décalé : lu au niveau de sa première ligne.
        assert_eq!(parse("  name: A\n  version: 2\n")["version"], "2");
        // Lignes sans clé : ignorées, pas d'erreur.
        assert_eq!(parse("juste du texte\nname: Z")["name"], "Z");
    }
}
