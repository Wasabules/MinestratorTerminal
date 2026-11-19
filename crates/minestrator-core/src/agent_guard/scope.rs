//! Droits selon l'origine des serveurs (« mode constructeur ») : logique pure, sans réseau.
//!
//! En portée `builder`, un outil modifiant n'est permis que sur les serveurs **créés pendant la
//! session MCP** ou listés dans `allowed_servers`. Ce module dit quels serveurs un appel vise
//! ([`write_targets`]) et tranche une fois ces serveurs résolus ([`check`]) ; la résolution d'une
//! MyBox ou d'un changeset en serveurs (appels réseau ou base) est faite par l'appelant.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Portée des écritures des agents connectés au serveur MCP (réglage `write_scope` de
/// `mcp.json`, surchargé par `MINESTRATOR_MCP_WRITE_SCOPE`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum WriteScope {
    /// Tout le compte (comportement historique).
    #[default]
    All,
    /// Mode constructeur : seulement les serveurs créés pendant la session MCP et ceux de
    /// `allowed_servers` ; lectures partout, création de serveurs permise.
    Builder,
}

/// Variable d'environnement qui impose la portée (`all` ou `builder`), quel que soit `mcp.json`.
/// Valeur illisible : `builder` (le plus restrictif).
pub const MCP_WRITE_SCOPE_ENV: &str = "MINESTRATOR_MCP_WRITE_SCOPE";
/// Variable d'environnement qui remplace `allowed_servers` : identifiants séparés par des virgules.
pub const MCP_ALLOWED_SERVERS_ENV: &str = "MINESTRATOR_MCP_ALLOWED_SERVERS";

/// Clés d'arguments qui désignent un serveur. Même liste que `official_relay::SERVER_KEYS`
/// (privée là-bas) : à garder alignées.
pub(crate) const SERVER_KEYS: &[&str] =
    &["server_id", "other_server_id", "source_server_id", "target_server_id", "proxy_server_id"];
/// Clés d'arguments qui listent des serveurs (`broadcast_command`, `link_proxy`). Alignées sur
/// `official_relay::SERVER_LIST_KEYS`.
pub(crate) const SERVER_LIST_KEYS: &[&str] = &["server_ids", "backend_server_ids"];

/// Portée effective : la variable d'environnement prime sur le réglage, la liste d'env aussi.
pub(crate) fn effective(
    scope: WriteScope,
    allowed: &[i64],
    env_scope: Option<&str>,
    env_allowed: Option<&str>,
) -> (WriteScope, Vec<i64>) {
    let scope = match env_scope.map(|s| s.trim().to_ascii_lowercase()) {
        None => scope,
        Some(s) if s.is_empty() => scope,
        Some(s) if s == "all" => WriteScope::All,
        // `builder`, ou toute valeur illisible : le plus restrictif.
        Some(_) => WriteScope::Builder,
    };
    let allowed = match env_allowed {
        Some(csv) => csv.split(',').filter_map(|t| t.trim().parse::<i64>().ok()).collect(),
        None => allowed.to_vec(),
    };
    (scope, allowed)
}

/// Ce qu'un outil modifiant vise, d'après ses arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Targets {
    /// Création d'un serveur : toujours permise (le serveur créé rejoint la portée).
    Create,
    /// Serveurs désignés par les arguments.
    Servers(Vec<i64>),
    /// Une MyBox entière : permis si tous ses serveurs le sont.
    MyBox(i64),
    /// Annulation d'un changeset : permis si tous les serveurs de ses étapes le sont.
    Changeset(String),
    /// Réinitialisation du mot de passe d'une base de données : la base sert à tous les serveurs
    /// de sa MyBox (`mybox`, sinon celle de `server`), plus `server` dont la config est écrite.
    Database { server: i64, mybox: Option<i64> },
    /// `all_running=true` : tous les serveurs démarrés, ensemble non vérifiable d'avance.
    AllRunning,
    /// Aucune cible lisible (outil au niveau du compte, ou outil inconnu).
    Unknown,
    /// Un identifiant présent mais illisible (refus, comme la portée du Copilote).
    Invalid(String),
}

/// Identifiant entier d'un argument (nombre, ou chaîne de chiffres comme en envoient certains
/// clients). `Err` si la clé est présente mais illisible.
fn id_arg(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(Some)
            .ok_or_else(|| key.to_string()),
    }
}

/// Tous les serveurs désignés par les clés connues (`SERVER_KEYS` et les listes
/// `SERVER_LIST_KEYS`).
pub(crate) fn servers_in(args: &Value) -> Result<Vec<i64>, String> {
    let mut out = Vec::new();
    for key in SERVER_KEYS {
        if let Some(id) = id_arg(args, key)? {
            out.push(id);
        }
    }
    for key in SERVER_LIST_KEYS {
        match args.get(*key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                for v in items {
                    out.push(v.as_i64().ok_or_else(|| (*key).to_string())?);
                }
            }
            Some(_) => return Err((*key).to_string()),
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Cibles d'un outil modifiant. Règles particulières :
/// - `mybox_create_server` et `clone_server` créent (permis) ; le serveur créé est enregistré
///   pour la session ([`super::record_created_server`]) ;
/// - copie entre serveurs : seule la cible compte ;
/// - outils qui portent sur une MyBox entière (`rename_mybox`, `mybox_reorder_servers`, ou tout
///   outil qui ne désigne qu'une `mybox_id`) : tous les serveurs de la MyBox ;
/// - `broadcast_command` avec `all_running=true` : refusé (ensemble non vérifiable d'avance) ;
/// - `connect_database` avec `rotate_password=true` : le serveur et toute la MyBox de la base
///   (un mot de passe réinitialisé casse les autres connexions à cette base).
/// - `link_proxy` : le proxy **et** chaque serveur de jeu (tous reçoivent des écritures) ; sans
///   proxy lisible, rien n'est vérifiable.
pub(crate) fn write_targets(name: &str, args: &Value) -> Targets {
    let one = |key: &str| match id_arg(args, key) {
        Ok(Some(id)) => Targets::Servers(vec![id]),
        Ok(None) => Targets::Unknown,
        Err(k) => Targets::Invalid(k),
    };
    let mybox = || match id_arg(args, "mybox_id") {
        Ok(Some(id)) => Targets::MyBox(id),
        Ok(None) => Targets::Unknown,
        Err(k) => Targets::Invalid(k),
    };
    match name {
        "mybox_create_server" | "clone_server" => return Targets::Create,
        "rollback_changeset" => {
            return match args.get("changeset_id").and_then(Value::as_str) {
                Some(id) => Targets::Changeset(id.to_string()),
                None => Targets::Unknown,
            }
        }
        "copy_between_servers" => return one("target_server_id"),
        "rename_mybox" | "mybox_reorder_servers" => return mybox(),
        "broadcast_command" if args.get("all_running").and_then(Value::as_bool) == Some(true) => {
            return Targets::AllRunning
        }
        "connect_database" if args.get("rotate_password").and_then(Value::as_bool) == Some(true) => {
            return match (id_arg(args, "server_id"), id_arg(args, "mybox_id")) {
                (Err(k), _) | (_, Err(k)) => Targets::Invalid(k),
                (Ok(Some(server)), Ok(mybox)) => Targets::Database { server, mybox },
                (Ok(None), _) => Targets::Unknown,
            }
        }
        "link_proxy" => {
            return match (id_arg(args, "proxy_server_id"), servers_in(args)) {
                (Err(k), _) | (_, Err(k)) => Targets::Invalid(k),
                (Ok(None), _) => Targets::Unknown,
                (Ok(Some(_)), Ok(ids)) => Targets::Servers(ids),
            }
        }
        _ => {}
    }
    match servers_in(args) {
        Err(k) => Targets::Invalid(k),
        Ok(ids) if !ids.is_empty() => Targets::Servers(ids),
        Ok(_) => mybox(),
    }
}

/// Préfixe commun des refus.
const REFUS: &str = "Refusé (mode constructeur) : les écritures sont limitées aux serveurs créés pendant cette session MCP et aux serveurs autorisés dans les réglages";

/// Tranche une fois les cibles résolues. `resolved` : serveurs d'une MyBox ou d'un changeset
/// (lus par l'appelant), `None` s'ils n'ont pas pu être lus (refus : on ne vérifie pas à l'aveugle).
pub(crate) fn check(
    targets: &Targets,
    resolved: Option<&[i64]>,
    permitted: impl Fn(i64) -> bool,
) -> Result<(), String> {
    let outside = |ids: &[i64]| -> Vec<String> {
        ids.iter().filter(|id| !permitted(**id)).map(|id| id.to_string()).collect()
    };
    match targets {
        Targets::Create => Ok(()),
        Targets::Servers(ids) => {
            let out = outside(ids);
            if out.is_empty() {
                Ok(())
            } else {
                Err(format!("{REFUS} ; serveur(s) {} hors de cette portée.", out.join(", ")))
            }
        }
        Targets::MyBox(id) => match resolved {
            None => Err(format!("{REFUS} ; serveurs de la MyBox {id} illisibles, rien n'est vérifiable.")),
            Some(ids) => {
                let out = outside(ids);
                if out.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "{REFUS} ; l'outil porte sur toute la MyBox {id}, dont des serveurs hors de cette portée ({}).",
                        out.join(", ")
                    ))
                }
            }
        },
        Targets::Changeset(cs) => match resolved {
            None => Err(format!("{REFUS} ; serveurs du changeset « {cs} » illisibles.")),
            Some(ids) => {
                let out = outside(ids);
                if out.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "{REFUS} ; ce changeset modifie des serveurs hors de cette portée ({}).",
                        out.join(", ")
                    ))
                }
            }
        },
        Targets::Database { .. } => match resolved {
            None => Err(format!("{REFUS} ; MyBox de la base illisible, rien n'est vérifiable.")),
            Some(ids) => {
                let out = outside(ids);
                if out.is_empty() {
                    Ok(())
                } else {
                    Err(format!(
                        "{REFUS} ; réinitialiser le mot de passe d'une base touche tous les serveurs de sa MyBox, dont des serveurs hors de cette portée ({}).",
                        out.join(", ")
                    ))
                }
            }
        },
        Targets::AllRunning => Err(format!(
            "{REFUS} ; `all_running` vise tous les serveurs démarrés : donne plutôt une liste `server_ids` de serveurs permis."
        )),
        Targets::Unknown => Err(format!("{REFUS} ; cet outil ne désigne aucun serveur vérifiable.")),
        Targets::Invalid(key) => Err(format!("{REFUS} ; identifiant `{key}` illisible.")),
    }
}

/// Identifiant du serveur créé, relevé dans la sortie de `mybox_create_server`
/// (« Serveur 123 créé dans la MyBox … »). Repli tant que l'outil ne l'enregistre pas lui-même
/// via [`super::record_created_server`].
pub(crate) fn created_server_id(output: &str) -> Option<i64> {
    let rest = output.trim_start().strip_prefix("Serveur ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let after = &rest[digits.len()..];
    if !after.starts_with(" créé") {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn env_overrides_the_settings_and_fails_closed() {
        let (s, a) = effective(WriteScope::All, &[1], None, None);
        assert_eq!((s, a), (WriteScope::All, vec![1]));
        assert_eq!(effective(WriteScope::All, &[], Some("builder"), None).0, WriteScope::Builder);
        assert_eq!(effective(WriteScope::Builder, &[], Some(" ALL "), None).0, WriteScope::All);
        assert_eq!(effective(WriteScope::Builder, &[], Some(""), None).0, WriteScope::Builder);
        // Valeur illisible : le plus restrictif.
        assert_eq!(effective(WriteScope::All, &[], Some("tout"), None).0, WriteScope::Builder);
        // La liste d'env remplace celle des réglages ; entrées illisibles ignorées.
        assert_eq!(effective(WriteScope::Builder, &[9], None, Some("1, 2,x,,3")).1, vec![1, 2, 3]);
        assert!(effective(WriteScope::Builder, &[9], None, Some("")).1.is_empty());
    }

    #[test]
    fn targets_follow_the_tool_rules() {
        assert_eq!(write_targets("mybox_create_server", &json!({ "mybox_id": 3 })), Targets::Create);
        // Le clone ne lit que la source : seule la création compte.
        assert_eq!(write_targets("clone_server", &json!({ "server_id": 5, "mybox_id": 3 })), Targets::Create);
        assert_eq!(write_targets("write_file", &json!({ "server_id": 5, "path": "/a" })), Targets::Servers(vec![5]));
        assert_eq!(write_targets("write_file", &json!({ "server_id": "5" })), Targets::Servers(vec![5]));
        // Copie : seule la cible compte.
        assert_eq!(
            write_targets("copy_between_servers", &json!({ "source_server_id": 1, "target_server_id": 2 })),
            Targets::Servers(vec![2])
        );
        // Une MyBox entière.
        assert_eq!(write_targets("rename_mybox", &json!({ "mybox_id": 7, "name": "x" })), Targets::MyBox(7));
        assert_eq!(
            write_targets("mybox_reorder_servers", &json!({ "mybox_id": 7, "server_ids": [1, 2] })),
            Targets::MyBox(7)
        );
        // Un serveur d'une MyBox : seulement ce serveur.
        assert_eq!(
            write_targets("mybox_resize_server", &json!({ "mybox_id": 7, "server_id": 4 })),
            Targets::Servers(vec![4])
        );
        // Outil inconnu qui ne désigne qu'une MyBox (outil officiel relayé, par exemple).
        assert_eq!(write_targets("create_database", &json!({ "mybox_id": 8 })), Targets::MyBox(8));
        // Plusieurs serveurs.
        assert_eq!(
            write_targets("broadcast_command", &json!({ "server_ids": [3, 1, 3], "command": "say" })),
            Targets::Servers(vec![1, 3])
        );
        assert_eq!(write_targets("broadcast_command", &json!({ "all_running": true })), Targets::AllRunning);
        assert_eq!(write_targets("create_ticket", &json!({ "subject": "x" })), Targets::Unknown);
        assert_eq!(write_targets("write_file", &json!({ "server_id": "abc" })), Targets::Invalid("server_id".into()));
        assert_eq!(write_targets("broadcast_command", &json!({ "server_ids": [1, "x"] })), Targets::Invalid("server_ids".into()));
        assert_eq!(
            write_targets("rollback_changeset", &json!({ "changeset_id": "mcp:p1" })),
            Targets::Changeset("mcp:p1".into())
        );
        // Base de données : seulement le serveur, sauf réinitialisation du mot de passe.
        assert_eq!(
            write_targets("connect_database", &json!({ "server_id": 5, "target": "luckperms" })),
            Targets::Servers(vec![5])
        );
        assert_eq!(
            write_targets("connect_database", &json!({ "server_id": 5, "rotate_password": true })),
            Targets::Database { server: 5, mybox: None }
        );
        assert_eq!(
            write_targets("connect_database", &json!({ "server_id": 5, "mybox_id": "8", "rotate_password": true })),
            Targets::Database { server: 5, mybox: Some(8) }
        );
        assert_eq!(
            write_targets("connect_database", &json!({ "server_id": 5, "mybox_id": "x", "rotate_password": true })),
            Targets::Invalid("mybox_id".into())
        );
        // Réseau proxy : le proxy et tous les serveurs de jeu.
        assert_eq!(
            write_targets("link_proxy", &json!({ "proxy_server_id": 9, "backend_server_ids": [4, 2] })),
            Targets::Servers(vec![2, 4, 9])
        );
        assert_eq!(write_targets("link_proxy", &json!({ "backend_server_ids": [4] })), Targets::Unknown);
        assert_eq!(
            write_targets("link_proxy", &json!({ "proxy_server_id": 9, "backend_server_ids": [4, "x"] })),
            Targets::Invalid("backend_server_ids".into())
        );
        assert_eq!(
            write_targets("link_proxy", &json!({ "proxy_server_id": "p", "backend_server_ids": [4] })),
            Targets::Invalid("proxy_server_id".into())
        );
    }

    #[test]
    fn check_allows_only_permitted_servers() {
        let permitted = |id: i64| id == 1 || id == 2;
        assert!(check(&Targets::Create, None, permitted).is_ok());
        assert!(check(&Targets::Servers(vec![1, 2]), None, permitted).is_ok());
        let e = check(&Targets::Servers(vec![1, 9]), None, permitted).unwrap_err();
        assert!(e.contains("mode constructeur") && e.contains('9') && !e.contains("1,"), "{e}");
        // MyBox : tous ses serveurs doivent être permis ; illisible = refus.
        assert!(check(&Targets::MyBox(4), Some(&[1, 2]), permitted).is_ok());
        assert!(check(&Targets::MyBox(4), Some(&[1, 3]), permitted).is_err());
        assert!(check(&Targets::MyBox(4), None, permitted).is_err());
        // MyBox vide : rien hors portée.
        assert!(check(&Targets::MyBox(4), Some(&[]), permitted).is_ok());
        assert!(check(&Targets::Changeset("c".into()), Some(&[2]), permitted).is_ok());
        assert!(check(&Targets::Changeset("c".into()), Some(&[5]), permitted).is_err());
        assert!(check(&Targets::Changeset("c".into()), None, permitted).is_err());
        // Base : le serveur et toute la MyBox (résolus par l'appelant) ; illisible = refus.
        let db = Targets::Database { server: 1, mybox: None };
        assert!(check(&db, Some(&[1, 2]), permitted).is_ok());
        let e = check(&db, Some(&[1, 2, 7]), permitted).unwrap_err();
        assert!(e.contains("mot de passe") && e.contains('7'), "{e}");
        assert!(check(&db, None, permitted).is_err());
        assert!(check(&Targets::AllRunning, None, |_| true).is_err());
        assert!(check(&Targets::Unknown, None, |_| true).is_err());
        assert!(check(&Targets::Invalid("server_id".into()), None, |_| true).is_err());
    }

    #[test]
    fn created_server_is_read_from_the_tool_output() {
        assert_eq!(
            created_server_id("Serveur 4242 créé dans la MyBox 7 (adresse : x). L'hébergeur l'installe"),
            Some(4242)
        );
        assert_eq!(created_server_id("Serveur 12 renommé."), None);
        assert_eq!(created_server_id("Erreur : quota"), None);
        assert_eq!(created_server_id("Serveur  créé"), None);
    }

    /// Lot plateforme : `set_server_platform` vise son serveur ; `delete_database`, qui ne désigne
    /// qu'une `mybox_id` (`database_id` n'est pas un serveur), toute la MyBox (règle standard).
    #[test]
    fn platform_and_database_deletion_follow_the_standard_rules() {
        assert_eq!(
            write_targets("set_server_platform", &json!({ "server_id": 5, "platform": "velocity" })),
            Targets::Servers(vec![5])
        );
        assert_eq!(
            write_targets("delete_database", &json!({ "mybox_id": 3, "database_id": 99, "confirm_name": "x" })),
            Targets::MyBox(3)
        );
        assert_eq!(write_targets("delete_database", &json!({ "database_id": 99 })), Targets::Unknown);
    }
}
