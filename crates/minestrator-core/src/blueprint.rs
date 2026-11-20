//! **Monter un réseau entier depuis un fichier** (« blueprint ») : un proxy Velocity ou BungeeCord
//! et ses serveurs de jeu, décrits en un seul document JSON, montés sur une MyBox sans le panel.
//!
//! - [`plan_network_tool`] (lecture) : rapproche chaque élément du compte, vérifie la place libre de
//!   la MyBox et calcule, élément par élément, ce qu'il reste à faire (`ok`, `create`, `change`,
//!   `manual`, `blocked`) en simulant les outils élémentaires (`set_server_platform`,
//!   `plan_manifest`, `sync_dir`, `upload_world`, `link_proxy`, `network_preset`) ;
//! - [`apply_network_tool`] (modifiant) : `dry_run` vrai par défaut ; sinon confirmation de
//!   l'humain avant tout effet, puis les phases dans l'ordre, chaque écriture passant par
//!   `mcp::dispatch` (une étape de la garde des agents par sous-écriture ; l'outil composite n'en a
//!   pas, `Plan::Skip`). Idempotent : relancé, il saute ce qui est déjà conforme ;
//! - [`export_network_tool`] (lecture) : blueprint d'un réseau existant à partir de son proxy ;
//! - [`network_template_tool`] (lecture) : modèles pré-remplis, ressources réparties selon la place
//!   libre d'une MyBox.
//!
//! Rien n'est réimplémenté : la création passe par `mybox_create_server`, la plateforme par la
//! logique de `set_server_platform` (`platform.rs`), le contenu et les configs par
//! `apply_manifest`, puis `sync_dir`, `upload_world`, `link_proxy`, `network_preset`,
//! `pregenerate_world`, `network_topology` et `ping_server`. Format détaillé dans `MCP.md`
//! (« Monter un réseau entier »).

mod apply;
mod export;
mod plan;
#[cfg(test)]
mod tests;

pub(crate) use apply::apply_network_tool;
pub(crate) use export::{export_network_tool, network_template_tool, template_names};
pub(crate) use plan::plan_network_tool;

use crate::manifest::Manifest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

/// Version du format.
pub(crate) const BLUEPRINT_VERSION: i64 = 1;
/// Serveurs de jeu au plus (hors proxy).
pub(crate) const MAX_SERVERS: usize = 20;
/// Synchronisations au plus par serveur.
const MAX_SYNCS: usize = 10;
/// Hôtes forcés au plus.
const MAX_FORCED_HOSTS: usize = 50;
/// Plateformes de proxy (nom normalisé, comme `set_server_platform`).
pub(crate) const PROXY_PLATFORMS: &[&str] = &["velocity", "bungeecord", "waterfall"];
/// Préréglages de réseau (`network_preset`).
pub(crate) const PRESETS: &[&str] = &["luckperms", "geyser"];
/// Marqueur posé à la racine d'un serveur par `apply_network` : monde installé (empreinte de la
/// source) et prégénération lancée. C'est lui qui rend ces deux étapes idempotentes.
pub(crate) const MARKER_PATH: &str = "/.minestrator-blueprint.json";

// --- Format v1 -----------------------------------------------------------------------------------

/// Blueprint v1 : un réseau (proxy facultatif) et ses serveurs, dans une MyBox.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Blueprint {
    pub version: i64,
    /// Libellé du réseau (affichage seulement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// MyBox où rapprocher (par nom) et créer les serveurs.
    pub mybox_id: i64,
    /// Jeu des serveurs créés (défaut : l'egg « Minecraft » du catalogue).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub egg_id: Option<i64>,
    /// Localisation des serveurs créés (défaut : celle des serveurs de la MyBox, sinon la première
    /// en stock ; ignorée pour une MyBox Pro).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxySpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<NodeSpec>,
    /// Préréglages de réseau : `luckperms`, `geyser`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<String>,
    /// Base de données partagée (préréglage `luckperms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<DatabaseSpec>,
}

/// Le proxy du réseau.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProxySpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<i64>,
    /// `velocity` (défaut), `bungeecord` ou `waterfall`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Version du proxy (Velocity : `3.4.0`…) ; absente = celle en place, ou la dernière.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram: Option<i64>,
    /// Transfert : `modern` (Velocity, défaut) ou `legacy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Domaine → nom (ou liste de noms) de serveurs du blueprint.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub forced_hosts: Map<String, Value>,
    /// Liste de repli : noms de serveurs du blueprint, dans l'ordre.
    #[serde(default, rename = "try", skip_serializing_if = "Vec::is_empty")]
    pub try_order: Vec<String>,
    /// Retirer les entrées du proxy qui ne mènent à aucun serveur (exemples de Velocity).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub prune: bool,
    /// Port UDP du proxy pour Bedrock (préréglage `geyser`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bedrock_port: Option<u16>,
    /// Manifeste v1 du proxy, sans section `server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Value>,
    /// Dossier(s) de l'ordinateur synchronisé(s) : objet ou liste.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<Value>,
}

/// Un serveur de jeu du réseau.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct NodeSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<i64>,
    /// Rôle libre (`lobby`, `survival`, `creative`…) ; `lobby` passe en tête de la liste de repli.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Nom de son entrée sur le proxy (défaut : celui que choisit `link_proxy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// `paper`, `spigot`, `purpur`, `folia`, `fabric`, `forge`, `neoforge`, `vanilla`…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// Version de Minecraft ; absente = celle en place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram: Option<i64>,
    /// Manifeste v1 du serveur (`properties`, `content`, `configs`, `files`, `players`, `startup`,
    /// `schedules`), sans section `server`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<WorldSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pregenerate: Option<PregenSpec>,
}

/// Synchronisation d'un dossier de l'ordinateur (`sync_dir`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SyncSpec {
    pub local_dir: String,
    pub remote_dir: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delete_extra: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// Monde de l'ordinateur installé une fois (`upload_world`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorldSpec {
    pub local_path: String,
    /// Dossier du monde sur le serveur (défaut : `level-name`) ; donné, il devient `level-name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world_name: Option<String>,
    /// Remplacer un monde existant que le blueprint n'a pas installé (l'ancien est renommé).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}

/// Prégénération (`pregenerate_world`, Chunky).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PregenSpec {
    pub radius: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<String>,
}

/// Base partagée (préréglage `luckperms`).
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DatabaseSpec {
    pub database_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mybox_id: Option<i64>,
}

// --- Réseau validé -------------------------------------------------------------------------------

/// Un serveur du réseau (proxy compris), validé.
#[derive(Debug, Clone)]
pub(crate) struct Node {
    /// Chemin dans le blueprint (`proxy`, `servers[2]`) : cité dans les erreurs.
    pub path: String,
    pub name: String,
    pub proxy: bool,
    pub server_id: Option<i64>,
    pub role: Option<String>,
    pub entry: Option<String>,
    /// Nom normalisé (`paper`, `velocity`…).
    pub platform: Option<String>,
    pub version: Option<String>,
    pub cpu: Option<i64>,
    pub ram: Option<i64>,
    /// Manifeste donné (version 1, sans `server`).
    pub manifest: Option<Manifest>,
    pub syncs: Vec<SyncSpec>,
    pub world: Option<WorldSpec>,
    pub pregen: Option<PregenSpec>,
}

impl Node {
    pub(crate) fn label(&self) -> String {
        format!("« {} »", self.name)
    }
}

/// Réglages du proxy, références résolues en indices de [`Network::nodes`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ProxyOpts {
    pub mode: Option<String>,
    pub forced_hosts: Vec<(String, Vec<usize>)>,
    pub try_order: Vec<usize>,
    pub prune: bool,
    pub bedrock_port: Option<u16>,
}

/// Blueprint validé : le proxy (s'il y en a un) en premier dans `nodes`.
#[derive(Debug, Clone)]
pub(crate) struct Network {
    pub name: Option<String>,
    pub mybox_id: i64,
    pub egg_id: Option<i64>,
    pub location_id: Option<i64>,
    pub nodes: Vec<Node>,
    pub proxy: Option<ProxyOpts>,
    pub presets: Vec<String>,
    pub database: Option<DatabaseSpec>,
}

impl Network {
    /// Indice du proxy.
    pub(crate) fn proxy_index(&self) -> Option<usize> {
        self.nodes.iter().position(|n| n.proxy)
    }

    /// Serveurs de jeu dans l'ordre de liaison : rôle `lobby` d'abord (premier de la liste de repli
    /// par défaut), puis l'ordre du blueprint.
    pub(crate) fn backends(&self) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.nodes.len()).filter(|i| !self.nodes[*i].proxy).collect();
        v.sort_by_key(|i| !self.nodes[*i].role.as_deref().is_some_and(|r| r == "lobby"));
        v
    }

    pub(crate) fn label(&self) -> String {
        match &self.name {
            Some(n) => format!("« {n} »"),
            None => format!("de la MyBox {}", self.mybox_id),
        }
    }
}

fn err(path: &str, msg: impl std::fmt::Display) -> String {
    format!("blueprint : `{path}` : {msg}")
}

/// Nom de plateforme normalisé et vérifié (`Paper` → `paper`).
fn platform_of(path: &str, raw: &str, proxy: bool) -> Result<String, String> {
    let p = crate::platform::normalize_platform(raw);
    if p.is_empty() || p.len() > 32 || !p.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(err(path, format!("plateforme « {raw} » illisible")));
    }
    if p == "quilt" {
        return Err(err(path, "Quilt n'a ni entrée au catalogue de l'hébergeur ni JAR unique : prends Fabric, ou passe par le panel"));
    }
    let is_proxy = PROXY_PLATFORMS.contains(&p.as_str());
    if proxy && !is_proxy {
        return Err(err(path, format!("« {raw} » n'est pas une plateforme de proxy (velocity, bungeecord, waterfall)")));
    }
    if !proxy && is_proxy {
        return Err(err(path, format!("« {raw} » est un proxy : un seul proxy par réseau, décrit dans `proxy`")));
    }
    Ok(p)
}

fn version_of(path: &str, raw: &Option<String>) -> Result<Option<String>, String> {
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) if v.len() <= 32 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+' | '_')) => {
            Ok(Some(v.to_string()))
        }
        Some(v) => Err(err(path, format!("version « {v} » illisible (ex. 1.21.4, 26.1.2, 3.4.0)"))),
    }
}

fn resources(path: &str, cpu: Option<i64>, ram: Option<i64>) -> Result<(), String> {
    for (k, v) in [("cpu", cpu), ("ram", ram)] {
        if let Some(n) = v {
            if !(1..=1024).contains(&n) {
                return Err(err(&format!("{path}.{k}"), format!("{n} : entier d'au moins 1 attendu (CPU en cœurs entiers, RAM en Go entiers)")));
            }
        }
    }
    Ok(())
}

/// Manifeste inclus : objet, version 1 (facultative), sans section `server` (nom, plateforme et
/// version sont ceux du blueprint).
fn manifest_of(path: &str, raw: Option<&Value>) -> Result<Option<Manifest>, String> {
    let Some(v) = raw else { return Ok(None) };
    let mut obj = match v {
        Value::Object(m) => m.clone(),
        Value::Null => return Ok(None),
        _ => return Err(err(path, "objet attendu (manifeste v1 sans `server`)")),
    };
    if obj.contains_key("server") {
        return Err(err(path, "section `server` refusée : nom, plateforme et version se donnent au niveau du serveur dans le blueprint (`name`, `platform`, `version`)"));
    }
    match obj.get("version") {
        None => {
            obj.insert("version".into(), json!(crate::manifest::MANIFEST_VERSION));
        }
        Some(Value::Number(n)) if n.as_i64() == Some(crate::manifest::MANIFEST_VERSION) => {}
        Some(other) => return Err(err(&format!("{path}.version"), format!("{other} : seule la version {} existe", crate::manifest::MANIFEST_VERSION))),
    }
    Manifest::parse(&Value::Object(obj)).map(Some).map_err(|e| err(path, e))
}

fn syncs_of(path: &str, raw: Option<&Value>) -> Result<Vec<SyncSpec>, String> {
    let list: Vec<Value> = match raw {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a.clone(),
        Some(v @ Value::Object(_)) => vec![v.clone()],
        Some(_) => return Err(err(path, "objet { local_dir, remote_dir, delete_extra? } ou liste de tels objets attendu")),
    };
    if list.len() > MAX_SYNCS {
        return Err(err(path, format!("{} synchronisations, {MAX_SYNCS} au plus", list.len())));
    }
    let mut out = Vec::new();
    for (i, v) in list.into_iter().enumerate() {
        let p = format!("{path}[{i}]");
        let s: SyncSpec = serde_json::from_value(v).map_err(|e| err(&p, e))?;
        // Mêmes règles que `sync_dir` (chemin absolu, dossier distant, delete_extra hors racine, motifs).
        crate::local_sync::parse_request(&sync_args(0, &s, true)).map_err(|e| err(&p, e))?;
        out.push(s);
    }
    Ok(out)
}

/// Arguments de `sync_dir` pour une synchronisation.
pub(crate) fn sync_args(server_id: i64, s: &SyncSpec, dry_run: bool) -> Value {
    json!({
        "server_id": server_id,
        "local_dir": s.local_dir,
        "remote_dir": s.remote_dir,
        "delete_extra": s.delete_extra,
        "include": s.include,
        "exclude": s.exclude,
        "dry_run": dry_run,
    })
}

/// Arguments de `pregenerate_world start`.
pub(crate) fn pregen_args(server_id: i64, p: &PregenSpec) -> Value {
    let mut a = json!({ "server_id": server_id, "action": "start", "radius": p.radius });
    if let Some(w) = &p.world {
        a["world"] = json!(w);
    }
    if let Some(c) = &p.center {
        a["center"] = c.clone();
    }
    if let Some(s) = &p.shape {
        a["shape"] = json!(s);
    }
    a
}

/// Nom d'un rôle : `[a-z0-9_-]`, 32 caractères au plus.
fn role_of(path: &str, raw: &Option<String>) -> Result<Option<String>, String> {
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(r) if r.len() <= 32 && r.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_') => {
            Ok(Some(r.to_string()))
        }
        Some(r) => Err(err(path, format!("rôle « {r} » : minuscules, chiffres, - et _ (lobby, survival, creative…)"))),
    }
}

impl Network {
    /// Lit un blueprint (objet, ou texte JSON) et vérifie tout ce qui peut l'être sans réseau :
    /// champs inconnus, noms, plateformes, chemins, manifestes, références entre serveurs.
    pub(crate) fn parse(v: &Value) -> Result<Self, String> {
        let v = match v {
            Value::String(s) => serde_json::from_str::<Value>(s).map_err(|e| format!("blueprint : JSON invalide ({e})"))?,
            other => other.clone(),
        };
        if !v.is_object() {
            return Err("blueprint : objet JSON attendu".into());
        }
        let bp: Blueprint = serde_json::from_value(v).map_err(|e| format!("blueprint invalide : {e}"))?;
        if bp.version != BLUEPRINT_VERSION {
            return Err(format!("blueprint : version {} non prise en charge (seule la version {BLUEPRINT_VERSION} existe)", bp.version));
        }
        if bp.mybox_id <= 0 {
            return Err(err("mybox_id", "identifiant de MyBox attendu (list_servers, mybox_capacity)"));
        }
        if let Some(n) = &bp.name {
            if n.trim().is_empty() || n.chars().count() > 80 || n.chars().any(char::is_control) {
                return Err(err("name", "libellé de 1 à 80 caractères, sur une ligne"));
            }
        }
        if bp.servers.is_empty() {
            return Err(err("servers", "au moins un serveur de jeu attendu"));
        }
        if bp.servers.len() > MAX_SERVERS {
            return Err(err("servers", format!("{} serveurs, {MAX_SERVERS} au plus", bp.servers.len())));
        }
        let mut nodes: Vec<Node> = Vec::new();
        if let Some(p) = &bp.proxy {
            let path = "proxy";
            resources(path, p.cpu, p.ram)?;
            let platform = platform_of(&format!("{path}.platform"), p.platform.as_deref().unwrap_or("velocity"), true)?;
            nodes.push(Node {
                path: path.into(),
                name: crate::mybox::valid_name(&p.name).map_err(|e| err(&format!("{path}.name"), e))?,
                proxy: true,
                server_id: p.server_id,
                role: None,
                entry: None,
                platform: Some(platform),
                version: version_of(&format!("{path}.version"), &p.version)?,
                cpu: p.cpu,
                ram: p.ram,
                manifest: manifest_of(&format!("{path}.manifest"), p.manifest.as_ref())?,
                syncs: syncs_of(&format!("{path}.sync"), p.sync.as_ref())?,
                world: None,
                pregen: None,
            });
        }
        for (i, s) in bp.servers.iter().enumerate() {
            let path = format!("servers[{i}]");
            resources(&path, s.cpu, s.ram)?;
            let platform = match &s.platform {
                Some(p) => Some(platform_of(&format!("{path}.platform"), p, false)?),
                None => None,
            };
            let entry = match s.entry.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(e) => Some(crate::network::valid_name(e).map_err(|e| err(&format!("{path}.entry"), e))?),
            };
            if let Some(w) = &s.world {
                let mut a = json!({ "server_id": 0, "local_path": w.local_path });
                if let Some(n) = &w.world_name {
                    a["world_name"] = json!(n);
                }
                crate::world_upload::parse_request(&a).map_err(|e| err(&format!("{path}.world"), e))?;
            }
            if let Some(p) = &s.pregenerate {
                crate::pregen::Request::parse(&pregen_args(0, p)).map_err(|e| err(&format!("{path}.pregenerate"), e))?;
            }
            nodes.push(Node {
                path: path.clone(),
                name: crate::mybox::valid_name(&s.name).map_err(|e| err(&format!("{path}.name"), e))?,
                proxy: false,
                server_id: s.server_id,
                role: role_of(&format!("{path}.role"), &s.role)?,
                entry,
                platform,
                version: version_of(&format!("{path}.version"), &s.version)?,
                cpu: s.cpu,
                ram: s.ram,
                manifest: manifest_of(&format!("{path}.manifest"), s.manifest.as_ref())?,
                syncs: syncs_of(&format!("{path}.sync"), s.sync.as_ref())?,
                world: s.world.clone(),
                pregen: s.pregenerate.clone(),
            });
        }
        // Unicité : noms, identifiants, entrées.
        let mut names = BTreeSet::new();
        let mut ids = BTreeSet::new();
        let mut entries = BTreeSet::new();
        for n in &nodes {
            if !names.insert(n.name.to_lowercase()) {
                return Err(err(&format!("{}.name", n.path), format!("« {} » donné deux fois : chaque serveur a son nom", n.name)));
            }
            if let Some(id) = n.server_id {
                if id <= 0 || !ids.insert(id) {
                    return Err(err(&format!("{}.server_id", n.path), format!("{id} : identifiant invalide ou donné deux fois")));
                }
            }
            if let Some(e) = &n.entry {
                if !entries.insert(e.to_lowercase()) {
                    return Err(err(&format!("{}.entry", n.path), format!("entrée « {e} » donnée deux fois")));
                }
            }
        }
        // Références à des serveurs de jeu du blueprint (nom ou entrée, casse ignorée).
        let find = |what: &str, raw: &str| -> Result<usize, String> {
            let r = raw.trim().to_lowercase();
            nodes
                .iter()
                .position(|n| !n.proxy && (n.name.to_lowercase() == r || n.entry.as_deref().is_some_and(|e| e.to_lowercase() == r)))
                .ok_or_else(|| err(what, format!("« {raw} » n'est ni le nom ni l'entrée d'un serveur de jeu du blueprint")))
        };
        let mut presets: Vec<String> = Vec::new();
        for p in &bp.presets {
            let p = p.trim().to_ascii_lowercase();
            if !PRESETS.contains(&p.as_str()) {
                return Err(err("presets", format!("« {p} » inconnu : luckperms ou geyser")));
            }
            if presets.contains(&p) {
                return Err(err("presets", format!("« {p} » cité deux fois")));
            }
            presets.push(p);
        }
        if !presets.is_empty() && bp.proxy.is_none() {
            return Err(err("presets", "les préréglages de réseau demandent un proxy (`proxy`)"));
        }
        // Ordre d'application fixe : LuckPerms, puis Geyser.
        presets.sort_by_key(|p| PRESETS.iter().position(|x| x == p));
        let proxy = match &bp.proxy {
            None => None,
            Some(p) => {
                let platform = nodes[0].platform.clone().unwrap_or_default();
                let mode = match p.mode.as_deref().map(|m| m.trim().to_ascii_lowercase()) {
                    None => None,
                    Some(m) if m.is_empty() => None,
                    Some(m) if m == "modern" && platform != "velocity" => {
                        return Err(err("proxy.mode", "le transfert moderne est propre à Velocity : BungeeCord et Waterfall ne connaissent que legacy"))
                    }
                    Some(m) if m == "modern" || m == "legacy" => Some(m),
                    Some(m) => return Err(err("proxy.mode", format!("« {m} » : modern ou legacy"))),
                };
                if p.forced_hosts.len() > MAX_FORCED_HOSTS {
                    return Err(err("proxy.forced_hosts", format!("{} domaines, {MAX_FORCED_HOSTS} au plus", p.forced_hosts.len())));
                }
                let mut hosts: Vec<(String, Vec<usize>)> = Vec::new();
                for (h, v) in &p.forced_hosts {
                    let what = format!("proxy.forced_hosts.{h}");
                    let host = crate::network::valid_host(h).map_err(|e| err(&what, e))?;
                    if hosts.iter().any(|(x, _)| *x == host) {
                        return Err(err(&what, "domaine donné deux fois"));
                    }
                    let refs: Vec<String> = match v {
                        Value::String(s) => vec![s.clone()],
                        Value::Array(a) if !a.is_empty() => a
                            .iter()
                            .map(|x| x.as_str().map(str::to_string).ok_or_else(|| err(&what, "noms de serveurs (texte) attendus")))
                            .collect::<Result<_, _>>()?,
                        _ => return Err(err(&what, "nom de serveur ou liste non vide de noms attendu")),
                    };
                    let idx: Vec<usize> = refs.iter().map(|r| find(&what, r)).collect::<Result<_, _>>()?;
                    if platform != "velocity" && idx.len() > 1 {
                        return Err(err(&what, "BungeeCord n'accepte qu'un serveur par domaine"));
                    }
                    hosts.push((host, idx));
                }
                let mut try_order = Vec::new();
                for t in &p.try_order {
                    let i = find("proxy.try", t)?;
                    if try_order.contains(&i) {
                        return Err(err("proxy.try", format!("« {t} » cité deux fois")));
                    }
                    try_order.push(i);
                }
                if let Some(port) = p.bedrock_port {
                    if port == 0 {
                        return Err(err("proxy.bedrock_port", "port 1-65535 attendu"));
                    }
                    if !bp.presets.iter().any(|x| x.trim().eq_ignore_ascii_case("geyser")) {
                        return Err(err("proxy.bedrock_port", "ne sert qu'au préréglage geyser (`presets`)"));
                    }
                }
                Some(ProxyOpts { mode, forced_hosts: hosts, try_order, prune: p.prune, bedrock_port: p.bedrock_port })
            }
        };
        if let Some(d) = &bp.database {
            if !presets.iter().any(|p| p == "luckperms") {
                return Err(err("database", "ne sert qu'au préréglage luckperms (`presets`) ; pour un autre plugin : connect_database"));
            }
            if d.database_id <= 0 || d.mybox_id.is_some_and(|m| m <= 0) {
                return Err(err("database", "identifiants entiers attendus (mybox_info : bases de la MyBox)"));
            }
        }
        if bp.egg_id.is_some_and(|e| e <= 0) || bp.location_id.is_some_and(|l| l <= 0) {
            return Err(err("egg_id / location_id", "identifiants entiers attendus (server_catalog)"));
        }
        Ok(Network {
            name: bp.name.map(|n| n.trim().to_string()),
            mybox_id: bp.mybox_id,
            egg_id: bp.egg_id,
            location_id: bp.location_id,
            nodes,
            proxy,
            presets,
            database: bp.database,
        })
    }
}

/// Identifiants des serveurs existants que désigne un blueprint (`server_id` du proxy et des
/// serveurs), pour le mode constructeur (`agent_guard::scope::write_targets`) : lecture pure, sans
/// validation complète. `Err(clé)` : identifiant ou blueprint illisible.
pub(crate) fn target_ids(args: &Value) -> Result<Vec<i64>, String> {
    let bp = match args.get("blueprint") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).map_err(|_| "blueprint".to_string())?,
        Some(v) => v.clone(),
    };
    let id = |v: &Value| -> Result<Option<i64>, String> {
        match v.get("server_id") {
            None | Some(Value::Null) => Ok(None),
            Some(x) => x
                .as_i64()
                .or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))
                .map(Some)
                .ok_or_else(|| "server_id".to_string()),
        }
    };
    let mut out = Vec::new();
    if let Some(p) = bp.get("proxy").filter(|p| p.is_object()) {
        out.extend(id(p)?);
    }
    match bp.get("servers") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for s in list {
                out.extend(id(s)?);
            }
        }
        Some(_) => return Err("servers".into()),
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Manifeste effectivement appliqué à un serveur : celui du blueprint, plus son nom (renommage
/// d'un serveur désigné par `server_id`) et Chunky quand une prégénération est demandée. `None` :
/// rien à appliquer. `chunky_kind` : `plugin` ou `mod` selon la plateforme.
pub(crate) fn effective_manifest(node: &Node, existing: bool, chunky_kind: Option<&str>) -> Option<Manifest> {
    let mut m = node.manifest.clone().unwrap_or_else(|| Manifest { version: crate::manifest::MANIFEST_VERSION, ..Default::default() });
    if existing && node.server_id.is_some() {
        m.server = Some(crate::manifest::ServerSpec { name: Some(node.name.clone()), ..Default::default() });
    }
    if let (Some(_), Some(kind)) = (&node.pregen, chunky_kind) {
        let (mut items, remove) = m.content_items().unwrap_or_default();
        let has = items.iter().any(|i| {
            [i.project.as_deref(), i.name.as_deref(), i.file.as_deref()]
                .iter()
                .flatten()
                .any(|t| t.to_ascii_lowercase().contains("chunky") && !t.to_ascii_lowercase().contains("border"))
        });
        if !has {
            items.push(crate::manifest::ContentItem {
                kind: kind.into(),
                source: Some("modrinth".into()),
                project: Some("chunky".into()),
                ..Default::default()
            });
            m.content = Some(json!({ "items": items, "remove_unlisted": remove }));
        }
    }
    let empty = m.server.is_none()
        && m.properties.as_ref().is_none_or(Map::is_empty)
        && m.startup.is_none()
        && m.content_items().map(|(i, _)| i.is_empty()).unwrap_or(true)
        && m.configs.is_empty()
        && m.files.is_empty()
        && m.players.is_none()
        && m.schedule_items().map(|(s, r)| s.is_empty() && !r).unwrap_or(true);
    (!empty).then_some(m)
}

/// Le manifeste a-t-il du contenu (plugins, mods, datapacks) ?
pub(crate) fn has_content(m: &Manifest) -> bool {
    m.content_items().map(|(i, r)| !i.is_empty() || r).unwrap_or(false)
}

/// Une étape `manual` d'un sous-outil se résout-elle par un démarrage du serveur (fichier pas
/// encore généré) ? Convention des messages de `apply_manifest`, `link_proxy` et `network_preset`.
pub(crate) fn needs_start_hint(summary: &str) -> bool {
    summary.to_lowercase().contains("démarr")
}

/// Texte JSON d'un sous-outil, ou la chaîne brute s'il n'en est pas.
pub(crate) fn parse_out(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string()))
}

// --- Schéma ------------------------------------------------------------------------------------------

/// Schéma JSON de l'argument `blueprint` (description du format, validation stricte dans l'outil).
pub(crate) fn blueprint_schema() -> Value {
    json!({
        "type": "object",
        "description": "Blueprint v1 : { version: 1, name?, mybox_id, egg_id?, location_id?, proxy?: { name, server_id?, platform?: velocity|bungeecord|waterfall, version?, cpu?, ram?, mode?: modern|legacy, forced_hosts?: { domaine: nom | [noms] }, try?: [noms], prune?, bedrock_port?, manifest?, sync? }, servers: [{ name, server_id?, role?, entry?, platform?, version?, cpu?, ram?, manifest?: manifeste v1 sans `server` (properties, content, configs, files, players, startup, schedules), sync?: { local_dir, remote_dir, delete_extra?, include?, exclude? } ou liste, world?: { local_path, world_name?, replace? }, pregenerate?: { radius, world?, center?, shape? } }], presets?: [luckperms, geyser], database?: { database_id, mybox_id? } }. Champ inconnu = erreur. Serveur sans server_id : rapproché par son nom exact dans la MyBox, sinon créé (cpu et ram requis). Secrets : {{secret:NOM}} dans les manifestes.",
        "properties": {
            "version": { "type": "integer", "description": "1" },
            "mybox_id": { "type": "integer", "description": "MyBox du réseau" },
            "proxy": { "type": "object", "description": "Proxy (facultatif)" },
            "servers": { "type": "array", "items": { "type": "object" }, "description": "Serveurs de jeu (1 à 20)" },
            "presets": { "type": "array", "items": { "type": "string", "enum": PRESETS } },
            "database": { "type": "object", "description": "{ database_id, mybox_id? } : base partagée de LuckPerms" }
        },
        "required": ["version", "mybox_id", "servers"]
    })
}
