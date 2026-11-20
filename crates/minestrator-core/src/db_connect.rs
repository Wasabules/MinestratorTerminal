//! Outil MCP `connect_database` : brancher une base de données d'une MyBox à un plugin **sans
//! que l'IA voie le mot de passe**.
//!
//! Les accès d'une base (hôte, port, nom, utilisateur, mot de passe) sont dans `GET /mybox/{id}` →
//! `databases[]` (la description de l'outil officiel `get_mybox` annonce les « accès complets ») ;
//! le mot de passe régénéré par l'outil officiel `manage_database` (`reset_password`) « se lit via
//! `get_mybox` ». La **forme exacte des entrées n'a pas pu être relevée** (compte de test sans base,
//! `docs/api/REFERENCE.md`) : [`read_creds`] les lit avec tolérance (plusieurs noms candidats par
//! champ, un niveau d'imbrication) et, s'il manque un champ indispensable, échoue en citant les
//! **noms** de champs vus, jamais leurs valeurs. Mot de passe absent ou masqué dans l'API : repli
//! sur l'outil officiel `get_mybox` (lecture, connexion `readonly=1`), jamais montré au modèle.
//!
//! **Écriture.** La config est modifiée par `set_config` (édition structurée vérifiée), passé par la
//! garde des agents (`guarded_write`) : étape capturée et annulable avec `rollback_changeset`. La
//! vraie valeur ne transite que dans les arguments internes de `set_config` : elle n'est ni dans
//! les arguments de `connect_database` (journalisés), ni dans la sortie (les clés de mot de passe
//! sont masquées des deux côtés du diff par `config_edit`, et toute sortie ou erreur est en plus
//! nettoyée de la valeur, [`scrub`]). Une clé de mot de passe qui ne serait pas reconnue comme
//! secrète par `config_edit` est refusée (sa valeur apparaîtrait dans le diff). `dry_run` (défaut)
//! montre le diff sans rien écrire.
//!
//! **Réinitialisation** (`rotate_password`, faux par défaut) : seulement si le mot de passe n'est
//! pas lisible. Demandée à l'humain (`mcp_ctx::confirm`) **avant tout effet** : elle casse toute
//! connexion qui utilise l'ancien mot de passe. Faite par l'outil officiel `manage_database`
//! (`action: reset_password`), dont le corps est connu, plutôt que par `PATCH
//! /mybox/{id}/database/password` (corps non documenté, jamais essayé) ; la configuration est
//! vérifiée (simulation) avant, pour ne jamais réinitialiser un mot de passe qu'on ne saurait pas
//! écrire ensuite.
//!
//! L'outil ne crée pas de base : `create_database` (outil officiel, passerelle `official_gateway`)
//! ou le panel.
//!
//! **Suppression** (`delete_database`, [`delete_database_tool`]) : `DELETE
//! /mybox/{id}/database/{id}` (essayé en réel le 2026-11-19 : réponse `0`, la base disparaît de
//! `GET /mybox/{id}`), seulement si la base est listée dans la MyBox et après confirmation de son
//! nom exact (`dbname`) par l'humain (élicitation) ou par `confirm_name` donné par l'utilisateur ;
//! le nom attendu n'est jamais révélé dans un refus, le mot de passe jamais affiché.

use crate::config_edit;
use crate::mcp_ctx::{self, Confirmation};
use crate::Core;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Masque des valeurs secrètes dans les sorties.
const MASK: &str = config_edit::SECRET_MASK;
/// Valeur provisoire du mot de passe dans la simulation qui précède une réinitialisation (clé
/// secrète : masquée dans le diff comme la vraie).
const PENDING_PASSWORD: &str = "mot-de-passe-a-reinitialiser";
/// Port MySQL/MariaDB par défaut, supposé si l'API n'en donne pas.
const DEFAULT_PORT: i64 = 3306;

// --- Lecture tolérante des accès -------------------------------------------------------------------

const ID_KEYS: &[&str] = &["id", "id_database", "database_id", "db_id"];
const HOST_KEYS: &[&str] = &[
    "host",
    "hostname",
    "db_host",
    "database_host",
    "sql_host",
    "address",
    "server",
    "ip",
    "endpoint",
];
const PORT_KEYS: &[&str] = &["port", "db_port", "database_port", "sql_port"];
const NAME_KEYS: &[&str] = &[
    "database",
    "database_name",
    "db_name",
    "dbname",
    "db",
    "schema",
    "name",
];
const USER_KEYS: &[&str] = &[
    "username",
    "user",
    "user_name",
    "login",
    "db_user",
    "db_username",
    "database_username",
    "database_user",
];
const PASS_KEYS: &[&str] = &[
    "password",
    "pass",
    "passwd",
    "pwd",
    "db_password",
    "db_pass",
    "database_password",
];
const ENGINE_KEYS: &[&str] = &["engine", "type", "driver", "dbms", "server_type", "version"];
const LABEL_KEYS: &[&str] = &["description", "label", "comment"];

/// Moteur de la base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engine {
    MariaDb,
    MySql,
    /// PostgreSQL, MongoDB… : aucun préréglage.
    Other,
    Unknown,
}

impl Engine {
    fn detect(text: Option<&str>) -> Self {
        let Some(t) = text.map(str::to_ascii_lowercase) else {
            return Engine::Unknown;
        };
        if t.contains("maria") {
            Engine::MariaDb
        } else if t.contains("mysql") {
            Engine::MySql
        } else if ["postgres", "mongo", "redis", "sqlite"]
            .iter()
            .any(|o| t.contains(o))
        {
            Engine::Other
        } else {
            Engine::Unknown
        }
    }

    fn label(self) -> &'static str {
        match self {
            Engine::MariaDb => "mariadb",
            Engine::MySql => "mysql",
            Engine::Other => "autre",
            Engine::Unknown => "inconnu (MySQL/MariaDB supposé)",
        }
    }
}

/// Accès d'une base, lus dans l'API. `password` : `None` s'il n'est pas lisible.
#[derive(Clone, PartialEq)]
pub(crate) struct Creds {
    pub id: i64,
    pub label: Option<String>,
    pub engine: Engine,
    pub host: String,
    pub port: i64,
    pub port_assumed: bool,
    pub database: String,
    pub username: String,
    pub password: Option<String>,
}

impl std::fmt::Debug for Creds {
    /// Jamais le mot de passe, même dans un message de test ou de journal.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds")
            .field("id", &self.id)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| MASK))
            .finish()
    }
}

impl Creds {
    /// `hôte:port`, forme attendue par LuckPerms et LiteBans.
    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Feuilles d'une entrée (un niveau d'imbrication) : `(chemin, dernier segment en minuscules, valeur)`.
fn leaves(entry: &Value) -> Vec<(String, String, &Value)> {
    let mut out = Vec::new();
    let Some(map) = entry.as_object() else {
        return out;
    };
    for (k, v) in map {
        match v {
            Value::Object(inner) => {
                for (k2, v2) in inner {
                    if !v2.is_object() && !v2.is_array() {
                        out.push((format!("{k}.{k2}"), k2.to_ascii_lowercase(), v2));
                    }
                }
            }
            Value::Array(_) => {}
            _ => out.push((k.clone(), k.to_ascii_lowercase(), v)),
        }
    }
    // Les champs de premier niveau d'abord.
    out.sort_by_key(|(path, ..)| path.contains('.'));
    out
}

/// Noms de champs vus (jamais les valeurs), pour un message d'erreur.
fn field_names(entry: &Value) -> String {
    let mut names: Vec<String> = Vec::new();
    if let Some(map) = entry.as_object() {
        for (k, v) in map {
            match v {
                Value::Object(inner) => names.extend(inner.keys().map(|k2| format!("{k}.{k2}"))),
                _ => names.push(k.clone()),
            }
        }
    }
    if names.is_empty() {
        "aucun".into()
    } else {
        names.join(", ")
    }
}

fn scalar_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Premier champ non vide parmi les noms candidats (par ordre de priorité).
fn pick(leaves: &[(String, String, &Value)], candidates: &[&str]) -> Option<String> {
    candidates.iter().find_map(|c| {
        leaves
            .iter()
            .filter(|(_, last, _)| last == c)
            .find_map(|(_, _, v)| scalar_text(v))
    })
}

/// Mot de passe utilisable (ni vide, ni masqué par l'API : `********`, `[SECRET]`…).
fn usable_password(p: &str) -> bool {
    !p.is_empty()
        && !p
            .chars()
            .all(|c| matches!(c, '*' | '•' | '●' | 'x' | 'X' | '.'))
        && !config_edit::MASK_MARKERS.iter().any(|m| p.contains(m))
}

fn as_id(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Entrées `databases` d'une réponse `GET /mybox/{id}` (tableau, ou objet indexé par identifiant).
pub(crate) fn database_entries(data: &Value) -> Vec<&Value> {
    let list = data
        .get("databases")
        .or_else(|| data.pointer("/mybox/databases"))
        .or_else(|| data.pointer("/data/databases"));
    match list {
        Some(Value::Array(a)) => a.iter().filter(|e| e.is_object()).collect(),
        Some(Value::Object(m)) => m.values().filter(|e| e.is_object()).collect(),
        _ => Vec::new(),
    }
}

/// Identifiant d'une entrée.
fn entry_id(entry: &Value) -> Option<i64> {
    ID_KEYS.iter().find_map(|k| entry.get(*k).and_then(as_id))
}

/// Résumé non secret d'une entrée (pour lister les bases).
fn entry_summary(entry: &Value) -> String {
    let l = leaves(entry);
    let id = entry_id(entry)
        .map(|i| i.to_string())
        .unwrap_or_else(|| "?".into());
    let name = pick(&l, NAME_KEYS).or_else(|| pick(&l, LABEL_KEYS));
    match name {
        Some(n) => format!("{id} ({n})"),
        None => id,
    }
}

/// Choisit l'entrée de `database_id` ; sans identifiant, la seule base de la MyBox.
pub(crate) fn select_entry<'a>(
    entries: &[&'a Value],
    database_id: Option<i64>,
    mybox_id: i64,
) -> Result<&'a Value, String> {
    if entries.is_empty() {
        return Err(format!(
            "La MyBox {mybox_id} n'a aucune base de données (ou l'API n'en renvoie aucune). Crée-la avec l'outil officiel `create_database` {{mybox_id: {mybox_id}, engine: \"mariadb118\"}} (passerelle vers le MCP officiel : Réglages → MCP) ou dans le panel (MyBox → Bases de données), puis relance connect_database avec son identifiant."
        ));
    }
    let list = || {
        entries
            .iter()
            .map(|e| entry_summary(e))
            .collect::<Vec<_>>()
            .join(", ")
    };
    match database_id {
        Some(id) => entries
            .iter()
            .copied()
            .find(|e| entry_id(e) == Some(id))
            .ok_or_else(|| {
                format!(
                    "base {id} introuvable dans la MyBox {mybox_id} ; bases : {}",
                    list()
                )
            }),
        None if entries.len() == 1 => Ok(entries[0]),
        None => Err(format!(
            "plusieurs bases dans la MyBox {mybox_id} : précise `database_id` parmi {}",
            list()
        )),
    }
}

/// Accès d'une entrée `databases[]`. Erreur : champs indispensables absents, avec les **noms** de
/// champs vus.
pub(crate) fn read_creds(entry: &Value) -> Result<Creds, String> {
    let l = leaves(entry);
    let id = entry_id(entry).unwrap_or_default();
    let host_raw = pick(&l, HOST_KEYS);
    let database = pick(&l, NAME_KEYS);
    let username = pick(&l, USER_KEYS);
    let mut missing = Vec::new();
    if host_raw.is_none() {
        missing.push("hôte");
    }
    if database.is_none() {
        missing.push("nom de la base");
    }
    if username.is_none() {
        missing.push("utilisateur");
    }
    if !missing.is_empty() {
        return Err(format!(
            "accès de la base {id} incomplets dans la réponse de l'API ({} introuvable) ; champs vus : {}. Format non relevé jusqu'ici : à signaler pour ajuster la lecture.",
            missing.join(", "),
            field_names(entry)
        ));
    }
    let mut host = host_raw.unwrap_or_default();
    let mut port = pick(&l, PORT_KEYS)
        .and_then(|p| p.parse::<i64>().ok())
        .filter(|p| (1..=65535).contains(p));
    // `hôte:port` dans le champ de l'hôte (jamais une IPv6, qui a plusieurs `:`).
    if let Some((h, p)) = host.rsplit_once(':') {
        if !h.contains(':') {
            if let Ok(n) = p.parse::<i64>() {
                port = port.or(Some(n));
                host = h.to_string();
            }
        }
    }
    let password = pick(&l, PASS_KEYS).filter(|p| usable_password(p));
    // `GET /mybox/{id}` ne donne pas le moteur (champs relevés le 2026-11-19 : id, dbname, host,
    // port, username, password, description) ; l'hôte le dit (`mariadb1.…`).
    let engine = match Engine::detect(pick(&l, ENGINE_KEYS).as_deref()) {
        Engine::Unknown => Engine::detect(Some(&host)),
        e => e,
    };
    // Un nom (`name`) qui serait le libellé est sans effet : `database` / `db_name` priment.
    Ok(Creds {
        id,
        label: pick(&l, LABEL_KEYS),
        engine,
        host,
        port: port.unwrap_or(DEFAULT_PORT),
        port_assumed: port.is_none(),
        database: database.unwrap_or_default(),
        username: username.unwrap_or_default(),
        password,
    })
}

/// Lit le JSON d'une sortie texte (l'outil officiel peut l'entourer de texte). Partagé avec
/// `platform.rs` (sorties de `search_game_content`, `get_content_files`).
pub(crate) fn json_in_text(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        return Some(v);
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start)
        .then(|| serde_json::from_str(&text[start..=end]).ok())
        .flatten()
}

// --- Cibles : préréglages et correspondance explicite ----------------------------------------------

/// Préréglages : `(nom, fichier, clés écrites)` — documentés dans `MCP.md`.
pub(crate) const PRESETS: &[(&str, &str, &str)] = &[
    (
        "luckperms",
        "/plugins/LuckPerms/config.yml",
        "storage-method (mariadb|mysql), data.address (hôte:port), data.database, data.username, data.password",
    ),
    (
        "coreprotect",
        "/plugins/CoreProtect/config.yml",
        "use-mysql (true), mysql-host, mysql-port, mysql-database, mysql-username, mysql-password",
    ),
    (
        "litebans",
        "/plugins/LiteBans/config.yml",
        "sql.driver (MariaDB|MySQL), sql.address (hôte:port), sql.database, sql.username, sql.password",
    ),
    (
        "plan",
        "/plugins/Plan/config.yml",
        "Database.Type (MySQL), Database.MySQL.Host, Database.MySQL.Port, Database.MySQL.User, Database.MySQL.Password, Database.MySQL.Database",
    ),
];

/// Modifications d'une config : `(clé, valeur)`, dans l'ordre d'écriture.
pub(crate) type Changes = Vec<(String, Value)>;

/// Modifications d'un préréglage : `(fichier par défaut, modifications)`.
pub(crate) fn preset_changes(
    name: &str,
    c: &Creds,
    password: &str,
) -> Result<(&'static str, Changes), String> {
    let (_, path, _) = PRESETS
        .iter()
        .find(|(n, ..)| *n == name)
        .ok_or_else(|| {
            format!(
                "préréglage `target` inconnu : « {name} » ({}), ou `path` + `keys` pour un autre plugin",
                PRESETS.iter().map(|(n, ..)| *n).collect::<Vec<_>>().join(", ")
            )
        })?;
    if c.engine == Engine::Other {
        return Err(format!(
            "la base {} n'est pas MySQL/MariaDB : les préréglages ne s'appliquent pas (utilise `path` + `keys`)",
            c.id
        ));
    }
    let maria = c.engine == Engine::MariaDb;
    let k = |key: &str, v: Value| (key.to_string(), v);
    let changes = match name {
        "luckperms" => vec![
            k(
                "storage-method",
                json!(if maria { "mariadb" } else { "mysql" }),
            ),
            k("data.address", json!(c.address())),
            k("data.database", json!(c.database)),
            k("data.username", json!(c.username)),
            k("data.password", json!(password)),
        ],
        "coreprotect" => vec![
            k("use-mysql", json!(true)),
            k("mysql-host", json!(c.host)),
            k("mysql-port", json!(c.port)),
            k("mysql-database", json!(c.database)),
            k("mysql-username", json!(c.username)),
            k("mysql-password", json!(password)),
        ],
        "litebans" => vec![
            k("sql.driver", json!(if maria { "MariaDB" } else { "MySQL" })),
            k("sql.address", json!(c.address())),
            k("sql.database", json!(c.database)),
            k("sql.username", json!(c.username)),
            k("sql.password", json!(password)),
        ],
        "plan" => vec![
            k("Database.Type", json!("MySQL")),
            k("Database.MySQL.Host", json!(c.host)),
            k("Database.MySQL.Port", json!(c.port)),
            k("Database.MySQL.User", json!(c.username)),
            k("Database.MySQL.Password", json!(password)),
            k("Database.MySQL.Database", json!(c.database)),
        ],
        _ => unreachable!("préréglage listé dans PRESETS"),
    };
    Ok((path, changes))
}

/// Rôles d'une correspondance explicite (`keys`).
const ROLES: &[&str] = &[
    "host", "port", "address", "database", "username", "password",
];

/// Modifications d'une correspondance explicite `keys` (rôle → clé de config) et `set` (clé →
/// valeur littérale non secrète). La clé du mot de passe doit être reconnue comme secrète.
pub(crate) fn mapping_changes(
    path: &str,
    keys: &Map<String, Value>,
    set: Option<&Map<String, Value>>,
    c: &Creds,
    password: &str,
) -> Result<Changes, String> {
    let format = config_edit::Format::detect(path).ok_or_else(|| {
        format!("format de « {path} » non pris en charge : .yml/.yaml, .toml, .json ou .properties")
    })?;
    if keys.is_empty() {
        return Err(format!(
            "`keys` vide : rôles possibles {}",
            ROLES.join(", ")
        ));
    }
    let mut out = Vec::new();
    for (role, key) in keys {
        let key = key
            .as_str()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| format!("`keys.{role}` : chemin de clé (texte) attendu"))?;
        let segs = format
            .parse_key(key)
            .map_err(|e| format!("`keys.{role}` : {e}"))?;
        let value = match role.as_str() {
            "host" => json!(c.host),
            "port" => json!(c.port),
            "address" => json!(c.address()),
            "database" => json!(c.database),
            "username" => json!(c.username),
            "password" => {
                if !config_edit::path_is_secret(&segs) {
                    return Err(format!(
                        "`keys.password` = « {key} » : clé non reconnue comme secrète, sa valeur apparaîtrait dans le diff ; refusé (clés reconnues : …password…, …pass…, …pwd…, …secret…)"
                    ));
                }
                json!(password)
            }
            other => {
                return Err(format!(
                    "`keys` : rôle « {other} » inconnu ({})",
                    ROLES.join(", ")
                ))
            }
        };
        out.push((key.to_string(), value));
    }
    for (key, value) in set.into_iter().flatten() {
        if value.is_object() || value.is_array() {
            return Err(format!(
                "`set.{key}` : valeur texte, nombre ou booléen attendue"
            ));
        }
        let segs = format
            .parse_key(key)
            .map_err(|e| format!("`set.{key}` : {e}"))?;
        if config_edit::path_is_secret(&segs) {
            return Err(format!(
                "`set.{key}` : clé secrète ; le mot de passe de la base passe par `keys.password`"
            ));
        }
        out.push((key.clone(), value.clone()));
    }
    let mut seen = BTreeSet::new();
    for (k, _) in &out {
        if !seen.insert(k.as_str()) {
            return Err(format!("clé « {k} » écrite deux fois"));
        }
    }
    Ok(out)
}

/// Retire toute occurrence d'un secret d'un texte (défense en profondeur : sorties et erreurs).
pub(crate) fn scrub(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.chars().count() >= 3) {
        out = out.replace(s, MASK);
    }
    out
}

// --- Arguments ---------------------------------------------------------------------------------------

/// Arguments validés (sans réseau).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Request {
    pub server_id: i64,
    pub mybox_id: Option<i64>,
    pub database_id: Option<i64>,
    pub target: Option<String>,
    pub path: Option<String>,
    pub keys: Option<Map<String, Value>>,
    pub set: Option<Map<String, Value>>,
    pub rotate_password: bool,
    pub dry_run: bool,
}

fn int_arg(args: &Value, key: &str) -> Result<Option<i64>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => as_id(v)
            .map(Some)
            .ok_or_else(|| format!("`{key}` : entier attendu")),
    }
}

fn object_arg(args: &Value, key: &str) -> Result<Option<Map<String, Value>>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(m)) => Ok(Some(m.clone())),
        Some(_) => Err(format!("`{key}` : objet {{ clé: valeur }} attendu")),
    }
}

impl Request {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        let server_id =
            int_arg(args, "server_id")?.ok_or("paramètre `server_id` (entier) requis")?;
        let text = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let flag = |k: &str, d: bool| args.get(k).and_then(Value::as_bool).unwrap_or(d);
        let target = text("target").map(|t| t.to_ascii_lowercase());
        let keys = object_arg(args, "keys")?;
        let set = object_arg(args, "set")?;
        let path = text("path");
        match (&target, &keys) {
            (Some(_), Some(_)) => return Err("`target` (préréglage) ou `keys` (correspondance explicite), pas les deux".into()),
            (None, None) => {
                return Err(format!(
                    "cible requise : `target` ({}) ou `path` + `keys` {{ host|port|address|database|username|password: clé }}",
                    PRESETS.iter().map(|(n, ..)| *n).collect::<Vec<_>>().join(", ")
                ))
            }
            (None, Some(_)) if path.is_none() => return Err("`path` (fichier de config) requis avec `keys`".into()),
            (Some(_), None) if set.is_some() => return Err("`set` ne s'utilise qu'avec `keys` (un préréglage écrit déjà ses réglages)".into()),
            _ => {}
        }
        Ok(Request {
            server_id,
            mybox_id: int_arg(args, "mybox_id")?,
            database_id: int_arg(args, "database_id")?,
            target,
            path,
            keys,
            set,
            rotate_password: flag("rotate_password", false),
            dry_run: flag("dry_run", true),
        })
    }

    /// Fichier et modifications, avec le mot de passe donné.
    /// Fichier, modifications et création des clés absentes (jamais pour un préréglage).
    fn changes(&self, c: &Creds, password: &str) -> Result<(String, Changes, bool), String> {
        match (&self.target, &self.keys, &self.path) {
            (Some(t), _, path) => {
                let (default_path, changes) = preset_changes(t, c, password)?;
                // Préréglage : clés attendues dans le fichier généré par le plugin, jamais créées.
                Ok((
                    path.clone().unwrap_or_else(|| default_path.to_string()),
                    changes,
                    false,
                ))
            }
            (None, Some(keys), Some(path)) => Ok((
                path.clone(),
                mapping_changes(path, keys, self.set.as_ref(), c, password)?,
                true,
            )),
            _ => Err("cible incomplète".into()),
        }
    }
}

// --- Façade -----------------------------------------------------------------------------------------

/// Arguments internes de `set_config` (le mot de passe y passe, jamais ailleurs).
fn set_config_args(
    server_id: i64,
    path: &str,
    changes: &[(String, Value)],
    create_missing: bool,
    dry_run: bool,
) -> Value {
    let changes: Vec<Value> = changes
        .iter()
        .map(|(k, v)| json!({ "key": k, "value": v }))
        .collect();
    json!({
        "server_id": server_id,
        "path": path,
        "changes": changes,
        "create_missing": create_missing,
        "keep_type": true,
        "dry_run": dry_run,
    })
}

/// MyBox du serveur (liste des serveurs du compte).
pub(crate) async fn mybox_of(core: &Core, server_id: i64) -> Result<i64, String> {
    let overview = core.list_servers().await.map_err(|e| e.to_string())?;
    overview
        .servers
        .iter()
        .find(|s| s.id == server_id)
        .map(|s| s.mybox_id)
        .filter(|m| *m > 0)
        .ok_or_else(|| format!("serveur {server_id} introuvable sur ce compte (ou sans MyBox) : précise `mybox_id`"))
}

/// Accès de la base, API puis, faute de mot de passe lisible, outil officiel `get_mybox` (lecture).
pub(crate) async fn load_creds(core: &Core, mybox_id: i64, database_id: Option<i64>) -> Result<Creds, String> {
    let token = core.token().map_err(|e| e.to_string())?;
    let data = core
        .api
        .get_data(&token, &format!("/mybox/{mybox_id}"))
        .await
        .map_err(|e| format!("MyBox {mybox_id} illisible : {e}"))?;
    let entries = database_entries(&data);
    let entry = select_entry(&entries, database_id, mybox_id)?;
    let mut creds = read_creds(entry)?;
    if creds.password.is_none() {
        if let Ok(text) = crate::official_mcp::call_tool(
            &token,
            "mybox",
            true,
            "get_mybox",
            json!({ "mybox_id": mybox_id }),
        )
        .await
        {
            let official = json_in_text(&text).and_then(|v| {
                let list = database_entries(&v);
                list.iter()
                    .copied()
                    .find(|e| entry_id(e) == Some(creds.id))
                    .and_then(|e| read_creds(e).ok())
            });
            if let Some(o) = official.filter(|o| o.username == creds.username) {
                creds.password = o.password;
            }
        }
    }
    Ok(creds)
}

/// Sortie JSON d'une simulation `set_config` (déjà masquée par `config_edit`).
async fn preview(core: &Core, args: &Value, secrets: &[&str]) -> Result<Value, String> {
    let text = Box::pin(config_edit::set_config_tool(core, args))
        .await
        .map_err(|e| scrub(&e, secrets))?;
    serde_json::from_str(&scrub(&text, secrets)).map_err(|e| format!("simulation illisible : {e}"))
}

/// Outil MCP `connect_database`.
pub(crate) async fn connect_database_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = Request::parse(args)?;
    let id = req.server_id;
    let mybox_id = match req.mybox_id {
        Some(m) => m,
        None => mybox_of(core, id).await?,
    };
    let mut creds = load_creds(core, mybox_id, req.database_id).await?;
    let readable = creds.password.is_some();
    if !readable && !req.rotate_password {
        return Err(format!(
            "Le mot de passe de la base {} n'est pas lisible par l'API. Relance avec `rotate_password=true` pour le réinitialiser puis l'écrire directement dans la config (l'utilisateur confirme ; toute autre connexion qui utilise l'ancien mot de passe sera cassée jusqu'à sa mise à jour), ou configure-le depuis le panel. Rien n'a été modifié.",
            creds.id
        ));
    }
    let mut notes: Vec<String> = Vec::new();
    if readable && req.rotate_password {
        notes
            .push("mot de passe lisible : aucune réinitialisation (rotate_password ignoré)".into());
    }
    if creds.port_assumed {
        notes.push(format!(
            "port absent de la réponse de l'API : {DEFAULT_PORT} supposé"
        ));
    }
    if creds.engine == Engine::Unknown {
        notes.push("moteur non précisé par l'API : MySQL/MariaDB supposé".into());
    }
    let rotate = !readable;
    // Simulation d'abord (avec une valeur provisoire si le mot de passe sera réinitialisé) : un
    // fichier absent ou une clé introuvable arrête tout AVANT la moindre modification.
    let staged = creds
        .password
        .clone()
        .unwrap_or_else(|| PENDING_PASSWORD.to_string());
    let (path, changes, create_missing) = req.changes(&creds, &staged)?;
    let known = creds.password.clone().unwrap_or_default();
    let secrets_now = [known.as_str()];
    let sim = preview(core, &set_config_args(id, &path, &changes, create_missing, true), &secrets_now)
        .await
        .map_err(|e| format!("{e} — rien n'a été modifié{}", if req.target.is_some() { " (le plugin doit être installé et avoir généré sa config : démarre le serveur une fois)" } else { "" }))?;

    let mut out = json!({
        "server_id": id,
        "mybox_id": mybox_id,
        "database": {
            "id": creds.id,
            "label": creds.label,
            "engine": creds.engine.label(),
            "host": creds.host,
            "port": creds.port,
            "database": creds.database,
            "username": creds.username,
            "password": if readable { "lu par l'API, jamais affiché" } else { "non lisible" },
        },
        "target": req.target,
        "path": path,
        "dry_run": req.dry_run,
        "changes": sim.get("changes").cloned().unwrap_or(Value::Null),
        "diff": sim.get("diff").cloned().unwrap_or(Value::Null),
    });
    if let Some(w) = sim.get("warnings") {
        out["warnings"] = w.clone();
    }
    let unchanged = sim.get("diff").is_none_or(Value::is_null);

    if req.dry_run {
        out["written"] = json!(false);
        out["rotated"] = json!(false);
        out["next"] = json!(if rotate {
            "simulation : relance avec dry_run=false et rotate_password=true pour réinitialiser le mot de passe (confirmation demandée) puis écrire la config"
        } else if unchanged {
            "déjà conforme : rien à écrire"
        } else {
            "simulation : relance avec dry_run=false pour écrire la config (étape annulable avec rollback_changeset)"
        });
    } else if unchanged && !rotate {
        out["written"] = json!(false);
        out["rotated"] = json!(false);
        out["next"] = json!("déjà conforme : rien à écrire");
    } else {
        let mut rotated = false;
        if rotate {
            let label = creds
                .label
                .clone()
                .unwrap_or_else(|| creds.database.clone());
            let message = format!(
                "Réinitialiser le mot de passe de la base « {label} » (MyBox {mybox_id}) ? Toute connexion qui utilise l'ancien mot de passe (autres plugins, sites) sera cassée jusqu'à la mise à jour de sa config. Le nouveau sera écrit directement dans {path} (serveur {id}), sans être affiché."
            );
            if mcp_ctx::confirm(&message, None).await == Confirmation::Declined {
                return Err("Refusé par l'utilisateur : mot de passe non réinitialisé, rien n'a été modifié.".into());
            }
            let token = core.token().map_err(|e| e.to_string())?;
            crate::official_mcp::call_tool(
                &token,
                "databases",
                false,
                "manage_database",
                json!({ "mybox_id": mybox_id, "database_id": creds.id, "action": "reset_password" }),
            )
            .await
            .map(drop)
            .map_err(|e| format!("réinitialisation du mot de passe impossible (outil officiel manage_database) : {} — rien n'a été modifié", core.redact_ai(&e)))?;
            rotated = true;
            match load_creds(core, mybox_id, Some(creds.id)).await {
                Ok(fresh) if fresh.password.is_some() => creds = fresh,
                _ => {
                    return Err(format!(
                        "Mot de passe de la base {} RÉINITIALISÉ, mais le nouveau n'est pas lisible par l'API : les connexions existantes sont cassées. Récupère-le dans le panel (MyBox → Bases de données) et mets-le dans {path} toi-même (edit_file ; set_config refuse les valeurs masquées).",
                        creds.id
                    ));
                }
            }
        }
        let password = creds.password.clone().unwrap_or_default();
        let secrets = [password.as_str()];
        let (path, changes, create_missing) = req.changes(&creds, &password)?;
        let guard = crate::agent_guard::guarded_write(core, "set_config", set_config_args(id, &path, &changes, create_missing, false))
            .await
            .map_err(|e| {
                let e = scrub(&e, &secrets);
                if rotated {
                    format!("mot de passe réinitialisé, mais la config n'a pas pu être écrite : {e} — les connexions existantes sont cassées ; relance connect_database (le nouveau mot de passe est lisible)")
                } else {
                    e
                }
            })?;
        out["written"] = json!(true);
        out["rotated"] = json!(rotated);
        if rotated {
            out["database"]["password"] =
                json!("réinitialisé puis écrit directement, jamais affiché");
        }
        if let Some(g) = guard {
            notes.push(scrub(&g, &secrets));
        }
        out["next"] = json!(format!(
            "config écrite (étape annulable avec rollback_changeset{}) : redémarre le serveur (power_action restart) pour que le plugin se connecte, puis vérifie la console (error_digest ou wait_for)",
            if rotated { " ; le mot de passe réinitialisé, lui, ne revient pas en arrière" } else { "" }
        ));
        if !notes.is_empty() {
            out["notes"] = json!(notes);
        }
        let text = config_edit::render_for_ai(core, out);
        return Ok(scrub(&text, &secrets));
    }
    if !notes.is_empty() {
        out["notes"] = json!(notes);
    }
    let text = config_edit::render_for_ai(core, out);
    Ok(scrub(&text, &secrets_now))
}

// --- Suppression d'une base (`delete_database`) ------------------------------------------------------

/// Arguments validés de `delete_database`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DeleteRequest {
    pub mybox_id: i64,
    pub database_id: i64,
    pub confirm_name: Option<String>,
}

impl DeleteRequest {
    pub(crate) fn parse(args: &Value) -> Result<Self, String> {
        Ok(DeleteRequest {
            mybox_id: int_arg(args, "mybox_id")?.ok_or("paramètre `mybox_id` (entier) requis")?,
            database_id: int_arg(args, "database_id")?
                .ok_or("paramètre `database_id` (entier) requis (voir get_mybox → databases, ou l'erreur de connect_database)")?,
            confirm_name: args
                .get("confirm_name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        })
    }
}

/// Nom exact d'une base (`dbname`) : ce que l'utilisateur retape pour confirmer.
pub(crate) fn database_name(entry: &Value) -> Option<String> {
    pick(&leaves(entry), NAME_KEYS)
}

/// La confirmation reprend-elle exactement le nom de la base (espaces de bord ignorés, casse
/// comprise) ?
pub(crate) fn delete_confirmed(expected: &str, given: &str) -> bool {
    let expected = expected.trim();
    !expected.is_empty() && given.trim() == expected
}

/// Message d'une confirmation manquante : ne révèle jamais le nom attendu.
const DELETE_CONFIRM_MISSING: &str = "Confirmation requise : `confirm_name` = nom exact de la base (son `dbname`, tel qu'affiché dans le panel → MyBox → Bases de données), donné par l'utilisateur — jamais deviné. Rien n'a été supprimé. Avant de supprimer : sauvegardes de la base avec l'outil officiel list_db_backups.";

/// La base est-elle encore listée par `GET /mybox/{id}` ?
async fn database_listed(core: &Core, mybox_id: i64, database_id: i64) -> Result<bool, String> {
    let token = core.token().map_err(|e| e.to_string())?;
    let data = core
        .api
        .get_data(&token, &format!("/mybox/{mybox_id}"))
        .await
        .map_err(|e| format!("MyBox {mybox_id} illisible : {e}"))?;
    Ok(database_entries(&data)
        .iter()
        .any(|e| entry_id(e) == Some(database_id)))
}

/// Outil MCP `delete_database` : supprime DÉFINITIVEMENT une base de données d'une MyBox, après
/// vérification qu'elle existe et confirmation de son nom exact par l'humain (élicitation) ou par
/// `confirm_name`. Le mot de passe de la base n'apparaît jamais.
pub(crate) async fn delete_database_tool(core: &Core, args: &Value) -> Result<String, String> {
    let req = DeleteRequest::parse(args)?;
    let (mybox_id, id) = (req.mybox_id, req.database_id);
    let token = core.token().map_err(|e| e.to_string())?;
    let data = core
        .api
        .get_data(&token, &format!("/mybox/{mybox_id}"))
        .await
        .map_err(|e| format!("MyBox {mybox_id} illisible : {e}"))?;
    let entries = database_entries(&data);
    let entry = select_entry(&entries, Some(id), mybox_id).map_err(|e| {
        if entries.is_empty() {
            format!("La MyBox {mybox_id} n'a aucune base de données : rien à supprimer.")
        } else {
            e
        }
    })?;
    let name = database_name(entry).ok_or_else(|| {
        format!(
            "nom de la base {id} illisible dans la réponse de l'API (champs vus : {}) : confirmation impossible, rien n'est supprimé",
            field_names(entry)
        )
    })?;
    // Lecture tolérante : de quoi décrire la base (jamais le mot de passe) et nettoyer les sorties.
    let creds = read_creds(entry).ok();
    let password = creds
        .as_ref()
        .and_then(|c| c.password.clone())
        .unwrap_or_default();
    let secrets = [password.as_str()];
    let label = creds.as_ref().and_then(|c| c.label.clone());
    let host = creds.as_ref().map(|c| c.host.clone());

    // Confirmation AVANT tout effet (seules des lectures précèdent : rejouables sans risque).
    let message = format!(
        "Suppression DÉFINITIVE de la base de données {id}{} de la MyBox {mybox_id} : toutes ses tables et données sont perdues, et tout ce qui s'y connecte (plugins, sites) cessera de fonctionner. Sauvegardes éventuelles : outil officiel list_db_backups.",
        label.as_deref().map(|l| format!(" (« {l} »)")).unwrap_or_default()
    );
    let expected = name.clone();
    let typed = mcp_ctx::confirm_or_given(&message, || async move { Ok(expected) }, req.confirm_name.clone())
        .await
        .map_err(|e| {
            if e.contains("requis") {
                DELETE_CONFIRM_MISSING.to_string()
            } else {
                e
            }
        })?;
    if !delete_confirmed(&name, &typed) {
        return Err("Refusé : `confirm_name` ne correspond pas au nom exact de la base (casse comprise). Rien n'a été supprimé.".into());
    }

    let job = core
        .api
        .delete_mybox_database(&token, mybox_id, id)
        .await
        .map_err(|e| scrub(&format!("suppression de la base {id} refusée par l'API : {e}"), &secrets))?;
    core.cache().clear();
    // Suppression asynchrone : la base doit disparaître de la MyBox (relevé : immédiat).
    let mut verified = None;
    for attempt in 0..4 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        match database_listed(core, mybox_id, id).await {
            Ok(false) => {
                verified = Some(true);
                break;
            }
            Ok(true) => verified = Some(false),
            Err(_) => {}
        }
    }
    let out = json!({
        "mybox_id": mybox_id,
        "database": { "id": id, "name": name, "label": label, "host": host },
        "deleted": true,
        "job_id": job,
        "verified": verified,
        "note": match verified {
            Some(true) => "la base n'est plus listée dans la MyBox",
            Some(false) => "la base est encore listée (suppression asynchrone en cours) : vérifie dans quelques instants (mybox_info ou connect_database)",
            None => "MyBox illisible après la suppression : vérification impossible",
        },
        "undo": "irréversible : seule une sauvegarde automatique (outil officiel list_db_backups / restore_db_backup, si la base en avait) permet de retrouver des données",
        "next": "retire les accès de cette base des configs des plugins qui l'utilisaient (get_config / set_config), sinon ils échoueront au démarrage",
    });
    let text = config_edit::render_for_ai(core, out);
    Ok(scrub(&text, &secrets))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "S3cr3t-Pa55w0rd!";

    fn entry() -> Value {
        json!({
            "id": 12,
            "description": "DB LuckPerms",
            "engine": "mariadb118",
            "host": "db1.minestrator.example",
            "port": 3306,
            "database": "s12_luckperms",
            "username": "u12_lp",
            "password": SECRET,
        })
    }

    fn creds() -> Creds {
        read_creds(&entry()).unwrap()
    }

    /// Forme réelle d'une entrée de `GET /mybox/{id}` → `databases[]` (base MariaDB 11.8 créée
    /// pour l'essai du 2026-11-19 ; valeurs remplacées).
    #[test]
    fn real_api_entry_is_read_and_its_engine_comes_from_the_host() {
        let c = read_creds(&json!({
            "id": 7395,
            "dbname": "minesr_abcdefgh",
            "description": "",
            "host": "mariadb1.mystrator.com",
            "port": 3306,
            "username": "minesr_abcdefgh",
            "password": SECRET,
        }))
        .unwrap();
        assert_eq!((c.id, c.engine, c.port, c.port_assumed), (7395, Engine::MariaDb, 3306, false));
        assert_eq!((c.database.as_str(), c.username.as_str()), ("minesr_abcdefgh", "minesr_abcdefgh"));
        assert_eq!(c.password.as_deref(), Some(SECRET));
        assert_eq!(c.label, None, "description vide : pas de libellé");
        // Hôte muet : moteur inconnu, comme avant.
        let quiet = read_creds(&json!({ "id": 1, "dbname": "d", "host": "db1.example", "username": "u", "password": SECRET })).unwrap();
        assert_eq!(quiet.engine, Engine::Unknown);
    }

    #[test]
    fn credentials_are_read_with_tolerance() {
        let c = creds();
        assert_eq!(
            (c.id, c.engine, c.port, c.port_assumed),
            (12, Engine::MariaDb, 3306, false)
        );
        assert_eq!(
            (c.host.as_str(), c.database.as_str(), c.username.as_str()),
            ("db1.minestrator.example", "s12_luckperms", "u12_lp")
        );
        assert_eq!(c.password.as_deref(), Some(SECRET));
        // Autres noms, hôte:port, port en texte, imbrication d'un niveau, mot de passe masqué.
        let other = read_creds(&json!({
            "id_database": "7",
            "name": "s7_coreprotect",
            "type": "MySQL 8.4",
            "access": { "hostname": "10.0.0.5:3307", "user": "u7", "password": "********" }
        }))
        .unwrap();
        assert_eq!(
            (other.id, other.engine, other.port),
            (7, Engine::MySql, 3307)
        );
        assert_eq!(
            (
                other.host.as_str(),
                other.database.as_str(),
                other.username.as_str()
            ),
            ("10.0.0.5", "s7_coreprotect", "u7")
        );
        assert_eq!(other.password, None, "masqué = non lisible");
        // Sans port : 3306 supposé, signalé.
        let assumed =
            read_creds(&json!({ "id": 1, "host": "h", "db_name": "d", "login": "u" })).unwrap();
        assert!(assumed.port_assumed && assumed.port == 3306);
        // Le Debug ne montre jamais le mot de passe.
        assert!(!format!("{:?}", creds()).contains(SECRET));
    }

    #[test]
    fn missing_fields_are_reported_by_name_never_by_value() {
        let e = read_creds(
            &json!({ "id": 3, "label": "valeur-visible", "pwd": SECRET, "meta": { "size": 10 } }),
        )
        .unwrap_err();
        assert!(e.contains("hôte") && e.contains("utilisateur"), "{e}");
        assert!(
            e.contains("label") && e.contains("pwd") && e.contains("meta.size"),
            "noms vus : {e}"
        );
        assert!(
            !e.contains(SECRET) && !e.contains("valeur-visible"),
            "jamais les valeurs : {e}"
        );
    }

    #[test]
    fn databases_are_selected_or_listed() {
        let data = json!({ "databases": [entry(), { "id": 13, "database": "s13_plan" }] });
        let entries = database_entries(&data);
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entry_id(select_entry(&entries, Some(13), 4).unwrap()),
            Some(13)
        );
        let e = select_entry(&entries, None, 4).unwrap_err();
        assert!(
            e.contains("12 (s12_luckperms)") && e.contains("13 (s13_plan)"),
            "{e}"
        );
        assert!(!e.contains(SECRET));
        assert!(select_entry(&entries, Some(99), 4)
            .unwrap_err()
            .contains("introuvable"));
        // Objet indexé par identifiant, et une seule base : choisie sans identifiant.
        let one = json!({ "databases": { "12": entry() } });
        assert_eq!(
            entry_id(select_entry(&database_entries(&one), None, 4).unwrap()),
            Some(12)
        );
        // Aucune base : renvoi vers create_database, jamais de création.
        let none =
            select_entry(&database_entries(&json!({ "databases": [] })), None, 4).unwrap_err();
        assert!(none.contains("create_database") && none.contains("panel"));
    }

    #[test]
    fn presets_write_the_documented_keys() {
        let c = creds();
        let (path, ch) = preset_changes("luckperms", &c, SECRET).unwrap();
        assert_eq!(path, "/plugins/LuckPerms/config.yml");
        let get = |ch: &[(String, Value)], k: &str| {
            ch.iter().find(|(key, _)| key == k).map(|(_, v)| v.clone())
        };
        assert_eq!(get(&ch, "storage-method"), Some(json!("mariadb")));
        assert_eq!(
            get(&ch, "data.address"),
            Some(json!("db1.minestrator.example:3306"))
        );
        assert_eq!(get(&ch, "data.password"), Some(json!(SECRET)));
        let (_, ch) = preset_changes("coreprotect", &c, SECRET).unwrap();
        assert_eq!(
            (get(&ch, "use-mysql"), get(&ch, "mysql-port")),
            (Some(json!(true)), Some(json!(3306)))
        );
        let (_, ch) = preset_changes(
            "litebans",
            &Creds {
                engine: Engine::MySql,
                ..c.clone()
            },
            SECRET,
        )
        .unwrap();
        assert_eq!(get(&ch, "sql.driver"), Some(json!("MySQL")));
        let (path, ch) = preset_changes("plan", &c, SECRET).unwrap();
        assert_eq!(
            (path, get(&ch, "Database.Type")),
            ("/plugins/Plan/config.yml", Some(json!("MySQL")))
        );
        // Chaque clé de mot de passe d'un préréglage est bien secrète pour config_edit (masquée).
        for (name, path, _) in PRESETS {
            let format = config_edit::Format::detect(path).unwrap();
            let (_, ch) = preset_changes(name, &c, SECRET).unwrap();
            let secret_keys: Vec<&String> = ch
                .iter()
                .filter(|(k, _)| config_edit::path_is_secret(&format.parse_key(k).unwrap()))
                .map(|(k, _)| k)
                .collect();
            assert_eq!(
                secret_keys.len(),
                1,
                "{name} : une seule clé secrète, celle du mot de passe"
            );
            assert_eq!(get(&ch, secret_keys[0]), Some(json!(SECRET)), "{name}");
        }
        assert!(preset_changes("inconnu", &c, SECRET)
            .unwrap_err()
            .contains("luckperms"));
        assert!(preset_changes(
            "luckperms",
            &Creds {
                engine: Engine::Other,
                ..c
            },
            SECRET
        )
        .is_err());
    }

    #[test]
    fn explicit_mappings_refuse_a_password_key_that_would_show() {
        let c = creds();
        let keys = |v: Value| v.as_object().unwrap().clone();
        let ok = mapping_changes(
            "/plugins/X/config.yml",
            &keys(json!({ "address": "db.url", "username": "db.user", "password": "db.pass" })),
            Some(&keys(json!({ "db.type": "mysql" }))),
            &c,
            SECRET,
        )
        .unwrap();
        assert_eq!(ok.len(), 4);
        let shown = mapping_changes(
            "/plugins/X/config.yml",
            &keys(json!({ "password": "db.credential_value" })),
            None,
            &c,
            SECRET,
        );
        assert!(shown.is_ok(), "credential = secret");
        let refused = mapping_changes(
            "/plugins/X/config.yml",
            &keys(json!({ "password": "db.motdepasse" })),
            None,
            &c,
            SECRET,
        )
        .unwrap_err();
        assert!(refused.contains("non reconnue comme secrète") && !refused.contains(SECRET));
        assert!(
            mapping_changes("/x.yml", &keys(json!({ "pseudo": "a" })), None, &c, SECRET)
                .unwrap_err()
                .contains("inconnu")
        );
        assert!(mapping_changes(
            "/x.yml",
            &keys(json!({ "host": "a" })),
            Some(&keys(json!({ "a.password": "x" }))),
            &c,
            SECRET
        )
        .is_err());
        assert!(mapping_changes(
            "/x.yml",
            &keys(json!({ "host": "a", "database": "a" })),
            None,
            &c,
            SECRET
        )
        .unwrap_err()
        .contains("deux fois"));
        assert!(
            mapping_changes("/x.ini", &keys(json!({ "host": "a" })), None, &c, SECRET).is_err()
        );
    }

    /// Le vrai mot de passe passe dans `set_config` : ni la sortie (changements) ni le diff ne
    /// doivent le contenir, pour chaque préréglage, sur un fichier réaliste.
    #[test]
    fn the_password_never_shows_in_the_edit_output_or_diff() {
        let files = [
            ("luckperms", "storage-method: h2\ndata:\n  address: localhost\n  database: minecraft\n  username: root\n  password: ''\n"),
            ("coreprotect", "use-mysql: false\ntable-prefix: co_\nmysql-host: 127.0.0.1\nmysql-port: 3306\nmysql-database: database\nmysql-username: root\nmysql-password: ''\n"),
            ("litebans", "sql:\n  driver: H2\n  address: localhost:3306\n  database: 'litebans'\n  username: 'root'\n  password: ''\n"),
            ("plan", "Database:\n    Type: SQLite\n    MySQL:\n        Host: localhost\n        User: root\n        Password: password\n        Database: Plan\n        Port: 3306\n"),
        ];
        let c = creds();
        for (name, text) in files {
            let (path, ch) = preset_changes(name, &c, SECRET).unwrap();
            let requests: Vec<config_edit::EditRequest> = ch
                .iter()
                .map(|(k, v)| config_edit::EditRequest {
                    key: k.clone(),
                    value: v.clone(),
                })
                .collect();
            let opts = config_edit::EditOptions {
                create_missing: false,
                ..Default::default()
            };
            let format = config_edit::Format::detect(path).unwrap();
            let outcome =
                config_edit::edit_text(format, path, text, &requests, opts, &|s: &str| {
                    s.to_string()
                })
                .unwrap_or_else(|e| panic!("{name} : {e}"));
            assert!(
                outcome.changed && outcome.new_text.contains(SECRET),
                "{name} : écrit dans le fichier"
            );
            let shown =
                serde_json::to_string(&config_edit::render_outcome(path, &outcome, false, true))
                    .unwrap();
            assert!(
                !shown.contains(SECRET),
                "{name} : mot de passe visible dans la sortie : {shown}"
            );
            assert!(
                !outcome.diff.contains(SECRET),
                "{name} : mot de passe dans le diff"
            );
            assert!(
                shown.contains(&c.username),
                "{name} : le reste reste lisible"
            );
        }
    }

    #[test]
    fn requests_need_a_target_and_default_to_a_dry_run() {
        let r =
            Request::parse(&json!({ "server_id": 4, "target": "LuckPerms", "database_id": 12 }))
                .unwrap();
        assert!(r.dry_run && !r.rotate_password);
        assert_eq!(
            (r.target.as_deref(), r.database_id),
            (Some("luckperms"), Some(12))
        );
        let err = |v: Value| Request::parse(&v).unwrap_err();
        assert!(err(json!({ "server_id": 4 })).contains("cible requise"));
        assert!(err(json!({ "server_id": 4, "keys": { "host": "a" } })).contains("path"));
        assert!(
            err(json!({ "server_id": 4, "target": "plan", "keys": { "host": "a" } }))
                .contains("pas les deux")
        );
        assert!(
            err(json!({ "server_id": 4, "target": "plan", "set": { "a": 1 } })).contains("set")
        );
        assert!(
            err(json!({ "server_id": 4, "target": "plan", "database_id": "x" }))
                .contains("database_id")
        );
        let explicit = Request::parse(
            &json!({ "server_id": 4, "path": "/a.yml", "keys": { "host": "h" }, "dry_run": false }),
        )
        .unwrap();
        assert!(!explicit.dry_run);
        let (path, ch, create) = explicit.changes(&creds(), SECRET).unwrap();
        assert_eq!((path.as_str(), ch.len(), create), ("/a.yml", 1, true));
        let (path, _, create) = r.changes(&creds(), SECRET).unwrap();
        assert_eq!(
            (path.as_str(), create),
            ("/plugins/LuckPerms/config.yml", false),
            "préréglage : clés jamais créées"
        );
    }

    #[test]
    fn secrets_are_scrubbed_from_any_text() {
        assert_eq!(
            scrub(&format!("erreur : valeur « {SECRET} » refusée"), &[SECRET]),
            "erreur : valeur « [SECRET] » refusée"
        );
        assert_eq!(scrub("rien", &[]), "rien");
        assert_eq!(
            scrub("ab", &["ab"]),
            "ab",
            "trop court pour être cherché sans faux positifs"
        );
        assert!(usable_password(SECRET));
        assert!(!usable_password("••••••") && !usable_password("[SECRET]") && !usable_password(""));
        assert_eq!(
            json_in_text("Voici : {\"databases\": []} fin").unwrap(),
            json!({ "databases": [] })
        );
    }

    /// `delete_database` : la base visée est retrouvée par son identifiant dans la vraie forme de
    /// `GET /mybox/{id}` → `databases[]` (relevé du 2026-11-19), son nom exact (`dbname`) est ce
    /// que l'utilisateur retape ; le refus ne révèle jamais le nom ni le mot de passe.
    #[test]
    fn database_deletion_needs_the_exact_dbname() {
        let data = json!({ "databases": [
            { "id": 7395, "dbname": "minesr_abcdefgh", "description": "", "host": "mariadb1.mystrator.com", "port": 3306, "username": "minesr_abcdefgh", "password": SECRET },
            { "id": 7396, "dbname": "minesr_zyxwvuts", "description": "Plan", "host": "mariadb1.mystrator.com", "port": 3306, "username": "minesr_zyxwvuts", "password": SECRET }
        ] });
        let entries = database_entries(&data);
        let entry = select_entry(&entries, Some(7396), 111).unwrap();
        assert_eq!(database_name(entry).as_deref(), Some("minesr_zyxwvuts"));
        assert!(select_entry(&entries, Some(1), 111).unwrap_err().contains("introuvable"));
        assert!(delete_confirmed("minesr_zyxwvuts", " minesr_zyxwvuts "));
        assert!(!delete_confirmed("minesr_zyxwvuts", "MINESR_ZYXWVUTS"), "casse comprise");
        assert!(!delete_confirmed("minesr_zyxwvuts", "Plan"), "le libellé n'est pas le nom");
        assert!(!delete_confirmed("", ""));
        assert!(!DELETE_CONFIRM_MISSING.contains("minesr") && DELETE_CONFIRM_MISSING.contains("list_db_backups"));
        let r = DeleteRequest::parse(&json!({ "mybox_id": "111", "database_id": 7396, "confirm_name": "  minesr_zyxwvuts " })).unwrap();
        assert_eq!((r.mybox_id, r.database_id, r.confirm_name.as_deref()), (111, 7396, Some("minesr_zyxwvuts")));
        assert!(DeleteRequest::parse(&json!({ "mybox_id": 111 })).unwrap_err().contains("database_id"));
        assert!(DeleteRequest::parse(&json!({ "database_id": 1 })).unwrap_err().contains("mybox_id"));
        assert_eq!(DeleteRequest::parse(&json!({ "mybox_id": 1, "database_id": 2, "confirm_name": " " })).unwrap().confirm_name, None);
    }

    /// Hors MCP (pas d'élicitation) : sans `confirm_name`, la confirmation échoue avant tout effet.
    #[tokio::test]
    async fn database_deletion_without_confirmation_is_refused() {
        let out = mcp_ctx::confirm_or_given("x", || async { Ok("minesr_x".to_string()) }, None).await;
        assert!(out.unwrap_err().contains("requis"), "remplacé par DELETE_CONFIRM_MISSING dans l'outil");
    }
}
