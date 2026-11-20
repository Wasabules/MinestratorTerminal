//! **Retrait de clés** d'une configuration : `set_config` avec `remove`, `link_proxy` avec
//! `prune`, annulation clé par clé de la garde des agents (clé créée par l'action).
//!
//! Même exigence que [`super::edit_text`] : chaque clé est retirée du texte par l'éditeur de son
//! format (lignes de la clé et de sa valeur en YAML, entrée de table en TOML, membre et sa virgule
//! en JSON, ligne logique en `.properties`), puis le fichier est **relu par l'analyseur strict** :
//! il doit donner l'arbre d'avant privé des seules clés retirées, sinon rien n'est écrit. Les
//! modifications demandées en même temps (`changes`) sont appliquées ensuite et vérifiées par
//! `edit_text`. Une clé déjà absente est signalée (`absente`), sans erreur : un retrait est
//! idempotent. Seules les clés de section se retirent (pas un élément de liste par son index :
//! la liste se réécrit en entier).

use super::*;

impl Format {
    /// Texte sans la clé `path` (doit exister ; vérifié ensuite par l'appelant).
    fn remove(self, text: &str, path: &[Seg]) -> Result<String, String> {
        match self {
            Self::Yaml => yaml::remove(text, path),
            Self::Toml => toml_cfg::remove(text, path),
            Self::Json => json::remove(text, path),
            Self::Properties => props::remove(text, path),
        }
    }
}

/// Retire `path` d'un arbre ; `true` s'il y était.
pub(crate) fn remove_path(root: &mut Value, path: &[Seg]) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut cur = root;
    for seg in parents {
        let next = match (seg, cur) {
            (Seg::Key(k), Value::Object(m)) => m.get_mut(k),
            (Seg::Index(n), Value::Array(a)) => a.get_mut(*n),
            _ => None,
        };
        match next {
            Some(n) => cur = n,
            None => return false,
        }
    }
    match (last, cur) {
        (Seg::Key(k), Value::Object(m)) => m.remove(k).is_some(),
        _ => false,
    }
}

/// Deux chemins dont l'un contient l'autre (ou égaux).
fn overlaps(a: &[Seg], b: &[Seg]) -> bool {
    let n = a.len().min(b.len());
    a[..n] == b[..n]
}

/// Clés à retirer passées à `set_config` (`remove`) : liste de chemins de clés.
pub(crate) fn parse_removals(v: Option<&Value>) -> Result<Vec<String>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                it.as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| format!("`remove[{i}]` : chemin de clé (texte) attendu"))
            })
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => Ok(vec![s.trim().to_string()]),
        Some(_) => Err("`remove` : liste de chemins de clés attendue".into()),
    }
}

/// Retire des clés puis applique des modifications, avec la vérification stricte de
/// [`edit_text`]. Sans retrait : `edit_text` tel quel. Erreur = rien ne doit être écrit.
pub(crate) fn edit_text_removing(
    format: Format,
    label: &str,
    old_text: &str,
    requests: &[EditRequest],
    removals: &[String],
    opts: EditOptions,
    redact: &dyn Fn(&str) -> String,
) -> Result<EditOutcome, String> {
    if removals.is_empty() {
        return edit_text(format, label, old_text, requests, opts, redact);
    }
    if requests.len() + removals.len() > MAX_EDIT_KEYS {
        return Err(format!(
            "{} modifications et retraits : {MAX_EDIT_KEYS} au plus par fichier",
            requests.len() + removals.len()
        ));
    }
    let (bom, body) = split_bom(old_text);
    let old_tree = format
        .tree(body)
        .map_err(|e| format!("le fichier actuel n'est pas lisible en {} ({e}) : corrige-le d'abord (edit_file) ; rien n'a été modifié", format.label()))?;
    let shown_of = |raw: &str, path: &[Seg]| {
        if format == Format::Properties {
            raw.trim().to_string()
        } else {
            render_key_path(path)
        }
    };
    let mut paths: Vec<(String, Vec<Seg>)> = Vec::new();
    for raw in removals {
        let path = format.parse_key(raw)?;
        let shown = shown_of(raw, &path);
        if !matches!(path.last(), Some(Seg::Key(_))) {
            return Err(format!("« {shown} » : retrait d'un élément de liste par index non pris en charge (réécris la liste entière)"));
        }
        if paths.iter().any(|(_, p)| overlaps(p, &path)) {
            return Err(format!(
                "« {shown} » : retrait demandé deux fois (ou dans une clé déjà retirée)"
            ));
        }
        paths.push((shown, path));
    }
    for req in requests {
        let path = format.parse_key(&req.key)?;
        if let Some((shown, _)) = paths.iter().find(|(_, p)| overlaps(p, &path)) {
            return Err(format!(
                "« {shown} » est à la fois retirée et modifiée (« {} ») : choisis l'un ou l'autre",
                req.key.trim()
            ));
        }
    }

    let mut text = body.to_string();
    let mut expected = old_tree;
    let mut reports = Vec::new();
    for (shown, path) in &paths {
        let secret = path_is_secret(path);
        let Some(current) = get_path(&expected, path).cloned() else {
            reports.push(ChangeReport {
                key: shown.clone(),
                action: "absente",
                old: None,
                new: Value::Null,
                note: Some("clé absente : rien à retirer".into()),
            });
            continue;
        };
        text = format
            .remove(&text, path)
            .map_err(|e| format!("« {shown} » : {e}"))?;
        remove_path(&mut expected, path);
        reports.push(ChangeReport {
            key: shown.clone(),
            action: "retirée",
            old: Some(mask_value(&current, secret).0),
            new: Value::Null,
            note: secret.then(|| "valeur secrète (masquée)".to_string()),
        });
    }
    // Relecture stricte : l'arbre d'avant, privé des seules clés retirées.
    let after = format.tree(&text).map_err(|e| {
        format!(
            "retrait refusé, rien n'a été écrit : le résultat ne s'analyserait plus en {} ({e})",
            format.label()
        )
    })?;
    if let Some(at) = first_difference(&expected, &after, &mut Vec::new()) {
        return Err(format!(
            "retrait refusé, rien n'a été écrit : après retrait, « {at} » ne vaudrait pas ce qui est attendu (structure inhabituelle à cet endroit ; utilise edit_file pour ce cas)"
        ));
    }
    let mut warnings = Vec::new();
    if !requests.is_empty() {
        let o = edit_text(format, label, &text, requests, opts, redact)?;
        text = o.new_text;
        reports.extend(o.changes);
        warnings = o.warnings;
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

impl Core {
    /// [`Core::config_edit`] avec des clés à retirer (`set_config` avec `remove`).
    pub(crate) async fn config_edit_removing(
        &self,
        server_id: i64,
        path: &str,
        requests: &[EditRequest],
        removals: &[String],
        opts: EditOptions,
        dry_run: bool,
    ) -> Result<(String, EditOutcome, bool), String> {
        let (path, format, text) = self
            .config_read(server_id, path)
            .await
            .map_err(|e| e.to_string())?;
        let redact = |s: &str| self.redact_ai(s);
        let outcome = edit_text_removing(format, &path, &text, requests, removals, opts, &redact)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn same(s: &str) -> String {
        s.to_string()
    }

    fn rm(format: Format, text: &str, keys: &[&str]) -> Result<EditOutcome, String> {
        let keys: Vec<String> = keys.iter().map(|k| k.to_string()).collect();
        edit_text_removing(
            format,
            "/x",
            text,
            &[],
            &keys,
            EditOptions::default(),
            &same,
        )
    }

    fn removed(format: Format, text: &str, keys: &[&str]) -> String {
        rm(format, text, keys)
            .unwrap_or_else(|e| panic!("{e}"))
            .new_text
    }

    #[test]
    fn yaml_keys_go_with_their_lines_and_comments_stay() {
        let text = "# entête\nservers:\n  lobby:\n    motd: '&1Lobby'\n    address: localhost:25565 # local\n    restricted: false\n  pvp:\n    address: 1.2.3.4:25565\nlisteners:\n- query_port: 25577\n  forced_hosts:\n    pvp.md-5.net: pvp\n    lobby.example.com: lobby\n  priorities:\n  - lobby\nip_forward: false\n";
        let out = removed(Format::Yaml, text, &["servers.lobby"]);
        assert_eq!(
            out,
            text.replace("  lobby:\n    motd: '&1Lobby'\n    address: localhost:25565 # local\n    restricted: false\n", "")
        );
        let out = removed(
            Format::Yaml,
            text,
            &["listeners[0].forced_hosts.\"pvp.md-5.net\""],
        );
        assert!(!out.contains("pvp.md-5.net") && out.contains("    lobby.example.com: lobby\n"));
        // Dernière clé d'une section : section vide `{}` (jamais une valeur nulle).
        let out = removed(
            Format::Yaml,
            text,
            &[
                "listeners[0].forced_hosts.\"pvp.md-5.net\"",
                "listeners[0].forced_hosts.\"lobby.example.com\"",
            ],
        );
        assert!(out.contains("  forced_hosts: {}\n  priorities:"), "{out}");
        let tree = Format::Yaml.tree(&out).unwrap();
        assert_eq!(tree["listeners"][0]["forced_hosts"], json!({}));
        // Première clé d'un élément de liste : la suivante remonte sur la ligne du tiret.
        let out = removed(Format::Yaml, text, &["listeners[0].query_port"]);
        assert!(
            out.contains("listeners:\n- forced_hosts:\n    pvp.md-5.net: pvp"),
            "{out}"
        );
        // Section en ligne ; clé de premier niveau.
        let flow = "a: {b: 1, c: 2}\nd: [1, 2]\n";
        assert_eq!(
            removed(Format::Yaml, flow, &["a.b"]),
            "a: {c: 2}\nd: [1, 2]\n"
        );
        assert_eq!(removed(Format::Yaml, flow, &["d"]), "a: {b: 1, c: 2}\n");
    }

    #[test]
    fn toml_entries_go_and_their_comments_move_to_the_next_value() {
        let text = "bind = \"0.0.0.0:25565\"\n\n[servers]\n# Vos serveurs ici.\nlobby = \"127.0.0.1:30066\"\nfactions = \"127.0.0.1:30067\"\n\n# Ordre de repli.\ntry = [\"lobby\"]\n\n[forced-hosts]\n\"lobby.example.com\" = [\"lobby\"]\n\"factions.example.com\" = [\"factions\"]\n";
        let out = removed(
            Format::Toml,
            text,
            &["servers.lobby", "forced-hosts.\"factions.example.com\""],
        );
        assert!(
            !out.contains("lobby = ") && !out.contains("factions.example.com"),
            "{out}"
        );
        assert!(
            out.contains("# Vos serveurs ici.\nfactions = \"127.0.0.1:30067\""),
            "{out}"
        );
        assert!(out.contains("\"lobby.example.com\" = [\"lobby\"]"));
        let tree = Format::Toml.tree(&out).unwrap();
        assert_eq!(tree["servers"]["try"], json!(["lobby"]));
        // Table vidée : elle reste (vide), le document reste valable.
        let out = removed(
            Format::Toml,
            text,
            &[
                "forced-hosts.\"lobby.example.com\"",
                "forced-hosts.\"factions.example.com\"",
            ],
        );
        assert_eq!(Format::Toml.tree(&out).unwrap()["forced-hosts"], json!({}));
        // Table en ligne.
        assert_eq!(
            removed(Format::Toml, "a = { b = 1, c = 2 }\n", &["a.b"]),
            "a = { c = 2 }\n"
        );
    }

    #[test]
    fn json_members_go_with_their_comma() {
        let text = "{\n  \"a\": 1,\n  // à propos de b\n  \"b\": {\"x\": true, \"y\": false},\n  \"c\": \"z\"\n}\n";
        assert_eq!(
            removed(Format::Json, text, &["a"]),
            "{\n  // à propos de b\n  \"b\": {\"x\": true, \"y\": false},\n  \"c\": \"z\"\n}\n"
        );
        assert_eq!(
            removed(Format::Json, text, &["c"]),
            "{\n  \"a\": 1,\n  // à propos de b\n  \"b\": {\"x\": true, \"y\": false}\n}\n"
        );
        assert_eq!(
            removed(Format::Json, text, &["b.x"]),
            text.replace("{\"x\": true, \"y\": false}", "{\"y\": false}")
        );
        assert_eq!(removed(Format::Json, "{\"only\": 1}", &["only"]), "{}");
    }

    #[test]
    fn properties_lines_go_every_time_they_appear() {
        let text = "#commentaire\nmotd=Salut\nrcon.password=abc\nmotd=Encore\nport=1\n";
        let o = rm(Format::Properties, text, &["motd", "rcon.password"]).unwrap();
        assert_eq!(o.new_text, "#commentaire\nport=1\n");
        // Valeur secrète : masquée dans le compte rendu et dans le diff.
        let pw = o.changes.iter().find(|c| c.key == "rcon.password").unwrap();
        assert_eq!(pw.old, Some(json!("[SECRET]")));
        assert!(!o.diff.contains("abc"), "{}", o.diff);
    }

    #[test]
    fn removal_is_checked_and_idempotent() {
        let text = "a: 1\nb: 2\n";
        // Clé absente : signalée, rien ne change.
        let o = rm(Format::Yaml, text, &["zz"]).unwrap();
        assert!(!o.changed);
        assert_eq!(o.changes[0].action, "absente");
        // Index, doublon, retrait et modification d'une même clé : refusés.
        assert!(rm(Format::Yaml, "l: [1, 2]\n", &["l[0]"])
            .unwrap_err()
            .contains("index"));
        assert!(rm(Format::Yaml, text, &["a", "a"])
            .unwrap_err()
            .contains("deux fois"));
        let both = edit_text_removing(
            Format::Yaml,
            "/x",
            text,
            &[EditRequest {
                key: "a".into(),
                value: json!(3),
            }],
            &["a".to_string()],
            EditOptions::default(),
            &same,
        );
        assert!(both.unwrap_err().contains("à la fois"));
        // Retrait et modification ensemble : un seul diff, les deux comptes rendus.
        let o = edit_text_removing(
            Format::Yaml,
            "/x",
            text,
            &[EditRequest {
                key: "c".into(),
                value: json!(true),
            }],
            &["a".to_string()],
            EditOptions::default(),
            &same,
        )
        .unwrap();
        assert_eq!(o.new_text, "b: 2\nc: true\n");
        assert!(o.diff.contains("-a: 1") && o.diff.contains("+c: true"));
        let actions: Vec<&str> = o.changes.iter().map(|c| c.action).collect();
        assert_eq!(actions, ["retirée", "créée"]);
        // Fichier invalide : refus avant tout.
        assert!(rm(Format::Yaml, "a: [\n", &["a"]).is_err());
        // Ancre réutilisée : refus (edit_file).
        assert!(rm(Format::Yaml, "a: &x 1\nb: *x\n", &["a"]).is_err());
    }

    #[test]
    fn removals_are_read_from_set_config_arguments() {
        assert_eq!(parse_removals(None).unwrap(), Vec::<String>::new());
        assert_eq!(
            parse_removals(Some(&json!(["a.b", " c "]))).unwrap(),
            ["a.b", "c"]
        );
        assert_eq!(parse_removals(Some(&json!("x"))).unwrap(), ["x"]);
        assert!(parse_removals(Some(&json!([1]))).is_err());
        assert!(parse_removals(Some(&json!({"a": 1}))).is_err());
    }
}
